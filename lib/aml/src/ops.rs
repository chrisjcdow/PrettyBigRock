// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The operators that give a value (ACPI 6.5, section 19.6): constants, locals and arguments,
//! arithmetic and logic, conversions, and the operators on strings, buffers, packages and
//! references.
//!
//! Integer arithmetic wraps around, as AML's does, and every result is kept within the
//! integer width (32 bits for an old DSDT). True is all ones, false zero.

use crate::code::Code;
use crate::exec::{Frame, Target};
use crate::namespace::Object;
use crate::op;
use crate::value::{Ref, Reference, ToText, Value, parse_explicit};
use crate::{Aml, Error, Host, Result};
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

impl<H: Host> Aml<H> {
    /// The value of the operator `opc`, already read.
    pub(crate) fn operator(
        &mut self,
        opc: u16,
        code: &mut Code,
        frame: &mut Frame,
    ) -> Result<Value> {
        let ones = self.ones();
        match opc {
            op::ZERO => Ok(Value::Integer(0)),
            op::ONE => Ok(Value::Integer(1)),
            op::ONES => Ok(Value::Integer(ones)),
            op::BYTE => Ok(Value::Integer(code.int(1)?)),
            op::WORD => Ok(Value::Integer(code.int(2)?)),
            op::DWORD => Ok(Value::Integer(code.int(4)?)),
            op::QWORD => Ok(Value::Integer(code.int(8)? & ones)),
            op::STRING => {
                let s = code.ascii()?;
                self.new_string(s)
            }
            op::REVISION => Ok(Value::Integer(2)),
            op::BUFFER => self.buffer(code, frame),
            op::PACKAGE => self.package(code, frame, false),
            op::VAR_PACKAGE => self.package(code, frame, true),
            op::LOCAL0..=op::LOCAL7 => {
                let i = usize::from(opc - op::LOCAL0);
                let cell =
                    frame.locals.get(i).ok_or(Error::Malformed("a local outside a method"))?;
                Ok(cell.borrow().clone())
            }
            op::ARG0..=op::ARG6 => {
                let i = usize::from(opc - op::ARG0);
                let cell =
                    frame.args.get(i).ok_or(Error::Malformed("an argument outside a method"))?;
                Ok(cell.borrow().clone())
            }
            op::DEBUG => Err(Error::WrongType("the debug object has no value")),
            op::TIMER => {
                // In units of 100 nanoseconds.
                Ok(Value::Integer((self.host.now() / 100) & ones))
            }

            op::STORE => {
                let v = self.term_arg(code, frame)?;
                let t = self.super_name(code, frame)?;
                self.store(&t, &v, frame)?;
                Ok(v)
            }
            op::COPY_OBJECT => {
                let v = self.term_arg(code, frame)?;
                let t = self.super_name(code, frame)?;
                self.copy_object(&t, &v, frame)?;
                Ok(v)
            }

            op::ADD
            | op::SUBTRACT
            | op::MULTIPLY
            | op::AND
            | op::NAND
            | op::OR
            | op::NOR
            | op::XOR
            | op::SHIFT_LEFT
            | op::SHIFT_RIGHT
            | op::MOD => {
                let a = self.integer_arg(code, frame)?;
                let b = self.integer_arg(code, frame)?;
                let r = match opc {
                    op::ADD => a.wrapping_add(b),
                    op::SUBTRACT => a.wrapping_sub(b),
                    op::MULTIPLY => a.wrapping_mul(b),
                    op::AND => a & b,
                    op::NAND => !(a & b),
                    op::OR => a | b,
                    op::NOR => !(a | b),
                    op::XOR => a ^ b,
                    op::SHIFT_LEFT => {
                        if b >= 64 {
                            0
                        } else {
                            a << b
                        }
                    }
                    op::SHIFT_RIGHT => {
                        if b >= 64 {
                            0
                        } else {
                            a >> b
                        }
                    }
                    _ => a.checked_rem(b).ok_or(Error::DivideByZero)?,
                };
                self.result(Value::Integer(r & ones), code, frame)
            }
            op::DIVIDE => {
                let a = self.integer_arg(code, frame)?;
                let b = self.integer_arg(code, frame)?;
                let quotient = a.checked_div(b).ok_or(Error::DivideByZero)?;
                let remainder = a % b;
                let rt = self.target(code, frame)?;
                let qt = self.target(code, frame)?;
                self.store(&rt, &Value::Integer(remainder), frame)?;
                self.store(&qt, &Value::Integer(quotient), frame)?;
                Ok(Value::Integer(quotient))
            }
            op::NOT => {
                let a = self.integer_arg(code, frame)?;
                self.result(Value::Integer(!a & ones), code, frame)
            }
            op::FIND_SET_LEFT_BIT | op::FIND_SET_RIGHT_BIT => {
                let a = self.integer_arg(code, frame)?;
                let r = if a == 0 {
                    0
                } else if opc == op::FIND_SET_LEFT_BIT {
                    64 - u64::from(a.leading_zeros())
                } else {
                    u64::from(a.trailing_zeros()) + 1
                };
                self.result(Value::Integer(r), code, frame)
            }
            op::FROM_BCD => {
                let mut a = self.integer_arg(code, frame)?;
                let mut r = 0u64;
                let mut scale = 1u64;
                while a != 0 {
                    let digit = a & 0xF;
                    if digit > 9 {
                        return Err(Error::WrongType("FromBCD of a digit that isn't decimal"));
                    }
                    r = r.wrapping_add(digit.wrapping_mul(scale));
                    scale = scale.wrapping_mul(10);
                    a >>= 4;
                }
                self.result(Value::Integer(r & ones), code, frame)
            }
            op::TO_BCD => {
                let mut a = self.integer_arg(code, frame)?;
                let mut r = 0u64;
                let mut shift = 0;
                while a != 0 && shift < 64 {
                    r |= (a % 10) << shift;
                    a /= 10;
                    shift += 4;
                }
                self.result(Value::Integer(r & ones), code, frame)
            }
            op::INCREMENT | op::DECREMENT => {
                let t = self.super_name(code, frame)?;
                let v = self.read_target(&t, frame)?;
                let v = self.integer(&v)?;
                let r =
                    if opc == op::INCREMENT { v.wrapping_add(1) } else { v.wrapping_sub(1) } & ones;
                self.store(&t, &Value::Integer(r), frame)?;
                Ok(Value::Integer(r))
            }

            op::LAND | op::LOR => {
                let a = self.integer_arg(code, frame)?;
                let b = self.integer_arg(code, frame)?;
                let r = if opc == op::LAND { a != 0 && b != 0 } else { a != 0 || b != 0 };
                Ok(self.boolean(r))
            }
            op::LNOT => {
                let a = self.integer_arg(code, frame)?;
                Ok(self.boolean(a == 0))
            }
            op::LEQUAL | op::LGREATER | op::LLESS => {
                let a = self.term_arg(code, frame)?;
                let b = self.term_arg(code, frame)?;
                let order = self.compare(&a, &b)?;
                let r = match opc {
                    op::LEQUAL => order.is_eq(),
                    op::LGREATER => order.is_gt(),
                    _ => order.is_lt(),
                };
                Ok(self.boolean(r))
            }

            op::CONCAT => {
                let a = self.term_arg(code, frame)?;
                let b = self.term_arg(code, frame)?;
                let r = self.concatenate(&a, &b)?;
                self.result(r, code, frame)
            }
            op::CONCAT_RES => {
                let a = self.term_arg(code, frame)?;
                let b = self.term_arg(code, frame)?;
                let a = self.bytes(&a)?;
                let b = self.bytes(&b)?;
                // Each template's end tag (and checksum) is dropped, and one put back at the
                // end, with a checksum of zero (meaning "not checked").
                let mut out = strip_end_tag(&a).to_vec();
                out.extend_from_slice(strip_end_tag(&b));
                out.extend_from_slice(&[0x79, 0x00]);
                let r = self.new_buffer(out)?;
                self.result(r, code, frame)
            }
            op::TO_BUFFER => {
                let a = self.term_arg(code, frame)?;
                let bytes = match self.deref_element(&a)? {
                    // An empty string makes an empty buffer.
                    Value::String(s) if s.is_empty() => Vec::new(),
                    other => self.bytes(&other)?,
                };
                let r = self.new_buffer(bytes)?;
                self.result(r, code, frame)
            }
            op::TO_DECIMAL_STRING | op::TO_HEX_STRING => {
                let a = self.term_arg(code, frame)?;
                let how = if opc == op::TO_DECIMAL_STRING { ToText::Decimal } else { ToText::Hex };
                let s = self.text(&a, how)?;
                let r = self.new_string(s)?;
                self.result(r, code, frame)
            }
            op::TO_INTEGER => {
                let a = self.term_arg(code, frame)?;
                let r = match self.deref_element(&a)? {
                    Value::String(s) => parse_explicit(&s) & ones,
                    other => self.integer(&other)?,
                };
                self.result(Value::Integer(r), code, frame)
            }
            op::TO_STRING => {
                let a = self.term_arg(code, frame)?;
                let length = self.integer_arg(code, frame)?;
                let bytes = self.bytes(&a)?;
                let s: String = bytes
                    .iter()
                    .take(usize::try_from(length).unwrap_or(usize::MAX))
                    .take_while(|&&b| b != 0)
                    .map(|&b| if b < 0x80 { char::from(b) } else { '?' })
                    .collect();
                let r = self.new_string(s)?;
                self.result(r, code, frame)
            }
            op::MID => {
                let a = self.term_arg(code, frame)?;
                let index = self.integer_arg(code, frame)?;
                let length = self.integer_arg(code, frame)?;
                let a = self.deref_element(&a)?;
                let slice = |len: usize| {
                    let from = usize::try_from(index).unwrap_or(usize::MAX).min(len);
                    let to =
                        from.saturating_add(usize::try_from(length).unwrap_or(usize::MAX)).min(len);
                    (from, to)
                };
                let r = match a {
                    Value::String(s) => {
                        let (from, to) = slice(s.len());
                        self.new_string(String::from(s.get(from..to).unwrap_or("")))?
                    }
                    Value::Buffer(b) => {
                        let bytes = b.borrow();
                        let (from, to) = slice(bytes.len());
                        let part = bytes[from..to].to_vec();
                        drop(bytes);
                        self.new_buffer(part)?
                    }
                    _ => {
                        return Err(Error::WrongType(
                            "Mid of something that isn't a string or buffer",
                        ));
                    }
                };
                self.result(r, code, frame)
            }

            op::SIZE_OF => {
                let t = self.super_name(code, frame)?;
                let v = self.read_target(&t, frame)?;
                let v = self.deref_element(&v)?;
                let n = match v {
                    Value::String(s) => s.len(),
                    Value::Buffer(b) => b.borrow().len(),
                    Value::Package(p) => p.borrow().0.len(),
                    _ => return Err(Error::WrongType("SizeOf of something without a size")),
                };
                Ok(Value::Integer(n as u64))
            }
            op::OBJECT_TYPE => {
                let t = self.super_name(code, frame)?;
                let kind = match &t {
                    Target::Node(n) => self.ns.object(*n).map(Object::kind).unwrap_or(0),
                    Target::Debug => 16,
                    t => {
                        let v = self.read_target(t, frame)?;
                        let v = self.deref_element(&v)?;
                        match v {
                            Value::Reference(Reference(Ref::Node(n))) => {
                                self.ns.object(n).map(Object::kind).unwrap_or(0)
                            }
                            // A name in a package, looked up now.
                            Value::Reference(Reference(Ref::Name(scope, name))) => self
                                .ns
                                .lookup(scope, &name)
                                .and_then(|n| self.ns.object(n))
                                .map(Object::kind)
                                .unwrap_or(0),
                            v => v.kind(),
                        }
                    }
                };
                Ok(Value::Integer(kind))
            }
            op::REF_OF => {
                let t = self.super_name(code, frame)?;
                self.reference_to(t, frame)
            }
            op::COND_REF_OF => {
                // Like `RefOf`, but a name that isn't there gives false instead of failing.
                let start = code.pos;
                let t = match self.super_name(code, frame) {
                    Ok(t) => t,
                    Err(Error::NotFound(_)) => {
                        // Step over the name and the target; nothing is stored.
                        code.pos = start;
                        code.name()?;
                        self.target(code, frame)?;
                        return Ok(Value::Integer(0));
                    }
                    Err(e) => return Err(e),
                };
                let dest = self.target(code, frame)?;
                let r = self.reference_to(t, frame)?;
                self.store(&dest, &r, frame)?;
                Ok(Value::Integer(ones))
            }
            op::DEREF_OF => {
                let v = self.term_arg(code, frame)?;
                match v {
                    Value::Reference(Reference(r)) => self.deref_value(&r),
                    Value::String(s) => {
                        let name = crate::AmlName::parse(&s)
                            .ok_or(Error::WrongType("DerefOf a string that isn't a name"))?;
                        let n = self.ns.lookup(frame.scope, &name).ok_or(Error::NotFound(s))?;
                        self.read_node(n)
                    }
                    _ => Err(Error::WrongType("DerefOf of something that isn't a reference")),
                }
            }
            op::INDEX => {
                let source = self.term_arg(code, frame)?;
                let i = self.integer_arg(code, frame)?;
                let i = usize::try_from(i).map_err(|_| Error::OutOfRange)?;
                let r = match self.deref_element(&source)? {
                    Value::Package(p) => {
                        if i >= p.borrow().0.len() {
                            return Err(Error::OutOfRange);
                        }
                        Ref::Element(p, i)
                    }
                    Value::Buffer(b) => {
                        if i >= b.borrow().len() {
                            return Err(Error::OutOfRange);
                        }
                        Ref::Byte(b, i)
                    }
                    Value::String(s) => {
                        // A reference to a copy of the string's bytes: reading works, writing
                        // changes only the copy.
                        if i >= s.len() {
                            return Err(Error::OutOfRange);
                        }
                        self.charge(s.len() + 32)?;
                        Ref::Byte(Rc::new(RefCell::new(s.into_bytes())), i)
                    }
                    _ => {
                        return Err(Error::WrongType(
                            "Index of something that isn't a package, buffer or string",
                        ));
                    }
                };
                let v = Value::Reference(Reference(r));
                let t = self.target(code, frame)?;
                self.store_index_target(&t, &v, frame)?;
                Ok(v)
            }
            op::MATCH => {
                let p = self.term_arg(code, frame)?;
                let op1 = code.byte()?;
                let v1 = self.integer_arg(code, frame)?;
                let op2 = code.byte()?;
                let v2 = self.integer_arg(code, frame)?;
                let start = self.integer_arg(code, frame)?;
                let Value::Package(p) = self.deref_element(&p)? else {
                    return Err(Error::WrongType("Match of something that isn't a package"));
                };
                let elements = p.borrow().0.clone();
                let start = usize::try_from(start).unwrap_or(usize::MAX);
                for (i, e) in elements.iter().enumerate().skip(start) {
                    self.tick()?;
                    // Elements that aren't integers (or can't become one) never match.
                    let Ok(x) = self.integer(e) else { continue };
                    if matches(op1, x, v1) && matches(op2, x, v2) {
                        return Ok(Value::Integer(i as u64));
                    }
                }
                Ok(Value::Integer(ones))
            }

            op::ACQUIRE => {
                let m = self.mutex(code, frame)?;
                let timeout = code.int(2)? as u16;
                let timed_out = self.acquire(m, timeout)?;
                Ok(self.boolean(timed_out))
            }
            op::WAIT => {
                let target = self.super_name(code, frame)?;
                let Target::Node(event) = target else {
                    return Err(Error::WrongType("not an event"));
                };
                if !matches!(self.ns.object(event), Some(Object::Event(_))) {
                    return Err(Error::WrongType("not an event"));
                }
                let timeout = self.integer_arg(code, frame)?;
                let timed_out = self.wait(event, timeout)?;
                Ok(self.boolean(timed_out))
            }
            op::LOAD_TABLE => Err(Error::NotSupported("LoadTable")),
            _ => Err(Error::Malformed("unknown opcode")),
        }
    }

    /// `DerefOf` a reference, as ACPICA does it: a package element that names an object is
    /// followed to that object's value; and an object that isn't data (a device, a method) is
    /// no value at all.
    fn deref_value(&mut self, r: &Ref) -> Result<Value> {
        let mut v = self.read_ref(r)?;
        if let (
            Ref::Element(..),
            Value::Reference(Reference(named @ (Ref::Node(_) | Ref::Name(..)))),
        ) = (r, &v)
        {
            let named = named.clone();
            v = self.read_ref(&named)?;
        }
        if matches!(v, Value::Reference(Reference(Ref::Node(_)))) {
            return Err(Error::WrongType("DerefOf of an object that isn't data"));
        }
        Ok(v)
    }

    /// True or false as AML writes them: all ones, or zero.
    fn boolean(&self, b: bool) -> Value {
        Value::Integer(if b { self.ones() } else { 0 })
    }

    /// Reads an operator's target and stores its result there; the result is also the
    /// operator's value.
    fn result(&mut self, v: Value, code: &mut Code, frame: &mut Frame) -> Result<Value> {
        let t = self.target(code, frame)?;
        self.store(&t, &v, frame)?;
        Ok(v)
    }

    /// `Index`'s target takes the reference itself, not a copy of what it points at.
    fn store_index_target(&mut self, t: &Target, v: &Value, frame: &mut Frame) -> Result<()> {
        match t {
            Target::Local(i) => {
                *frame
                    .locals
                    .get(*i)
                    .ok_or(Error::Malformed("a local outside a method"))?
                    .borrow_mut() = v.clone();
                Ok(())
            }
            t => self.store(t, v, frame),
        }
    }

    /// `CopyObject`: like `Store`, but a named object takes the value as it is instead of
    /// converting it to the kind it held.
    fn copy_object(&mut self, t: &Target, v: &Value, frame: &mut Frame) -> Result<()> {
        match t {
            Target::Node(n) => {
                let n = self.ns.through_alias(*n);
                match self.ns.object(n) {
                    Some(Object::Field(_) | Object::BufferField(_)) => {
                        self.store_node(n, v.clone())
                    }
                    _ => {
                        let copy = self.copy(v)?;
                        if let Some(o) = self.ns.object_mut(n) {
                            *o = Object::Data(copy);
                        }
                        Ok(())
                    }
                }
            }
            Target::Arg(i) => {
                let copy = self.copy(v)?;
                *frame
                    .args
                    .get(*i)
                    .ok_or(Error::Malformed("an argument outside a method"))?
                    .borrow_mut() = copy;
                Ok(())
            }
            t => self.store(t, v, frame),
        }
    }

    /// `RefOf`: a reference to what a `SuperName` names.
    fn reference_to(&mut self, t: Target, frame: &Frame) -> Result<Value> {
        let r = match t {
            Target::Node(n) => Ref::Node(n),
            Target::Local(i) => Ref::Cell(
                frame.locals.get(i).ok_or(Error::Malformed("a local outside a method"))?.clone(),
            ),
            Target::Arg(i) => {
                let cell = frame
                    .args
                    .get(i)
                    .ok_or(Error::Malformed("an argument outside a method"))?
                    .clone();
                // An argument that is already a reference gives that reference.
                if let Value::Reference(r) = &*cell.borrow() {
                    return Ok(Value::Reference(r.clone()));
                }
                Ref::Cell(cell)
            }
            Target::Ref(r) => r,
            Target::None | Target::Debug => return Err(Error::WrongType("RefOf of nothing")),
        };
        Ok(Value::Reference(Reference(r)))
    }

    /// Compares two values as `LEqual` and its kin do (ACPI 6.5, section 19.6.70): as the
    /// first's kind, the second converted to it. Strings and buffers compare byte by byte,
    /// then by length.
    fn compare(&mut self, a: &Value, b: &Value) -> Result<core::cmp::Ordering> {
        let a = self.deref_element(a)?;
        match a {
            Value::Integer(x) => {
                let y = self.integer(b)?;
                Ok(x.cmp(&y))
            }
            Value::String(x) => {
                let y = self.text(b, ToText::Implicit)?;
                Ok(x.as_bytes().cmp(y.as_bytes()))
            }
            Value::Buffer(x) => {
                let y = self.bytes(b)?;
                Ok(x.borrow().as_slice().cmp(y.as_slice()))
            }
            _ => {
                Err(Error::WrongType("comparing something that isn't an integer, string or buffer"))
            }
        }
    }

    /// `Concatenate` (ACPI 6.5, section 19.6.12): the result is the first operand's kind; two
    /// integers make a buffer of both.
    fn concatenate(&mut self, a: &Value, b: &Value) -> Result<Value> {
        let a = self.deref_element(a)?;
        match a {
            Value::Integer(_) => {
                let mut bytes = self.bytes(&a)?;
                let y = self.integer(b)?;
                bytes.extend_from_slice(&self.bytes(&Value::Integer(y))?);
                self.new_buffer(bytes)
            }
            Value::Buffer(x) => {
                let mut bytes = x.borrow().clone();
                bytes.extend_from_slice(&self.bytes(b)?);
                self.new_buffer(bytes)
            }
            other => {
                let mut s = self.text(&other, ToText::Implicit)?;
                s.push_str(&self.text(b, ToText::Implicit)?);
                self.new_string(s)
            }
        }
    }
}

/// `Match`'s comparisons: `x` against `v`.
fn matches(opcode: u8, x: u64, v: u64) -> bool {
    match opcode {
        0 => true,
        1 => x == v,
        2 => x <= v,
        3 => x < v,
        4 => x >= v,
        5 => x > v,
        _ => false,
    }
}

/// A resource template without its end tag: everything before the first end-tag descriptor,
/// found by walking the descriptors by their lengths.
fn strip_end_tag(t: &[u8]) -> &[u8] {
    let mut i = 0;
    while i < t.len() {
        let b = t[i];
        let len = if b & 0x80 != 0 {
            let Some(l) = t.get(i + 1..i + 3) else { break };
            3 + usize::from(u16::from_le_bytes([l[0], l[1]]))
        } else {
            if b & 0x78 == 0x78 {
                return &t[..i];
            }
            1 + usize::from(b & 7)
        };
        i += len;
    }
    &t[..i.min(t.len())]
}
