// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Running AML: lists of terms, named objects, methods, names and stores (ACPI 6.5, sections
//! 19.3.5 and 20.2.5).
//!
//! The bytecode is run as it is read, by *recursive descent*: a function for each part of
//! the grammar, calling the others for the parts inside it. Every call counts against the
//! nesting limit, every operation against the operation budget, so neither deep nor endless
//! bytecode can run the service's stack out or hang it.
//!
//! Loading a table runs its top level in the root scope, in two passes as ACPICA does: the
//! first only declares the methods and the devices that hold them, so code the table runs as
//! it loads can call a method defined further down; the second runs everything in order.

use crate::code::{AmlName, Code, starts_name};
use crate::field::{FieldUnit, Place, Reach, Region};
use crate::namespace::{BuiltIn, Method, MutexState, NodeId, Object};
use crate::op;
use crate::value::{BufferField, Ref, Reference, ToText, Value};
use crate::{Aml, Error, Host, Result, limits};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

/// How a list of terms ended.
pub(crate) enum Flow {
    /// It ran to its end.
    Next,
    Break,
    Continue,
    Return(Value),
}

/// The state of one method's run, or of a table's top level as it loads.
pub(crate) struct Frame {
    /// Where names are looked up from and created in.
    pub scope: NodeId,
    pub args: Vec<Rc<RefCell<Value>>>,
    pub locals: Vec<Rc<RefCell<Value>>>,
    /// The names a method created, removed when it returns; `None` while a table loads, when
    /// what is created stays.
    pub created: Option<Vec<NodeId>>,
}

impl Frame {
    /// A table's top level, or a deferred region's arguments, in `scope`.
    pub fn load(scope: NodeId) -> Frame {
        Frame { scope, args: Vec::new(), locals: Vec::new(), created: None }
    }

    fn method(scope: NodeId, args: &[Value]) -> Frame {
        let cell = |v: Value| Rc::new(RefCell::new(v));
        let mut a: Vec<_> = args.iter().cloned().map(cell).collect();
        a.resize_with(7, || cell(Value::Uninitialized));
        let locals = (0..8).map(|_| cell(Value::Uninitialized)).collect();
        Frame { scope, args: a, locals, created: Some(Vec::new()) }
    }

    fn loading(&self) -> bool {
        self.created.is_none()
    }
}

/// Where a store goes (a `SuperName` or `Target`).
pub(crate) enum Target {
    /// `NullName`: nowhere.
    None,
    Local(usize),
    Arg(usize),
    Node(NodeId),
    /// The debug object: the value goes to the log.
    Debug,
    Ref(Ref),
}

/// The Windows versions `_OSI` says yes to: those ACPICA knows, from Windows 2000 to Windows
/// 11's 2022 (note 0028, section 2), and ACPICA's one feature string that is always true.
const OSI_YES: &[&str] = &[
    "Windows 2000",
    "Windows 2001",
    "Windows 2001 SP1",
    "Windows 2001.1",
    "Windows 2001 SP2",
    "Windows 2001.1 SP1",
    "Windows 2006",
    "Windows 2006.1",
    "Windows 2006 SP1",
    "Windows 2006 SP2",
    "Windows 2009",
    "Windows 2012",
    "Windows 2013",
    "Windows 2015",
    "Windows 2016",
    "Windows 2017",
    "Windows 2017.2",
    "Windows 2018",
    "Windows 2018.2",
    "Windows 2019",
    "Windows 2020",
    "Windows 2021",
    "Windows 2022",
    "Extended Address Space Descriptor",
];

/// Reads an opcode: one byte, or two after the extended prefix `0x5B`.
pub(crate) fn opcode(code: &mut Code) -> Result<u16> {
    let b = code.byte()?;
    if b == 0x5B { Ok(0x5B00 | u16::from(code.byte()?)) } else { Ok(u16::from(b)) }
}

/// The next opcode, without moving past it.
pub(crate) fn peek_opcode(code: &Code) -> Result<u16> {
    let b = code.peek()?;
    if b == 0x5B { Ok(0x5B00 | u16::from(code.at(1)?)) } else { Ok(u16::from(b)) }
}

impl<H: Host> Aml<H> {
    /// The names the specification defines before any table loads (ACPI 6.5, section 5.3.1).
    pub(crate) fn predefine(&mut self) {
        let root = self.ns.root();
        let names: [(&[u8; 4], Object); 9] = [
            (b"_GPE", Object::Scope),
            (b"_PR_", Object::Scope),
            (b"_SB_", Object::Device),
            (b"_SI_", Object::Scope),
            (b"_TZ_", Object::Scope),
            (b"_GL_", Object::Mutex(MutexState::default())),
            (b"_OSI", Object::BuiltIn(BuiltIn::Osi)),
            (b"_OS_", Object::Data(Value::String(String::from("Microsoft Windows NT")))),
            (b"_REV", Object::Data(Value::Integer(2))),
        ];
        for (name, object) in names {
            // The namespace is empty and far from its limit, so this can't fail.
            let _ = self.ns.add(root, *name, object);
        }
    }

    /// Counts one operation, and checks the clock now and then.
    pub(crate) fn tick(&mut self) -> Result<()> {
        self.run.ops += 1;
        if self.run.ops > self.operations {
            return Err(Error::Limit("operations"));
        }
        if self.run.ops.is_multiple_of(4096) && self.host.now() > self.run.deadline {
            return Err(Error::Limit("time"));
        }
        Ok(())
    }

    /// Runs `f` one level deeper, failing past the nesting limit.
    pub(crate) fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T>) -> Result<T> {
        if self.run.nesting >= limits::NESTING {
            return Err(Error::Limit("nesting"));
        }
        self.run.nesting += 1;
        let r = f(self);
        self.run.nesting -= 1;
        r
    }

    // ---- Loading ----

    /// Loads one table's AML (the DSDT or an SSDT, header and all) into the namespace.
    pub(crate) fn load_aml(&mut self, bytes: Rc<[u8]>) -> Result<()> {
        let root = self.ns.root();
        let body = Code::new(bytes, 36, usize::MAX);
        self.declare(body.clone(), root);
        let mut frame = Frame::load(root);
        let mut code = body;
        let end = code.end;
        self.term_list(&mut code, end, &mut frame).map(|_| ())
    }

    /// The first pass: declares the methods, and the scopes that hold them, without running
    /// anything. Stops quietly at anything it can't step over; the second pass reports it.
    fn declare(&mut self, mut code: Code, scope: NodeId) {
        while !code.done() {
            if self.declare_one(&mut code, scope).is_err() {
                return;
            }
        }
    }

    fn declare_one(&mut self, code: &mut Code, scope: NodeId) -> Result<()> {
        let start = code.pos;
        let opc = opcode(code)?;
        let scoped = match opc {
            op::SCOPE => Some((None, 0)),
            op::DEVICE => Some((Some(Object::Device), 0)),
            op::PROCESSOR => Some((Some(Object::Processor), 6)),
            op::POWER_RES => Some((Some(Object::PowerResource), 3)),
            op::THERMAL_ZONE => Some((Some(Object::ThermalZone), 0)),
            _ => None,
        };
        if let Some((object, fixed)) = scoped {
            let end = code.pkg_length()?;
            let name = code.name()?;
            code.skip(fixed)?;
            let node = match object {
                None => self.ns.lookup_exact(scope, &name),
                Some(object) => {
                    let (parent, seg) = self.ns.parent_for(scope, &name)?;
                    match self.ns.child(parent, &seg) {
                        Some(n) => Some(n),
                        None => Some(self.ns.add(parent, seg, object)?),
                    }
                }
            };
            if let Some(node) = node {
                self.declare(code.until(end), node);
            }
            code.pos = end;
            return Ok(());
        }
        match opc {
            op::METHOD => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                let flags = code.byte()?;
                let method = self.method_object(code, end, flags);
                let (parent, seg) = self.ns.parent_for(scope, &name)?;
                match self.ns.child(parent, &seg) {
                    None => {
                        self.ns.add(parent, seg, method)?;
                    }
                    Some(n) if matches!(self.ns.object(n), Some(Object::External(_))) => {
                        if let Some(o) = self.ns.object_mut(n) {
                            *o = method;
                        }
                    }
                    Some(_) => {}
                }
                code.pos = end;
            }
            op::EXTERNAL => {
                let name = code.name()?;
                let _kind = code.byte()?;
                let args = code.byte()?;
                let (parent, seg) = self.ns.parent_for(scope, &name)?;
                if self.ns.child(parent, &seg).is_none() {
                    self.ns.add(parent, seg, Object::External(args & 7))?;
                }
            }
            _ => {
                code.pos = start;
                self.skip_term(code, scope)?;
            }
        }
        Ok(())
    }

    fn method_object(&self, code: &Code, end: usize, flags: u8) -> Object {
        Object::Method(Rc::new(Method {
            code: code.until(end),
            args: flags & 7,
            serialized: flags & 8 != 0,
        }))
    }

    // ---- Lists of terms ----

    /// Runs terms until `end`, or until one breaks, continues or returns.
    pub(crate) fn term_list(
        &mut self,
        code: &mut Code,
        end: usize,
        frame: &mut Frame,
    ) -> Result<Flow> {
        while code.pos < end {
            let start = code.pos;
            match self.term(code, frame) {
                Ok(Flow::Next) => {}
                Ok(flow) => return Ok(flow),
                Err(e) if frame.loading() && !matches!(e, Error::Limit(_)) => {
                    // A table's top level carries on past a term that fails, as ACPICA does,
                    // if it can step over it.
                    let path = self.ns.path(frame.scope);
                    self.host.log(false, format_args!("loading, in {path}: {e}"));
                    code.pos = start;
                    self.skip_term(code, frame.scope)?;
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Flow::Next)
    }

    /// Runs one term: a named object, a statement, or an expression whose value is dropped.
    fn term(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Flow> {
        self.tick()?;
        let opc = peek_opcode(code)?;
        match opc {
            op::ALIAS
            | op::NAME
            | op::SCOPE
            | op::METHOD
            | op::EXTERNAL
            | op::MUTEX
            | op::EVENT
            | op::REGION
            | op::FIELD
            | op::DEVICE
            | op::PROCESSOR
            | op::POWER_RES
            | op::THERMAL_ZONE
            | op::INDEX_FIELD
            | op::BANK_FIELD
            | op::DATA_REGION
            | op::CREATE_BIT_FIELD
            | op::CREATE_BYTE_FIELD
            | op::CREATE_WORD_FIELD
            | op::CREATE_DWORD_FIELD
            | op::CREATE_QWORD_FIELD
            | op::CREATE_FIELD => {
                opcode(code)?;
                self.nested(|s| s.named(opc, code, frame))?;
                Ok(Flow::Next)
            }
            op::IF
            | op::WHILE
            | op::BREAK
            | op::CONTINUE
            | op::RETURN
            | op::NOOP
            | op::BREAK_POINT
            | op::NOTIFY
            | op::SLEEP
            | op::STALL
            | op::SIGNAL
            | op::RESET
            | op::RELEASE
            | op::FATAL
            | op::LOAD
            | op::UNLOAD
            | op::ELSE => {
                opcode(code)?;
                self.nested(|s| s.statement(opc, code, frame))
            }
            _ => {
                self.term_arg(code, frame)?;
                Ok(Flow::Next)
            }
        }
    }

    fn statement(&mut self, opc: u16, code: &mut Code, frame: &mut Frame) -> Result<Flow> {
        match opc {
            op::IF => {
                let end = code.pkg_length()?;
                let yes = self.predicate(code, frame)?;
                let flow = if yes { self.term_list(code, end, frame)? } else { Flow::Next };
                code.pos = end;
                // An `Else` belongs to the `If` just before it.
                if !code.done() && code.peek()? == op::ELSE as u8 {
                    code.pos += 1;
                    let else_end = code.pkg_length()?;
                    if !yes {
                        let flow = self.term_list(code, else_end, frame)?;
                        code.pos = else_end;
                        return Ok(flow);
                    }
                    code.pos = else_end;
                }
                Ok(flow)
            }
            op::ELSE => {
                // An `Else` without its `If`: stepped over.
                let end = code.pkg_length()?;
                code.pos = end;
                Ok(Flow::Next)
            }
            op::WHILE => {
                let end = code.pkg_length()?;
                let top = code.pos;
                loop {
                    code.pos = top;
                    self.tick()?;
                    if !self.predicate(code, frame)? {
                        break;
                    }
                    match self.term_list(code, end, frame)? {
                        Flow::Next | Flow::Continue => {}
                        Flow::Break => break,
                        flow @ Flow::Return(_) => {
                            code.pos = end;
                            return Ok(flow);
                        }
                    }
                }
                code.pos = end;
                Ok(Flow::Next)
            }
            op::BREAK => Ok(Flow::Break),
            op::CONTINUE => Ok(Flow::Continue),
            op::RETURN => {
                let v = self.term_arg(code, frame)?;
                Ok(Flow::Return(v))
            }
            op::NOOP | op::BREAK_POINT => Ok(Flow::Next),
            op::NOTIFY => {
                let target = self.super_name(code, frame)?;
                let value = self.integer_arg(code, frame)?;
                let path = match target {
                    Target::Node(n) => self.ns.path(n),
                    _ => String::from("an object"),
                };
                self.host.notify(&path, value);
                Ok(Flow::Next)
            }
            op::SLEEP => {
                let ms = self.integer_arg(code, frame)?;
                let now = self.host.now();
                if now.saturating_add(ms.saturating_mul(1_000_000)) > self.run.deadline {
                    return Err(Error::Limit("time"));
                }
                self.host.sleep(ms);
                Ok(Flow::Next)
            }
            op::STALL => {
                let us = self.integer_arg(code, frame)?;
                let now = self.host.now();
                if now.saturating_add(us.saturating_mul(1000)) > self.run.deadline {
                    return Err(Error::Limit("time"));
                }
                self.host.stall(us);
                Ok(Flow::Next)
            }
            op::SIGNAL | op::RESET => {
                let event = self.event(code, frame)?;
                if let Some(Object::Event(count)) = self.ns.object_mut(event) {
                    *count = if opc == op::SIGNAL { count.saturating_add(1) } else { 0 };
                }
                Ok(Flow::Next)
            }
            op::RELEASE => {
                let m = self.mutex(code, frame)?;
                self.release(m)?;
                Ok(Flow::Next)
            }
            op::FATAL => {
                let kind = code.byte()?;
                let c = code.int(4)? as u32;
                let arg = self.integer_arg(code, frame)?;
                self.host
                    .log(false, format_args!("the firmware's Fatal({kind:#x}, {c:#x}, {arg:#x})"));
                Err(Error::Fatal(kind, c, arg))
            }
            op::LOAD => Err(Error::NotSupported("Load")),
            op::UNLOAD => Err(Error::NotSupported("Unload")),
            _ => Err(Error::Malformed("unknown statement")),
        }
    }

    /// An `If`'s or `While`'s condition: an integer, true if not zero.
    fn predicate(&mut self, code: &mut Code, frame: &mut Frame) -> Result<bool> {
        Ok(self.integer_arg(code, frame)? != 0)
    }

    /// A term argument converted to an integer.
    pub(crate) fn integer_arg(&mut self, code: &mut Code, frame: &mut Frame) -> Result<u64> {
        let v = self.term_arg(code, frame)?;
        self.integer(&v)
    }

    // ---- Named objects ----

    /// Creates `name` in the frame's scope. A name declared in the first pass, or by
    /// `External`, is taken over; otherwise one already there is an error.
    fn create(&mut self, frame: &mut Frame, name: &AmlName, object: Object) -> Result<NodeId> {
        let (parent, seg) = self.ns.parent_for(frame.scope, name)?;
        if let Some(existing) = self.ns.child(parent, &seg) {
            let takeover = match (self.ns.object(existing), &object) {
                (Some(Object::External(_)), _) => true,
                (Some(Object::Method(_)), Object::Method(_)) => frame.loading(),
                (Some(Object::Device), Object::Device)
                | (Some(Object::Processor), Object::Processor)
                | (Some(Object::PowerResource), Object::PowerResource)
                | (Some(Object::ThermalZone), Object::ThermalZone) => frame.loading(),
                _ => false,
            };
            if !takeover {
                return Err(Error::AlreadyExists(self.ns.path(existing)));
            }
            if let Some(o) = self.ns.object_mut(existing) {
                *o = object;
            }
            return Ok(existing);
        }
        let node = self.ns.add(parent, seg, object)?;
        if let Some(created) = &mut frame.created {
            created.push(node);
        }
        Ok(node)
    }

    fn named(&mut self, opc: u16, code: &mut Code, frame: &mut Frame) -> Result<()> {
        match opc {
            op::ALIAS => {
                let source = code.name()?;
                let alias = code.name()?;
                let to = self
                    .ns
                    .lookup(frame.scope, &source)
                    .ok_or_else(|| Error::NotFound(format!("{source}")))?;
                self.create(frame, &alias, Object::Alias(to))?;
            }
            op::NAME => {
                let name = code.name()?;
                let v = self.data_object(code, frame)?;
                self.create(frame, &name, Object::Data(v))?;
            }
            op::SCOPE => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                let node = self
                    .ns
                    .lookup_exact(frame.scope, &name)
                    .ok_or_else(|| Error::NotFound(format!("Scope({name})")))?;
                self.body(code, end, frame, node)?;
            }
            op::DEVICE | op::THERMAL_ZONE => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                let object = if opc == op::DEVICE { Object::Device } else { Object::ThermalZone };
                let node = self.create(frame, &name, object)?;
                self.body(code, end, frame, node)?;
            }
            op::PROCESSOR => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                code.skip(6)?; // its number, and its old register block's address and length
                let node = self.create(frame, &name, Object::Processor)?;
                self.body(code, end, frame, node)?;
            }
            op::POWER_RES => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                code.skip(3)?; // its system level and resource order
                let node = self.create(frame, &name, Object::PowerResource)?;
                self.body(code, end, frame, node)?;
            }
            op::METHOD => {
                let end = code.pkg_length()?;
                let name = code.name()?;
                let flags = code.byte()?;
                let method = self.method_object(code, end, flags);
                code.pos = end;
                self.create(frame, &name, method)?;
            }
            op::EXTERNAL => {
                let name = code.name()?;
                let _kind = code.byte()?;
                let args = code.byte()?;
                let (parent, seg) = self.ns.parent_for(frame.scope, &name)?;
                if self.ns.child(parent, &seg).is_none() {
                    self.create(frame, &name, Object::External(args & 7))?;
                }
            }
            op::MUTEX => {
                let name = code.name()?;
                let sync = code.byte()? & 0x0F;
                self.create(frame, &name, Object::Mutex(MutexState { sync, depth: 0 }))?;
            }
            op::EVENT => {
                let name = code.name()?;
                self.create(frame, &name, Object::Event(0))?;
            }
            op::REGION => {
                let name = code.name()?;
                let space = code.byte()?;
                let place = if frame.loading() {
                    // Worked out at first use: step over the two arguments for now.
                    let from = code.pos;
                    self.skip_term(code, frame.scope)?;
                    self.skip_term(code, frame.scope)?;
                    Place::Later {
                        code: Code::new(code.bytes.clone(), from, code.pos),
                        scope: frame.scope,
                    }
                } else {
                    let offset = self.integer_arg(code, frame)?;
                    let length = self.integer_arg(code, frame)?;
                    Place::Known { offset, length }
                };
                let region = Region { space, place, pci: None };
                self.create(frame, &name, Object::Region(Rc::new(RefCell::new(region))))?;
            }
            op::FIELD => {
                let end = code.pkg_length()?;
                let region = self.existing(code, frame)?;
                let flags = code.byte()?;
                self.field_list(code, end, frame, flags, || Reach::Region(region))?;
            }
            op::INDEX_FIELD => {
                let end = code.pkg_length()?;
                let index = self.existing(code, frame)?;
                let data = self.existing(code, frame)?;
                let flags = code.byte()?;
                self.field_list(code, end, frame, flags, || Reach::Index { index, data })?;
            }
            op::BANK_FIELD => {
                let end = code.pkg_length()?;
                let region = self.existing(code, frame)?;
                let bank = self.existing(code, frame)?;
                let value = self.integer_arg(code, frame)?;
                let flags = code.byte()?;
                self.field_list(code, end, frame, flags, || Reach::Bank { region, bank, value })?;
            }
            op::DATA_REGION => return Err(Error::NotSupported("DataTableRegion")),
            op::CREATE_BIT_FIELD
            | op::CREATE_BYTE_FIELD
            | op::CREATE_WORD_FIELD
            | op::CREATE_DWORD_FIELD
            | op::CREATE_QWORD_FIELD
            | op::CREATE_FIELD => {
                let source = self.term_arg(code, frame)?;
                let Value::Buffer(buffer) = self.deref_element(&source)? else {
                    return Err(Error::WrongType("a buffer field's source isn't a buffer"));
                };
                let index = self.integer_arg(code, frame)? as usize;
                let (bit, bits) = match opc {
                    op::CREATE_BIT_FIELD => (index, 1),
                    op::CREATE_BYTE_FIELD => (index.saturating_mul(8), 8),
                    op::CREATE_WORD_FIELD => (index.saturating_mul(8), 16),
                    op::CREATE_DWORD_FIELD => (index.saturating_mul(8), 32),
                    op::CREATE_QWORD_FIELD => (index.saturating_mul(8), 64),
                    _ => (index, self.integer_arg(code, frame)? as usize),
                };
                let name = code.name()?;
                if bits == 0 || bit.checked_add(bits).is_none_or(|e| e > buffer.borrow().len() * 8)
                {
                    return Err(Error::OutOfRange);
                }
                self.create(
                    frame,
                    &name,
                    Object::BufferField(BufferField {
                        buffer,
                        bit,
                        bits,
                        always_buffer: opc == op::CREATE_FIELD,
                    }),
                )?;
            }
            _ => return Err(Error::Malformed("unknown named object")),
        }
        Ok(())
    }

    /// Runs the body of a scope, device and the like in that node's scope.
    fn body(&mut self, code: &mut Code, end: usize, frame: &mut Frame, node: NodeId) -> Result<()> {
        let outer = core::mem::replace(&mut frame.scope, node);
        let r = self.term_list(code, end, frame);
        frame.scope = outer;
        code.pos = end;
        match r? {
            Flow::Next => Ok(()),
            _ => Err(Error::Malformed("a break or return outside a method")),
        }
    }

    /// A name that must already be there (a field's region, an index field's registers).
    fn existing(&mut self, code: &mut Code, frame: &Frame) -> Result<NodeId> {
        let name = code.name()?;
        self.ns
            .lookup(frame.scope, &name)
            .map(|n| self.ns.through_alias(n))
            .ok_or_else(|| Error::NotFound(format!("{name}")))
    }

    /// A field list (ACPI 6.5, section 20.2.5.2): named fields, reserved gaps and changes of
    /// access type, each placed after the one before.
    fn field_list(
        &mut self,
        code: &mut Code,
        end: usize,
        frame: &mut Frame,
        flags: u8,
        reach: impl Fn() -> Reach,
    ) -> Result<()> {
        let width = |t: u8| match t & 0x0F {
            1 => 1,
            2 => 2,
            3 => 4,
            4 => 8,
            5 => 1, // "buffer" access: by bytes here
            _ => 0,
        };
        let mut access = width(flags);
        let lock = flags & 0x10 != 0;
        let update = (flags >> 5) & 3;
        let mut bit: u64 = 0;
        while code.pos < end {
            self.tick()?;
            match code.peek()? {
                0x00 => {
                    code.pos += 1;
                    bit = bit.saturating_add(self.field_length(code)?);
                }
                0x01 => {
                    code.pos += 1;
                    access = width(code.byte()?);
                    code.byte()?; // the access attribute, for buses M7 doesn't reach
                }
                0x02 => {
                    // A connection, for GPIO and serial buses (not reached in M7): a name or a
                    // buffer.
                    code.pos += 1;
                    if code.peek()? == op::BUFFER as u8 {
                        code.pos += 1;
                        code.pos = code.pkg_length()?;
                    } else {
                        code.name()?;
                    }
                }
                0x03 => {
                    code.pos += 1;
                    access = width(code.byte()?);
                    code.skip(2)?;
                }
                _ => {
                    let seg = code.take(4)?;
                    let seg = [seg[0], seg[1], seg[2], seg[3]];
                    let bits = self.field_length(code)?;
                    let unit = FieldUnit { reach: reach(), bit, bits, access, lock, update };
                    let name = AmlName { absolute: false, parents: 0, segs: alloc::vec![seg] };
                    self.create(frame, &name, Object::Field(Rc::new(unit)))?;
                    bit = bit.saturating_add(bits);
                }
            }
        }
        code.pos = end;
        Ok(())
    }

    /// A field's length in bits, written as a package length (which here doesn't count
    /// itself).
    fn field_length(&mut self, code: &mut Code) -> Result<u64> {
        let lead = code.byte()?;
        let follow = usize::from(lead >> 6);
        if follow == 0 {
            return Ok(u64::from(lead & 0x3F));
        }
        let mut length = u64::from(lead & 0x0F);
        for i in 0..follow {
            length |= u64::from(code.byte()?) << (4 + 8 * i);
        }
        Ok(length)
    }

    /// Works out a deferred region's offset and length.
    pub(crate) fn region_arguments(&mut self, mut code: Code, scope: NodeId) -> Result<(u64, u64)> {
        let mut frame = Frame::load(scope);
        let offset = self.nested(|s| s.integer_arg(&mut code, &mut frame))?;
        let length = self.nested(|s| s.integer_arg(&mut code, &mut frame))?;
        Ok((offset, length))
    }

    /// What a `Name` holds: data, a package or buffer, or a name (kept as a reference).
    fn data_object(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Value> {
        if starts_name(code.peek()?) {
            return self.name_reference(code, frame);
        }
        self.term_arg(code, frame)
    }

    /// A name inside a package or a `Name`: a reference to it, looked up now if it is there,
    /// or when used if it isn't yet.
    fn name_reference(&mut self, code: &mut Code, frame: &Frame) -> Result<Value> {
        let name = code.name()?;
        Ok(Value::Reference(Reference(match self.ns.lookup(frame.scope, &name) {
            Some(n) => Ref::Node(n),
            None => Ref::Name(frame.scope, name),
        })))
    }

    // ---- Methods ----

    /// Calls a method, or reads an object that isn't one.
    pub(crate) fn call_or_read(&mut self, node: NodeId, args: &[Value]) -> Result<Value> {
        let node = self.ns.through_alias(node);
        let method = match self.ns.object(node) {
            Some(Object::Method(m)) => m.clone(),
            Some(Object::BuiltIn(BuiltIn::Osi)) => {
                let name = match args.first() {
                    Some(v) => self.text(v, ToText::Implicit)?,
                    None => String::new(),
                };
                let yes = OSI_YES.contains(&name.as_str());
                if self.verbose {
                    self.host.log(
                        true,
                        format_args!("_OSI({name:?}): {}", if yes { "yes" } else { "no" }),
                    );
                }
                return Ok(Value::Integer(if yes { self.ones() } else { 0 }));
            }
            Some(Object::External(_)) | None => return Err(Error::NotFound(self.ns.path(node))),
            Some(_) => return self.read_node(node),
        };
        if self.run.calls >= limits::CALL_DEPTH {
            return Err(Error::Limit("method calls nested too deeply"));
        }
        self.run.calls += 1;
        let mut frame = Frame::method(node, args);
        let mut code = method.code.clone();
        let end = code.end;
        let result = self.nested(|s| s.term_list(&mut code, end, &mut frame));
        self.run.calls -= 1;
        // The names the method made go with it, newest first.
        for n in frame.created.take().unwrap_or_default().into_iter().rev() {
            self.ns.remove(n);
        }
        match result? {
            Flow::Return(v) => Ok(v),
            Flow::Next => Ok(Value::Uninitialized),
            _ => Err(Error::Malformed("a break or continue outside a loop")),
        }
    }

    /// An object's value, without calling it if it is a method: data, or what a field reads;
    /// anything else is a reference to itself.
    pub(crate) fn read_node(&mut self, node: NodeId) -> Result<Value> {
        let node = self.ns.through_alias(node);
        match self.ns.object(node) {
            Some(Object::Data(v)) => Ok(v.clone()),
            Some(Object::Field(f)) => {
                let f = f.clone();
                self.read_field(node, &f)
            }
            Some(Object::BufferField(f)) => {
                let f = f.clone();
                self.read_buffer_field(&f)
            }
            Some(Object::External(_)) | None => Err(Error::NotFound(self.ns.path(node))),
            Some(_) => Ok(Value::Reference(Reference(Ref::Node(node)))),
        }
    }

    /// A name used as a term: a method is called, with as many arguments as it takes read
    /// after it; anything else is read.
    fn name_term(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Value> {
        let name = code.name()?;
        let Some(node) = self.ns.lookup(frame.scope, &name) else {
            return Err(Error::NotFound(format!("{name} (from {})", self.ns.path(frame.scope))));
        };
        let node = self.ns.through_alias(node);
        let count = match self.ns.object(node) {
            Some(Object::Method(m)) => m.args,
            Some(Object::BuiltIn(BuiltIn::Osi)) => 1,
            Some(Object::External(n)) => *n,
            _ => 0,
        };
        let mut args = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            args.push(self.term_arg(code, frame)?);
        }
        self.call_or_read(node, &args)
    }

    /// The result of an evaluation as the caller gets it: references into packages and
    /// buffers become the values there.
    pub(crate) fn resolved(&mut self, v: Value) -> Result<Value> {
        self.deref_element(&v)
    }

    // ---- Targets and stores ----

    /// A `Target`: `NullName` (a zero byte) or a `SuperName`.
    pub(crate) fn target(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Target> {
        if code.peek()? == 0 {
            code.pos += 1;
            return Ok(Target::None);
        }
        self.super_name(code, frame)
    }

    /// A `SuperName`: a local, an argument, a name, the debug object, or what `RefOf`,
    /// `DerefOf` or `Index` point at. A name is not called here, even if it is a method.
    pub(crate) fn super_name(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Target> {
        let opc = peek_opcode(code)?;
        match opc {
            op::LOCAL0..=op::LOCAL7 => {
                code.pos += 1;
                Ok(Target::Local(usize::from(opc - op::LOCAL0)))
            }
            op::ARG0..=op::ARG6 => {
                code.pos += 1;
                Ok(Target::Arg(usize::from(opc - op::ARG0)))
            }
            op::DEBUG => {
                code.pos += 2;
                Ok(Target::Debug)
            }
            op::DEREF_OF => {
                code.pos += 1;
                let v = self.term_arg(code, frame)?;
                match v {
                    Value::Reference(Reference(r)) => Ok(Target::Ref(r)),
                    Value::String(s) => self.path_target(&s, frame),
                    _ => Err(Error::WrongType("DerefOf of something that isn't a reference")),
                }
            }
            _ if starts_name(code.peek()?) => {
                let name = code.name()?;
                match self.ns.lookup(frame.scope, &name) {
                    Some(n) => Ok(Target::Node(self.ns.through_alias(n))),
                    None => Err(Error::NotFound(format!("{name}"))),
                }
            }
            _ => match self.term_arg(code, frame)? {
                Value::Reference(Reference(r)) => Ok(Target::Ref(r)),
                _ => Err(Error::WrongType("a store to something that isn't a place")),
            },
        }
    }

    /// The node a path written as a string names.
    fn path_target(&mut self, s: &str, frame: &Frame) -> Result<Target> {
        let name = AmlName::parse(s).ok_or(Error::WrongType("a string that isn't a name"))?;
        let n =
            self.ns.lookup(frame.scope, &name).ok_or_else(|| Error::NotFound(String::from(s)))?;
        Ok(Target::Node(n))
    }

    /// Stores `v` to `t`, as `Store` and every operator with a target does.
    pub(crate) fn store(&mut self, t: &Target, v: &Value, frame: &mut Frame) -> Result<()> {
        match t {
            Target::None => Ok(()),
            Target::Local(i) => {
                let copy = self.copy(v)?;
                *frame
                    .locals
                    .get(*i)
                    .ok_or(Error::Malformed("a local outside a method"))?
                    .borrow_mut() = copy;
                Ok(())
            }
            Target::Arg(i) => {
                let cell = frame
                    .args
                    .get(*i)
                    .ok_or(Error::Malformed("an argument outside a method"))?
                    .clone();
                // An argument holding a reference is stored through.
                let held = cell.borrow().clone();
                if let Value::Reference(Reference(r)) = held {
                    return self.store_ref(&r, v);
                }
                let copy = self.copy(v)?;
                *cell.borrow_mut() = copy;
                Ok(())
            }
            Target::Node(n) => self.store_node(*n, v.clone()),
            Target::Debug => {
                self.host.log(true, format_args!("Debug: {v:?}"));
                Ok(())
            }
            Target::Ref(r) => self.store_ref(r, v),
        }
    }

    /// Stores through a reference.
    pub(crate) fn store_ref(&mut self, r: &Ref, v: &Value) -> Result<()> {
        match r {
            Ref::Node(n) => self.store_node(*n, v.clone()),
            Ref::Name(scope, name) => {
                let n = self
                    .ns
                    .lookup(*scope, name)
                    .ok_or_else(|| Error::NotFound(format!("{name}")))?;
                self.store_node(n, v.clone())
            }
            Ref::Element(p, i) => {
                let copy = self.copy(v)?;
                let mut p = p.borrow_mut();
                *p.0.get_mut(*i).ok_or(Error::OutOfRange)? = copy;
                Ok(())
            }
            Ref::Byte(b, i) => {
                let x = self.integer(v)? as u8;
                *b.borrow_mut().get_mut(*i).ok_or(Error::OutOfRange)? = x;
                Ok(())
            }
            Ref::Cell(c) => {
                let copy = self.copy(v)?;
                *c.borrow_mut() = copy;
                Ok(())
            }
        }
    }

    /// Stores to a named object, converting to the kind it already holds (ACPI 6.5, section
    /// 19.3.5.8): an integer stays an integer, a string a string, and a buffer keeps its
    /// length; a package, or nothing yet, takes a copy of whatever comes; a field writes.
    pub(crate) fn store_node(&mut self, node: NodeId, v: Value) -> Result<()> {
        let node = self.ns.through_alias(node);
        let new = match self.ns.object(node) {
            Some(Object::Data(Value::Integer(_))) => Value::Integer(self.integer(&v)?),
            Some(Object::Data(Value::String(_))) => {
                let s = self.text(&v, ToText::Implicit)?;
                self.new_string(s)?
            }
            Some(Object::Data(Value::Buffer(b))) => {
                let b = b.clone();
                let src = self.bytes(&v)?;
                let mut dst = b.borrow_mut();
                let n = dst.len();
                dst.iter_mut().for_each(|x| *x = 0);
                for (d, s) in dst.iter_mut().zip(src.iter().take(n)) {
                    *d = *s;
                }
                return Ok(());
            }
            Some(Object::Data(_)) => {
                let v = self.deref_element(&v)?;
                self.copy(&v)?
            }
            Some(Object::Field(f)) => {
                let f = f.clone();
                return self.write_field(node, &f, &v);
            }
            Some(Object::BufferField(f)) => {
                let f = f.clone();
                return self.write_buffer_field(&f, &v);
            }
            Some(Object::External(_)) | None => return Err(Error::NotFound(self.ns.path(node))),
            Some(_) => return Err(Error::WrongType("a store to an object that isn't data")),
        };
        if let Some(o) = self.ns.object_mut(node) {
            *o = Object::Data(new);
        }
        Ok(())
    }

    /// The value at a target, for the operators that change one in place (`Increment`) or
    /// look at it (`SizeOf`, `ObjectType`).
    pub(crate) fn read_target(&mut self, t: &Target, frame: &Frame) -> Result<Value> {
        match t {
            Target::Local(i) => {
                Ok(frame.locals.get(*i).map(|c| c.borrow().clone()).unwrap_or_default())
            }
            Target::Arg(i) => {
                let v = frame.args.get(*i).map(|c| c.borrow().clone()).unwrap_or_default();
                match v {
                    Value::Reference(Reference(r)) => self.read_ref(&r),
                    v => Ok(v),
                }
            }
            Target::Node(n) => self.read_node(*n),
            Target::Ref(r) => self.read_ref(r),
            Target::None | Target::Debug => Err(Error::WrongType("nothing to read")),
        }
    }

    /// The value a reference points at.
    pub(crate) fn read_ref(&mut self, r: &Ref) -> Result<Value> {
        match r {
            Ref::Node(n) => self.read_node(*n),
            Ref::Name(scope, name) => {
                let n = self
                    .ns
                    .lookup(*scope, name)
                    .ok_or_else(|| Error::NotFound(format!("{name}")))?;
                self.read_node(n)
            }
            Ref::Cell(c) => Ok(c.borrow().clone()),
            element => self.deref_element(&Value::Reference(Reference(element.clone()))),
        }
    }

    // ---- Synchronisation ----

    fn event(&mut self, code: &mut Code, frame: &mut Frame) -> Result<NodeId> {
        match self.super_name(code, frame)? {
            Target::Node(n) if matches!(self.ns.object(n), Some(Object::Event(_))) => Ok(n),
            _ => Err(Error::WrongType("not an event")),
        }
    }

    pub(crate) fn mutex(&mut self, code: &mut Code, frame: &mut Frame) -> Result<NodeId> {
        match self.super_name(code, frame)? {
            Target::Node(n) if matches!(self.ns.object(n), Some(Object::Mutex(_))) => Ok(n),
            _ => Err(Error::WrongType("not a mutex")),
        }
    }

    /// True if `m` is the global lock, `\_GL`.
    fn is_global(&self, m: NodeId) -> bool {
        self.ns.parent(m) == Some(self.ns.root()) && self.ns.name(m) == *b"_GL_"
    }

    /// `Acquire`: true if it timed out. Only the global lock can be held by anyone else (the
    /// firmware's hidden handler); it is tried every millisecond until `timeout` milliseconds
    /// have passed (`0xFFFF`: until the evaluation's time runs out).
    pub(crate) fn acquire(&mut self, m: NodeId, timeout: u16) -> Result<bool> {
        if self.is_global(m) {
            return Ok(!self.acquire_global(timeout)?);
        }
        if let Some(Object::Mutex(state)) = self.ns.object_mut(m) {
            state.depth = state.depth.saturating_add(1);
            self.run.mutexes.push(m);
        }
        Ok(false)
    }

    pub(crate) fn release(&mut self, m: NodeId) -> Result<()> {
        if self.is_global(m) {
            if self.run.global_lock == 0 {
                return Err(Error::BadState("releasing the global lock, which isn't held"));
            }
            self.release_global();
            return Ok(());
        }
        match self.ns.object_mut(m) {
            Some(Object::Mutex(state)) if state.depth > 0 => {
                state.depth -= 1;
                Ok(())
            }
            _ => Err(Error::BadState("releasing a mutex that isn't held")),
        }
    }

    /// Takes the global lock (or counts it again if already held); false if it timed out.
    pub(crate) fn acquire_global(&mut self, timeout: u16) -> Result<bool> {
        if self.run.global_lock > 0 {
            self.run.global_lock += 1;
            return Ok(true);
        }
        let mut waited = 0u32;
        loop {
            if self.host.acquire_global_lock() {
                self.run.global_lock = 1;
                return Ok(true);
            }
            if timeout != 0xFFFF && waited >= u32::from(timeout) {
                return Ok(false);
            }
            if self.host.now() > self.run.deadline {
                return Err(Error::Limit("time, waiting for the global lock"));
            }
            self.host.sleep(1);
            waited += 1;
        }
    }

    pub(crate) fn release_global(&mut self) {
        if self.run.global_lock > 0 {
            self.run.global_lock -= 1;
            if self.run.global_lock == 0 {
                self.host.release_global_lock();
            }
        }
    }

    /// `Wait`: true if it timed out. Nothing else runs while an evaluation does, so an event
    /// not signalled already never will be; the wait is the timeout's.
    pub(crate) fn wait(&mut self, event: NodeId, timeout: u64) -> Result<bool> {
        if let Some(Object::Event(count)) = self.ns.object_mut(event)
            && *count > 0
        {
            *count -= 1;
            return Ok(false);
        }
        let ms =
            if timeout >= 0xFFFF { return Err(Error::Limit("waiting for ever")) } else { timeout };
        if self.host.now().saturating_add(ms * 1_000_000) > self.run.deadline {
            return Err(Error::Limit("time"));
        }
        self.host.sleep(ms);
        Ok(true)
    }

    // ---- Terms with values ----

    /// A term argument: data, a local or argument, a name (a method is called), or an
    /// operator's result.
    pub(crate) fn term_arg(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Value> {
        self.nested(|s| {
            s.tick()?;
            let b = code.peek()?;
            if starts_name(b) {
                return s.name_term(code, frame);
            }
            let opc = opcode(code)?;
            s.operator(opc, code, frame)
        })
    }

    /// A `Package` or `VarPackage`'s elements: data, or names (kept as references).
    pub(crate) fn package(
        &mut self,
        code: &mut Code,
        frame: &mut Frame,
        var: bool,
    ) -> Result<Value> {
        let end = code.pkg_length()?;
        let count = if var {
            let mut inner = code.until(end);
            let n = self.integer_arg(&mut inner, frame)?;
            code.pos = inner.pos;
            n
        } else {
            u64::from(code.byte()?)
        };
        let count = usize::try_from(count)
            .ok()
            .filter(|&n| n <= limits::OBJECT)
            .ok_or(Error::Limit("a package's length"))?;
        let mut elements = Vec::new();
        let mut inner = code.until(end);
        while !inner.done() {
            let v = if starts_name(inner.peek()?) {
                self.name_reference(&mut inner, frame)?
            } else {
                self.term_arg(&mut inner, frame)?
            };
            if elements.len() < count {
                elements.push(v);
            }
        }
        elements.resize(count, Value::Uninitialized);
        code.pos = end;
        self.new_package(elements)
    }

    /// A `Buffer`: its size, then its first bytes; the rest are zero.
    pub(crate) fn buffer(&mut self, code: &mut Code, frame: &mut Frame) -> Result<Value> {
        let end = code.pkg_length()?;
        let mut inner = code.until(end);
        let size = self.integer_arg(&mut inner, frame)?;
        let size = usize::try_from(size)
            .ok()
            .filter(|&n| n <= limits::OBJECT)
            .ok_or(Error::Limit("a buffer's length"))?;
        let init = &inner.bytes[inner.pos..end];
        let mut bytes = init.to_vec();
        bytes.resize(size.max(init.len()), 0);
        code.pos = end;
        self.new_buffer(bytes)
    }
}
