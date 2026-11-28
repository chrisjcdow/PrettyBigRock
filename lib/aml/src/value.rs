// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The data AML computes with, and the conversions between its kinds (ACPI 6.5, section
//! 19.3.5).
//!
//! Integers and strings are copied as they are passed around. Buffers and packages are
//! shared: `Index(PKG, 2)` gives a reference into the very package `PKG` names, and storing
//! through it changes that package. So they are kept behind `Rc<RefCell<…>>`: `Rc` lets
//! several owners share one object, and `RefCell` lets it be changed through a shared
//! pointer, checking at run time that no one else is changing it at the same moment. Storing
//! one into a variable or a name copies it, as the specification says, so sharing only
//! reaches as far as AML expects.

use crate::namespace::NodeId;
use crate::{Aml, Error, Host, Result, limits};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;
use core::fmt;

/// A value.
#[derive(Clone, Default)]
pub enum Value {
    /// Nothing yet: a local not stored to, a method that returned nothing.
    #[default]
    Uninitialized,
    /// An integer, 64 bits (or 32, for an old DSDT).
    Integer(u64),
    /// A string of ASCII characters.
    String(String),
    /// Bytes.
    Buffer(Rc<RefCell<Vec<u8>>>),
    /// A list of values.
    Package(Rc<RefCell<Elements>>),
    /// A reference to an object (`RefOf`, `Index`, a name in a package).
    Reference(Reference),
}

/// A package's elements. Its own `Drop` frees nested packages one after another instead of
/// one inside the other, so a package nested a million deep (which a loop in the firmware
/// could build) can't run the stack out as it is freed.
#[derive(Default)]
pub struct Elements(pub(crate) Vec<Value>);

impl Drop for Elements {
    fn drop(&mut self) {
        let mut stack = core::mem::take(&mut self.0);
        while let Some(v) = stack.pop() {
            match v {
                Value::Package(rc) | Value::Reference(Reference(Ref::Element(rc, _))) => {
                    if let Ok(cell) = Rc::try_unwrap(rc) {
                        stack.append(&mut cell.into_inner().0);
                    }
                }
                Value::Reference(Reference(Ref::Cell(rc))) => {
                    if let Ok(cell) = Rc::try_unwrap(rc) {
                        stack.push(cell.into_inner());
                    }
                }
                _ => {}
            }
        }
    }
}

/// What a reference points at.
#[derive(Clone)]
pub struct Reference(pub(crate) Ref);

#[derive(Clone)]
pub(crate) enum Ref {
    /// A named object.
    Node(NodeId),
    /// A name in a package that wasn't there yet when the package was made: looked up when
    /// used, from the scope the package was made in (firmware often names a device defined
    /// further on, in a routing table).
    Name(NodeId, crate::code::AmlName),
    /// An element of a package.
    Element(Rc<RefCell<Elements>>, usize),
    /// A byte of a buffer.
    Byte(Rc<RefCell<Vec<u8>>>, usize),
    /// A method's local or argument (`RefOf(Local0)`).
    Cell(Rc<RefCell<Value>>),
}

/// A field made over a buffer by `CreateField` and its kin: bits of the buffer, read and
/// written as one value.
#[derive(Clone)]
pub(crate) struct BufferField {
    pub buffer: Rc<RefCell<Vec<u8>>>,
    pub bit: usize,
    pub bits: usize,
}

impl Value {
    /// A buffer holding `bytes`.
    pub fn buffer(bytes: Vec<u8>) -> Value {
        Value::Buffer(Rc::new(RefCell::new(bytes)))
    }

    /// A package of `values`.
    pub fn package(values: Vec<Value>) -> Value {
        Value::Package(Rc::new(RefCell::new(Elements(values))))
    }

    /// The integer, if it is one.
    pub fn as_integer(&self) -> Option<u64> {
        match self {
            Value::Integer(i) => Some(*i),
            _ => None,
        }
    }

    /// The string, if it is one.
    pub fn as_string(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// A copy of the buffer's bytes, if it is one.
    pub fn as_buffer(&self) -> Option<Vec<u8>> {
        match self {
            Value::Buffer(b) => Some(b.borrow().clone()),
            _ => None,
        }
    }

    /// The package's elements, if it is one (shared, not copied, for buffers and packages
    /// inside it).
    pub fn as_package(&self) -> Option<Vec<Value>> {
        match self {
            Value::Package(p) => Some(p.borrow().0.clone()),
            _ => None,
        }
    }

    /// The node a reference names, if it is a reference to a named object.
    pub fn as_node(&self) -> Option<NodeId> {
        match self {
            Value::Reference(Reference(Ref::Node(n))) => Some(*n),
            _ => None,
        }
    }

    /// The number `ObjectType` gives this kind of value.
    pub fn kind(&self) -> u64 {
        match self {
            Value::Uninitialized => 0,
            Value::Integer(_) => 1,
            Value::String(_) => 2,
            Value::Buffer(_) => 3,
            Value::Package(_) => 4,
            Value::Reference(_) => 20,
        }
    }

    /// The name of its kind, for messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Value::Uninitialized => "nothing",
            Value::Integer(_) => "an integer",
            Value::String(_) => "a string",
            Value::Buffer(_) => "a buffer",
            Value::Package(_) => "a package",
            Value::Reference(_) => "a reference",
        }
    }

    /// Writes the value for a message or a test's expected answers, at most `depth` packages
    /// deep.
    fn write(&self, f: &mut fmt::Formatter<'_>, depth: u32) -> fmt::Result {
        match self {
            Value::Uninitialized => f.write_str("Uninitialized"),
            Value::Integer(i) => write!(f, "{i:#x}"),
            Value::String(s) => write!(f, "{s:?}"),
            Value::Buffer(b) => {
                f.write_str("Buffer {")?;
                for (i, byte) in b.borrow().iter().enumerate() {
                    write!(f, "{}{byte:02x}", if i == 0 { " " } else { ", " })?;
                }
                f.write_str(" }")
            }
            Value::Package(p) if depth == 0 => write!(f, "Package {{ {} … }}", p.borrow().0.len()),
            Value::Package(p) => {
                f.write_str("Package {")?;
                for (i, e) in p.borrow().0.iter().enumerate() {
                    f.write_str(if i == 0 { " " } else { ", " })?;
                    e.write(f, depth - 1)?;
                }
                f.write_str(" }")
            }
            Value::Reference(Reference(r)) => match r {
                Ref::Node(_) => f.write_str("Reference"),
                Ref::Name(_, name) => write!(f, "Reference {name}"),
                Ref::Element(_, i) => write!(f, "Reference [{i}]"),
                Ref::Byte(_, i) => write!(f, "Reference byte [{i}]"),
                Ref::Cell(_) => f.write_str("Reference local"),
            },
        }
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.write(f, 8)
    }
}

/// How a value is turned into a string (ACPI 6.5, section 19.3.5.7; the forms ACPICA uses).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToText {
    /// Converting without being asked (`Concatenate`, storing to a string).
    Implicit,
    /// `ToHexString`.
    Hex,
    /// `ToDecimalString`.
    Decimal,
}

impl<H: Host> Aml<H> {
    /// The mask integers are kept within.
    pub(crate) fn ones(&self) -> u64 {
        if self.wide { u64::MAX } else { u64::from(u32::MAX) }
    }

    /// Counts `bytes` of new objects against the evaluation's budget.
    pub(crate) fn charge(&mut self, bytes: usize) -> Result<()> {
        if bytes > limits::OBJECT * 16 {
            return Err(Error::Limit("an object's size"));
        }
        self.run.allocated = self.run.allocated.saturating_add(bytes);
        if self.run.allocated > limits::MEMORY {
            return Err(Error::Limit("memory"));
        }
        Ok(())
    }

    /// A new buffer, counted against the budget.
    pub(crate) fn new_buffer(&mut self, bytes: Vec<u8>) -> Result<Value> {
        if bytes.len() > limits::OBJECT {
            return Err(Error::Limit("a buffer's length"));
        }
        self.charge(bytes.len() + 32)?;
        Ok(Value::buffer(bytes))
    }

    /// A new string, counted against the budget.
    pub(crate) fn new_string(&mut self, s: String) -> Result<Value> {
        if s.len() > limits::OBJECT {
            return Err(Error::Limit("a string's length"));
        }
        self.charge(s.len() + 32)?;
        Ok(Value::String(s))
    }

    /// A new package, counted against the budget.
    pub(crate) fn new_package(&mut self, values: Vec<Value>) -> Result<Value> {
        if values.len() > limits::OBJECT {
            return Err(Error::Limit("a package's length"));
        }
        self.charge(values.len() * 32 + 32)?;
        Ok(Value::package(values))
    }

    /// Follows a reference into a package or buffer to the value there; anything else is
    /// itself. Operands that want a value get one this way, as ACPICA does.
    pub(crate) fn deref_element(&mut self, v: &Value) -> Result<Value> {
        match v {
            Value::Reference(Reference(Ref::Element(p, i))) => {
                p.borrow().0.get(*i).cloned().ok_or(Error::OutOfRange)
            }
            Value::Reference(Reference(Ref::Byte(b, i))) => {
                b.borrow().get(*i).map(|&x| Value::Integer(u64::from(x))).ok_or(Error::OutOfRange)
            }
            _ => Ok(v.clone()),
        }
    }

    /// The value as an integer: a string's leading hexadecimal digits, a buffer's first 8 (or
    /// 4) bytes, little-endian.
    pub(crate) fn integer(&mut self, v: &Value) -> Result<u64> {
        let v = self.deref_element(v)?;
        let ones = self.ones();
        match v {
            Value::Integer(i) => Ok(i & ones),
            Value::String(s) => Ok(parse_hex(s.trim_start()).0 & ones),
            Value::Buffer(b) => {
                let b = b.borrow();
                let n = if self.wide { 8 } else { 4 };
                Ok(b.iter().take(n).rev().fold(0u64, |acc, &x| (acc << 8) | u64::from(x)))
            }
            other => Err(Error::WrongType(match other {
                Value::Uninitialized => "an integer was needed, but there was nothing",
                Value::Package(_) => "an integer was needed, not a package",
                _ => "an integer was needed, not a reference",
            })),
        }
    }

    /// The value's bytes, as a buffer conversion gives them: an integer's 8 (or 4)
    /// little-endian bytes, a string's characters and the zero after them.
    pub(crate) fn bytes(&mut self, v: &Value) -> Result<Vec<u8>> {
        let v = self.deref_element(v)?;
        match v {
            Value::Integer(i) => {
                let n = if self.wide { 8 } else { 4 };
                Ok(i.to_le_bytes()[..n].to_vec())
            }
            Value::String(s) => {
                let mut b = s.into_bytes();
                b.push(0);
                Ok(b)
            }
            Value::Buffer(b) => Ok(b.borrow().clone()),
            _ => Err(Error::WrongType("a buffer was needed")),
        }
    }

    /// The value as text.
    pub(crate) fn text(&mut self, v: &Value, how: ToText) -> Result<String> {
        use core::fmt::Write;
        let v = self.deref_element(v)?;
        let mut s = String::new();
        match v {
            Value::Integer(i) if how == ToText::Decimal => {
                let _ = write!(s, "{i}");
            }
            Value::Integer(i) => {
                let digits = if self.wide { 16 } else { 8 };
                let _ = write!(s, "{:0digits$X}", i & self.ones());
            }
            Value::String(t) => s = t,
            Value::Buffer(b) => {
                let sep = if how == ToText::Implicit { " " } else { "," };
                for (n, x) in b.borrow().iter().enumerate() {
                    if n > 0 {
                        s.push_str(sep);
                    }
                    let _ = match how {
                        ToText::Decimal => write!(s, "{x}"),
                        _ => write!(s, "0x{x:02X}"),
                    };
                }
            }
            Value::Package(_) => s.push_str("[Package]"),
            Value::Reference(_) => s.push_str("[Reference]"),
            Value::Uninitialized => return Err(Error::WrongType("a string was needed")),
        }
        if s.len() > limits::OBJECT {
            return Err(Error::Limit("a string's length"));
        }
        Ok(s)
    }

    /// A copy of `v` that shares nothing with it: what storing to a name or a variable makes
    /// (references are copied as references). At most 64 packages deep.
    pub(crate) fn copy(&mut self, v: &Value) -> Result<Value> {
        self.copy_at(v, 0)
    }

    fn copy_at(&mut self, v: &Value, depth: u32) -> Result<Value> {
        if depth > 64 {
            return Err(Error::Limit("packages nested too deeply"));
        }
        match v {
            Value::Buffer(b) => {
                let bytes = b.borrow().clone();
                self.new_buffer(bytes)
            }
            Value::Package(p) => {
                let old = p.borrow().0.clone();
                let mut new = Vec::with_capacity(old.len());
                for e in &old {
                    new.push(self.copy_at(e, depth + 1)?);
                }
                self.new_package(new)
            }
            other => Ok(other.clone()),
        }
    }
}

/// Reads leading hexadecimal digits: the value, and how many characters were read. It stops
/// at a digit that would overflow 64 bits, as ACPICA does.
pub(crate) fn parse_hex(s: &str) -> (u64, usize) {
    let mut v = 0u64;
    let mut n = 0;
    for c in s.chars() {
        let Some(d) = c.to_digit(16) else { break };
        if v >> 60 != 0 {
            break;
        }
        v = (v << 4) | u64::from(d);
        n += 1;
    }
    (v, n)
}

/// What `ToInteger` reads from a string: hexadecimal after `0x`, otherwise decimal.
pub(crate) fn parse_explicit(s: &str) -> u64 {
    let s = s.trim_start();
    if let Some(hex) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        return parse_hex(hex).0;
    }
    let mut v = 0u64;
    for c in s.chars() {
        let Some(d) = c.to_digit(10) else { break };
        match v.checked_mul(10).and_then(|v| v.checked_add(u64::from(d))) {
            Some(next) => v = next,
            None => break,
        }
    }
    v
}
