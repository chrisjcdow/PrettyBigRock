// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The namespace: the tree of named objects the tables build (ACPI 6.5, section 5.3).
//!
//! Each node has a four-character name, a parent, children and one object: data (`Name`), a
//! method, a device, an operation region, a field and so on. Nodes live in one list and are
//! referred to by [`NodeId`]: an index and a *generation*, a count that goes up each time
//! the slot is reused. Names a method creates are removed when it returns, so a reference the
//! firmware kept to one could otherwise end up meaning whatever took its place; with the
//! generation it means nothing, and looking it up fails.

use crate::code::{AmlName, write_seg};
use crate::field::{FieldUnit, Region};
use crate::value::{BufferField, Value};
use crate::{Error, Result, limits};
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// A node of the namespace.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

/// A method: its body, kept as bytecode and parsed each time it runs (the grammar depends on
/// the namespace at that moment, note 0028, section 2).
pub(crate) struct Method {
    pub code: crate::code::Code,
    /// How many arguments it takes, 0 to 7.
    pub args: u8,
    /// A serialized method may only run once at a time. This interpreter runs one evaluation
    /// at a time anyway, so it changes nothing here.
    #[allow(dead_code)]
    pub serialized: bool,
}

/// What the interpreter provides itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BuiltIn {
    /// `\_OSI(name)`: true for the Windows versions (note 0028, section 2).
    Osi,
}

/// A mutex the firmware's methods take and let go of. One evaluation runs at a time, so
/// taking one never waits; `depth` counts how many times it is held (a method may take it
/// again while it holds it).
#[derive(Debug, Default)]
pub(crate) struct MutexState {
    /// Its sync level, 0 to 15: the order mutexes must be taken in. Kept, not checked.
    #[allow(dead_code)]
    pub sync: u8,
    pub depth: u32,
}

/// A node's object. The kinds follow `ObjectType`'s numbering in [`Object::kind`].
pub(crate) enum Object {
    /// A scope with nothing else: `\`, `\_GPE`, `\_PR`, `\_SI`, `\_TZ`.
    Scope,
    /// Data, from `Name` or a store.
    Data(Value),
    Method(Rc<Method>),
    BuiltIn(BuiltIn),
    Device,
    /// The old way of declaring a processor; its numbers aren't needed.
    Processor,
    PowerResource,
    ThermalZone,
    Mutex(MutexState),
    /// An event's count of signals waiting.
    Event(u64),
    Region(Rc<RefCell<Region>>),
    Field(Rc<FieldUnit>),
    BufferField(BufferField),
    /// Another name for a node.
    Alias(NodeId),
    /// `External`: declared in this table, defined in another; `args` if a method.
    External(u8),
}

impl Object {
    /// The number `ObjectType` gives this kind (ACPI 6.5, section 19.6.97).
    pub fn kind(&self) -> u64 {
        match self {
            Object::Scope | Object::External(_) | Object::Alias(_) => 0,
            Object::Data(v) => v.kind(),
            Object::Field(_) => 5,
            Object::Device => 6,
            Object::Event(_) => 7,
            Object::Method(_) | Object::BuiltIn(_) => 8,
            Object::Mutex(_) => 9,
            Object::Region(_) => 10,
            Object::PowerResource => 11,
            Object::Processor => 12,
            Object::ThermalZone => 13,
            Object::BufferField(_) => 14,
        }
    }
}

pub(crate) struct Node {
    pub name: [u8; 4],
    pub parent: Option<NodeId>,
    pub children: BTreeMap<[u8; 4], NodeId>,
    pub object: Object,
}

struct Slot {
    generation: u32,
    node: Option<Node>,
}

/// The whole tree.
pub(crate) struct Namespace {
    slots: Vec<Slot>,
    free: Vec<u32>,
    count: usize,
}

impl Namespace {
    /// Just the root, `\`.
    pub fn new() -> Namespace {
        let root = Node {
            name: *b"\\___",
            parent: None,
            children: BTreeMap::new(),
            object: Object::Scope,
        };
        Namespace {
            slots: alloc::vec![Slot { generation: 0, node: Some(root) }],
            free: Vec::new(),
            count: 1,
        }
    }

    pub fn root(&self) -> NodeId {
        NodeId { index: 0, generation: 0 }
    }

    pub fn len(&self) -> usize {
        self.count
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        let slot = self.slots.get(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_ref()
    }

    pub fn node_mut(&mut self, id: NodeId) -> Option<&mut Node> {
        let slot = self.slots.get_mut(id.index as usize)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.node.as_mut()
    }

    pub fn object(&self, id: NodeId) -> Option<&Object> {
        self.node(id).map(|n| &n.object)
    }

    pub fn object_mut(&mut self, id: NodeId) -> Option<&mut Object> {
        self.node_mut(id).map(|n| &mut n.object)
    }

    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        self.node(id)?.parent
    }

    pub fn child(&self, id: NodeId, seg: &[u8; 4]) -> Option<NodeId> {
        self.node(id)?.children.get(seg).copied()
    }

    /// A node's children, in name order.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.node(id).map(|n| n.children.values().copied().collect()).unwrap_or_default()
    }

    /// The node's name segment.
    pub fn name(&self, id: NodeId) -> [u8; 4] {
        self.node(id).map(|n| n.name).unwrap_or(*b"____")
    }

    /// Adds `object` as `seg` under `parent`.
    pub fn add(&mut self, parent: NodeId, seg: [u8; 4], object: Object) -> Result<NodeId> {
        if self.count >= limits::NODES {
            return Err(Error::Limit("objects in the namespace"));
        }
        if self.node(parent).is_none() {
            return Err(Error::NotFound(String::from("the scope")));
        }
        let node = Node { name: seg, parent: Some(parent), children: BTreeMap::new(), object };
        let id = match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                slot.generation = slot.generation.wrapping_add(1);
                slot.node = Some(node);
                NodeId { index, generation: slot.generation }
            }
            None => {
                let index = u32::try_from(self.slots.len()).map_err(|_| Error::Limit("objects"))?;
                self.slots.push(Slot { generation: 0, node: Some(node) });
                NodeId { index, generation: 0 }
            }
        };
        self.count += 1;
        if let Some(p) = self.node_mut(parent) {
            p.children.insert(seg, id);
        }
        Ok(id)
    }

    /// Removes a node and everything under it. Without recursion, so a deep tree can't run
    /// the stack out.
    pub fn remove(&mut self, id: NodeId) {
        let Some(parent) = self.parent(id) else { return };
        let seg = self.name(id);
        if let Some(p) = self.node_mut(parent)
            && p.children.get(&seg) == Some(&id)
        {
            p.children.remove(&seg);
        }
        let mut doomed = alloc::vec![id];
        while let Some(d) = doomed.pop() {
            let Some(slot) = self.slots.get_mut(d.index as usize) else { continue };
            if slot.generation != d.generation {
                continue;
            }
            if let Some(node) = slot.node.take() {
                doomed.extend(node.children.values().copied());
                self.free.push(d.index);
                self.count -= 1;
            }
        }
    }

    /// The node `name` means from `scope`, using the search rules for a single bare segment
    /// (ACPI 6.5, section 5.3): looked for in the scope, then each scope above it.
    pub fn lookup(&self, scope: NodeId, name: &AmlName) -> Option<NodeId> {
        if name.searched() {
            let mut at = Some(scope);
            while let Some(s) = at {
                if let Some(found) = self.child(s, &name.segs[0]) {
                    return Some(found);
                }
                at = self.parent(s);
            }
            return None;
        }
        self.lookup_exact(scope, name)
    }

    /// The node `name` means from `scope`, without searching.
    pub fn lookup_exact(&self, scope: NodeId, name: &AmlName) -> Option<NodeId> {
        let mut at = self.start(scope, name)?;
        for seg in &name.segs {
            at = self.child(self.through_alias(at), seg)?;
        }
        Some(at)
    }

    /// Where `name` starts from: the root, or `scope` and up for each `^`.
    pub fn start(&self, scope: NodeId, name: &AmlName) -> Option<NodeId> {
        if name.absolute {
            return Some(self.root());
        }
        let mut at = scope;
        for _ in 0..name.parents {
            at = self.parent(at)?;
        }
        Some(at)
    }

    /// For creating `name` from `scope`: the scope it goes in, and its last segment.
    pub fn parent_for(&self, scope: NodeId, name: &AmlName) -> Result<(NodeId, [u8; 4])> {
        let missing = || Error::NotFound(alloc::format!("{name}"));
        let (last, path) = name.segs.split_last().ok_or(Error::Malformed("an empty name"))?;
        let mut at = self.start(scope, name).ok_or_else(missing)?;
        for seg in path {
            at = self.child(self.through_alias(at), seg).ok_or_else(missing)?;
        }
        Ok((self.through_alias(at), *last))
    }

    /// The node an alias stands for (or the node itself). Aliases of aliases are followed a
    /// few times only, so a loop of them can't hang.
    pub fn through_alias(&self, mut id: NodeId) -> NodeId {
        for _ in 0..8 {
            match self.object(id) {
                Some(Object::Alias(to)) => id = *to,
                _ => break,
            }
        }
        id
    }

    /// The node's full name, such as `\_SB.PCI0.LPCB`.
    pub fn path(&self, id: NodeId) -> String {
        let mut segs = Vec::new();
        let mut at = Some(id);
        while let Some(n) = at {
            let Some(node) = self.node(n) else { break };
            if node.parent.is_some() {
                segs.push(node.name);
            }
            at = node.parent;
        }
        let mut s = String::from("\\");
        for (i, seg) in segs.iter().rev().enumerate() {
            if i > 0 {
                s.push('.');
            }
            let _ = write_seg(&mut s, seg);
        }
        s
    }

    /// Every node under `id`, depth first, parents before their children, in name order.
    /// `enter` says whether to look under a node.
    pub fn walk(
        &self,
        id: NodeId,
        mut enter: impl FnMut(&Namespace, NodeId) -> bool,
    ) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.children(id).into_iter().rev().collect();
        while let Some(n) = stack.pop() {
            out.push(n);
            if enter(self, n) {
                stack.extend(self.children(n).into_iter().rev());
            }
        }
        out
    }
}
