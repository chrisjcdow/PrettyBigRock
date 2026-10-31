// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The capability derivation tree (docs/design/0006-objects-and-capabilities.md, section 5).
//!
//! Every capability is a node. A copy is a child of the node it was copied from. Closing a
//! node removes it and gives its children to its parent, so copies made from it keep
//! working and revoking further up still reaches them. Revoking a node removes every
//! descendant and keeps the node itself.
//!
//! The tree only records the family relationships and a value per node; what a node *means*
//! (which table slot or mapping it is) is the kernel's business, in that value. It's plain
//! logic, so it's tested on the host against a simple model.
//!
//! Nodes live in an arena and are named by a [`NodeId`] with a generation, so a stale id
//! never reaches a node that has since reused its slot. Nothing here recurses, so a deep tree
//! can't overflow a kernel stack.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::vec::Vec;

/// Names a node. Ids of removed nodes are never valid again.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId {
    index: u32,
    generation: u32,
}

impl NodeId {
    /// The id as one number, for diagnostics.
    pub fn raw(self) -> u64 {
        (u64::from(self.generation) << 32) | u64::from(self.index)
    }
}

const NONE: u32 = u32::MAX;

struct Node<T> {
    parent: u32,
    first_child: u32,
    prev_sibling: u32,
    next_sibling: u32,
    value: T,
}

struct Slot<T> {
    generation: u32,
    node: Option<Node<T>>,
}

/// Why an operation failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The id doesn't name a node in the tree (it was removed, or never existed).
    NoSuchNode,
    /// The tree has as many nodes as it can number.
    Full,
}

/// A forest of nodes, each holding a `T`.
pub struct Tree<T> {
    slots: Vec<Slot<T>>,
    free: Vec<u32>,
    len: usize,
}

impl<T> Default for Tree<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Tree<T> {
    /// An empty tree.
    pub const fn new() -> Self {
        Self { slots: Vec::new(), free: Vec::new(), len: 0 }
    }

    /// How many nodes there are.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True if there are no nodes.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn node(&self, id: NodeId) -> Option<&Node<T>> {
        let s = self.slots.get(id.index as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.node.as_ref()
    }

    fn at(&self, index: u32) -> &Node<T> {
        self.slots[index as usize].node.as_ref().expect("a link points at an empty slot")
    }

    fn at_mut(&mut self, index: u32) -> &mut Node<T> {
        self.slots[index as usize].node.as_mut().expect("a link points at an empty slot")
    }

    fn id_of(&self, index: u32) -> NodeId {
        NodeId { index, generation: self.slots[index as usize].generation }
    }

    fn check(&self, id: NodeId) -> Result<u32, Error> {
        self.node(id).map(|_| id.index).ok_or(Error::NoSuchNode)
    }

    /// True if `id` names a node.
    pub fn contains(&self, id: NodeId) -> bool {
        self.node(id).is_some()
    }

    /// The value at `id`.
    pub fn get(&self, id: NodeId) -> Option<&T> {
        self.node(id).map(|n| &n.value)
    }

    /// The value at `id`, to change it.
    pub fn get_mut(&mut self, id: NodeId) -> Option<&mut T> {
        let s = self.slots.get_mut(id.index as usize)?;
        if s.generation != id.generation {
            return None;
        }
        s.node.as_mut().map(|n| &mut n.value)
    }

    /// The node `id` was copied from, if any.
    pub fn parent(&self, id: NodeId) -> Option<NodeId> {
        let n = self.node(id)?;
        (n.parent != NONE).then(|| self.id_of(n.parent))
    }

    /// The nodes copied directly from `id`.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        if let Some(n) = self.node(id) {
            let mut c = n.first_child;
            while c != NONE {
                out.push(self.id_of(c));
                c = self.at(c).next_sibling;
            }
        }
        out
    }

    /// True if `id` has any copies.
    pub fn has_children(&self, id: NodeId) -> bool {
        self.node(id).is_some_and(|n| n.first_child != NONE)
    }

    fn alloc(&mut self, node: Node<T>) -> Result<u32, Error> {
        let index = if let Some(i) = self.free.pop() {
            i
        } else {
            let i = u32::try_from(self.slots.len()).map_err(|_| Error::Full)?;
            if i == NONE {
                return Err(Error::Full);
            }
            self.slots.push(Slot { generation: 0, node: None });
            i
        };
        self.slots[index as usize].node = Some(node);
        self.len += 1;
        Ok(index)
    }

    /// Adds a node with no parent: an original capability.
    pub fn insert_root(&mut self, value: T) -> Result<NodeId, Error> {
        let i = self.alloc(Node {
            parent: NONE,
            first_child: NONE,
            prev_sibling: NONE,
            next_sibling: NONE,
            value,
        })?;
        Ok(self.id_of(i))
    }

    /// Adds a copy of `parent`.
    pub fn insert_child(&mut self, parent: NodeId, value: T) -> Result<NodeId, Error> {
        let p = self.check(parent)?;
        let i = self.alloc(Node {
            parent: NONE,
            first_child: NONE,
            prev_sibling: NONE,
            next_sibling: NONE,
            value,
        })?;
        self.link(i, p);
        Ok(self.id_of(i))
    }

    /// Makes `child` the first child of `parent` (`child` must be unlinked).
    fn link(&mut self, child: u32, parent: u32) {
        let first = self.at(parent).first_child;
        {
            let c = self.at_mut(child);
            c.parent = parent;
            c.prev_sibling = NONE;
            c.next_sibling = first;
        }
        if first != NONE {
            self.at_mut(first).prev_sibling = child;
        }
        self.at_mut(parent).first_child = child;
    }

    /// Takes `n` out of its parent's list of children.
    fn unlink(&mut self, n: u32) {
        let (parent, prev, next) = {
            let x = self.at(n);
            (x.parent, x.prev_sibling, x.next_sibling)
        };
        if prev != NONE {
            self.at_mut(prev).next_sibling = next;
        } else if parent != NONE {
            self.at_mut(parent).first_child = next;
        }
        if next != NONE {
            self.at_mut(next).prev_sibling = prev;
        }
        let x = self.at_mut(n);
        x.parent = NONE;
        x.prev_sibling = NONE;
        x.next_sibling = NONE;
    }

    fn free_slot(&mut self, index: u32) -> T {
        let slot = &mut self.slots[index as usize];
        let node = slot.node.take().expect("freeing an empty slot");
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(index);
        self.len -= 1;
        node.value
    }

    /// Closes `id`: removes it, and gives its children to its parent (or makes them originals
    /// if it had none). Returns its value.
    pub fn remove(&mut self, id: NodeId) -> Result<T, Error> {
        let n = self.check(id)?;
        let parent = self.at(n).parent;
        // Move the children up, one by one.
        loop {
            let c = self.at(n).first_child;
            if c == NONE {
                break;
            }
            self.unlink(c);
            if parent != NONE {
                self.link(c, parent);
            }
        }
        self.unlink(n);
        Ok(self.free_slot(n))
    }

    /// Revokes `id`: removes every descendant of it, wherever it is, and keeps `id` itself.
    /// `destroyed` is called with each removed value, children before their parents.
    pub fn revoke(
        &mut self,
        id: NodeId,
        mut destroyed: impl FnMut(NodeId, T),
    ) -> Result<(), Error> {
        let top = self.check(id)?;
        // Walk down to a leaf, remove it, then carry on from its parent: each node is visited
        // a bounded number of times, with no recursion and no extra memory.
        let mut n = self.at(top).first_child;
        while n != NONE {
            loop {
                let c = self.at(n).first_child;
                if c == NONE {
                    break;
                }
                n = c;
            }
            let parent = self.at(n).parent;
            let removed_id = self.id_of(n);
            self.unlink(n);
            let v = self.free_slot(n);
            destroyed(removed_id, v);
            n = if parent == top { self.at(top).first_child } else { parent };
        }
        Ok(())
    }

    /// Removes `id` and all its descendants. `destroyed` is called with each, `id` last.
    pub fn remove_subtree(
        &mut self,
        id: NodeId,
        mut destroyed: impl FnMut(NodeId, T),
    ) -> Result<(), Error> {
        self.revoke(id, &mut destroyed)?;
        let v = self.remove(id)?;
        destroyed(id, v);
        Ok(())
    }

    /// Every descendant of `id` (not including `id`), in no particular order.
    pub fn descendants(&self, id: NodeId) -> Vec<NodeId> {
        let mut out = Vec::new();
        let Some(n) = self.node(id) else { return out };
        let mut stack = Vec::new();
        if n.first_child != NONE {
            stack.push(n.first_child);
        }
        while let Some(i) = stack.pop() {
            out.push(self.id_of(i));
            let x = self.at(i);
            if x.next_sibling != NONE {
                stack.push(x.next_sibling);
            }
            if x.first_child != NONE {
                stack.push(x.first_child);
            }
        }
        out
    }

    /// True if `ancestor` is `id`'s parent, or its parent's parent, and so on.
    pub fn is_descendant(&self, id: NodeId, ancestor: NodeId) -> bool {
        let Some(mut n) = self.node(id) else { return false };
        if !self.contains(ancestor) {
            return false;
        }
        while n.parent != NONE {
            if n.parent == ancestor.index {
                return true;
            }
            n = self.at(n.parent);
        }
        false
    }

    /// Checks every link is consistent, for tests: each child's parent is right, sibling
    /// lists run both ways, and the count of nodes is right. Returns the first problem found.
    pub fn verify(&self) -> Result<(), &'static str> {
        let mut seen = 0;
        for (i, s) in self.slots.iter().enumerate() {
            let Some(n) = &s.node else { continue };
            seen += 1;
            let i = i as u32;
            if n.parent != NONE {
                let p = self.slots.get(n.parent as usize).and_then(|s| s.node.as_ref());
                let Some(p) = p else { return Err("a parent link points at an empty slot") };
                // `i` must be in the parent's child list.
                let mut c = p.first_child;
                let mut found = false;
                let mut steps = 0;
                while c != NONE {
                    if c == i {
                        found = true;
                    }
                    c = self.at(c).next_sibling;
                    steps += 1;
                    if steps > self.slots.len() {
                        return Err("a child list loops");
                    }
                }
                if !found {
                    return Err("a node isn't in its parent's list of children");
                }
            } else if n.prev_sibling != NONE || n.next_sibling != NONE {
                return Err("an original has siblings");
            }
            if n.next_sibling != NONE && self.at(n.next_sibling).prev_sibling != i {
                return Err("sibling links disagree");
            }
            if n.prev_sibling != NONE && self.at(n.prev_sibling).next_sibling != i {
                return Err("sibling links disagree");
            }
            let mut c = n.first_child;
            while c != NONE {
                if self.at(c).parent != i {
                    return Err("a child's parent link is wrong");
                }
                c = self.at(c).next_sibling;
            }
        }
        if seen != self.len {
            return Err("the node count is wrong");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn copies_close_and_revoke() {
        let mut t = Tree::new();
        let a = t.insert_root("a").unwrap();
        let b = t.insert_child(a, "b").unwrap();
        let c = t.insert_child(b, "c").unwrap();
        let d = t.insert_child(b, "d").unwrap();
        assert_eq!(t.parent(c), Some(b));
        // Closing b gives c and d to a.
        assert_eq!(t.remove(b), Ok("b"));
        assert_eq!(t.parent(c), Some(a));
        assert_eq!(t.parent(d), Some(a));
        assert!(!t.contains(b));
        assert_eq!(t.get(b), None);
        // A new node reusing b's slot isn't reachable through b's old id.
        let e = t.insert_root("e").unwrap();
        assert_ne!(e, b);
        assert_eq!(t.get(b), None);
        // Revoking a removes c and d, keeps a.
        let mut gone = Vec::new();
        t.revoke(a, |_, v| gone.push(v)).unwrap();
        gone.sort();
        assert_eq!(gone, ["c", "d"]);
        assert!(t.contains(a));
        assert_eq!(t.len(), 2);
        t.verify().unwrap();
    }

    #[test]
    fn stale_ids_fail() {
        let mut t = Tree::new();
        let a = t.insert_root(1).unwrap();
        t.remove(a).unwrap();
        assert_eq!(t.remove(a), Err(Error::NoSuchNode));
        assert_eq!(t.insert_child(a, 2), Err(Error::NoSuchNode));
        assert_eq!(t.revoke(a, |_, _| {}), Err(Error::NoSuchNode));
    }

    #[test]
    fn deep_trees_dont_recurse() {
        let mut t = Tree::new();
        let root = t.insert_root(0).unwrap();
        let mut n = root;
        for i in 1..200_000 {
            n = t.insert_child(n, i).unwrap();
        }
        assert!(t.is_descendant(n, root));
        let mut count = 0;
        t.revoke(root, |_, _| count += 1).unwrap();
        assert_eq!(count, 199_999);
        assert_eq!(t.len(), 1);
    }

    /// A small random number generator, so the test is repeatable.
    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
        fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }
    }

    /// Random operations, checked against a model that just records each node's parent.
    #[test]
    fn matches_a_model() {
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
            let mut t: Tree<u64> = Tree::new();
            let mut model: BTreeMap<u64, Option<u64>> = BTreeMap::new(); // value -> parent value
            let mut ids: BTreeMap<u64, NodeId> = BTreeMap::new();
            let mut dead: Vec<NodeId> = Vec::new();
            let mut next = 0u64;
            for _ in 0..3000 {
                let live: Vec<u64> = model.keys().copied().collect();
                match rng.below(10) {
                    0 | 1 => {
                        let id = t.insert_root(next).unwrap();
                        model.insert(next, None);
                        ids.insert(next, id);
                        next += 1;
                    }
                    2..=5 if !live.is_empty() => {
                        let p = live[rng.below(live.len())];
                        let id = t.insert_child(ids[&p], next).unwrap();
                        model.insert(next, Some(p));
                        ids.insert(next, id);
                        next += 1;
                    }
                    6 | 7 if !live.is_empty() => {
                        let v = live[rng.below(live.len())];
                        let id = ids.remove(&v).unwrap();
                        assert_eq!(t.remove(id), Ok(v));
                        dead.push(id);
                        let gp = model.remove(&v).unwrap();
                        for p in model.values_mut() {
                            if *p == Some(v) {
                                *p = gp;
                            }
                        }
                    }
                    8 if !live.is_empty() => {
                        let v = live[rng.below(live.len())];
                        // Everything below v in the model.
                        let mut below = BTreeSet::new();
                        loop {
                            let more: Vec<u64> = model
                                .iter()
                                .filter(|(k, p)| {
                                    !below.contains(*k)
                                        && p.is_some_and(|p| p == v || below.contains(&p))
                                })
                                .map(|(k, _)| *k)
                                .collect();
                            if more.is_empty() {
                                break;
                            }
                            below.extend(more);
                        }
                        let mut got = BTreeSet::new();
                        t.revoke(ids[&v], |id, x| {
                            got.insert(x);
                            dead.push(id);
                        })
                        .unwrap();
                        assert_eq!(got, below, "seed {seed}");
                        for x in &below {
                            model.remove(x);
                            ids.remove(x);
                        }
                    }
                    _ => {}
                }
                t.verify().unwrap();
            }
            // Every live node's parent agrees with the model; dead ids are dead.
            assert_eq!(t.len(), model.len());
            for (v, p) in &model {
                let id = ids[v];
                assert_eq!(t.get(id), Some(v));
                assert_eq!(t.parent(id).map(|p| *t.get(p).unwrap()), *p, "seed {seed}");
            }
            for id in dead {
                assert!(!t.contains(id));
            }
        }
    }
}
