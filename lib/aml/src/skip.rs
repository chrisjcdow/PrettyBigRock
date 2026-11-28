// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Stepping over bytecode without running it, by the shape of each opcode's arguments.
//!
//! The first pass of loading uses it to get past everything but declarations, a region
//! declared as a table loads uses it to step over its arguments until they're needed, and a
//! table's top level uses it to carry on past a term that failed. AML has no length on most
//! terms, so stepping over one means reading its whole grammar, and a method call's arguments
//! can only be counted by looking the method up (a name not found is taken to have none, as
//! ACPICA does).

use crate::code::{Code, starts_name};
use crate::exec::{opcode, peek_opcode};
use crate::namespace::{BuiltIn, NodeId, Object};
use crate::op;
use crate::{Aml, Error, Host, Result};

/// What an opcode is followed by.
#[derive(Clone, Copy)]
enum Arg {
    /// A term argument.
    Term,
    /// A `Target`: a `SuperName` or `NullName`.
    Target,
    /// A `SuperName`.
    Super,
    /// A name.
    Name,
    /// A name, then data or a name (`Name`'s value).
    Data,
    /// Bytes of a fixed number.
    Bytes(usize),
    /// A null-terminated string.
    Ascii,
}

use Arg::*;

/// The arguments of each opcode without a package length.
fn shape(opc: u16) -> Option<&'static [Arg]> {
    Some(match opc {
        op::ZERO
        | op::ONE
        | op::ONES
        | op::REVISION
        | op::DEBUG
        | op::TIMER
        | op::BREAK
        | op::CONTINUE
        | op::NOOP
        | op::BREAK_POINT => &[],
        op::LOCAL0..=op::LOCAL7 | op::ARG0..=op::ARG6 => &[],
        op::BYTE => &[Bytes(1)],
        op::WORD => &[Bytes(2)],
        op::DWORD => &[Bytes(4)],
        op::QWORD => &[Bytes(8)],
        op::STRING => &[Ascii],
        op::ALIAS => &[Name, Name],
        op::NAME => &[Name, Data],
        op::EXTERNAL => &[Name, Bytes(2)],
        op::MUTEX => &[Name, Bytes(1)],
        op::EVENT => &[Name],
        op::REGION => &[Name, Bytes(1), Term, Term],
        op::DATA_REGION => &[Name, Term, Term, Term],
        op::CREATE_BIT_FIELD
        | op::CREATE_BYTE_FIELD
        | op::CREATE_WORD_FIELD
        | op::CREATE_DWORD_FIELD
        | op::CREATE_QWORD_FIELD => &[Term, Term, Name],
        op::CREATE_FIELD => &[Term, Term, Term, Name],
        op::RETURN | op::SLEEP | op::STALL | op::DEREF_OF | op::LNOT => &[Term],
        op::NOTIFY => &[Super, Term],
        op::SIGNAL
        | op::RESET
        | op::RELEASE
        | op::UNLOAD
        | op::INCREMENT
        | op::DECREMENT
        | op::SIZE_OF
        | op::OBJECT_TYPE
        | op::REF_OF => &[Super],
        op::FATAL => &[Bytes(5), Term],
        op::LOAD => &[Name, Super],
        op::STORE | op::COPY_OBJECT => &[Term, Super],
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
        | op::MOD
        | op::CONCAT
        | op::CONCAT_RES
        | op::INDEX
        | op::TO_STRING => &[Term, Term, Target],
        op::DIVIDE => &[Term, Term, Target, Target],
        op::NOT
        | op::FIND_SET_LEFT_BIT
        | op::FIND_SET_RIGHT_BIT
        | op::FROM_BCD
        | op::TO_BCD
        | op::TO_BUFFER
        | op::TO_DECIMAL_STRING
        | op::TO_HEX_STRING
        | op::TO_INTEGER => &[Term, Target],
        op::MID => &[Term, Term, Term, Target],
        op::COND_REF_OF => &[Super, Target],
        op::MATCH => &[Term, Bytes(1), Term, Bytes(1), Term, Term],
        op::LAND | op::LOR | op::LEQUAL | op::LGREATER | op::LLESS => &[Term, Term],
        op::ACQUIRE => &[Super, Bytes(2)],
        op::WAIT => &[Super, Term],
        op::LOAD_TABLE => &[Term, Term, Term, Term, Term, Term],
        _ => return None,
    })
}

/// True for the opcodes that start with a package length: stepping over one is jumping to
/// its end.
fn has_pkg_length(opc: u16) -> bool {
    matches!(
        opc,
        op::SCOPE
            | op::BUFFER
            | op::PACKAGE
            | op::VAR_PACKAGE
            | op::METHOD
            | op::IF
            | op::ELSE
            | op::WHILE
            | op::DEVICE
            | op::PROCESSOR
            | op::POWER_RES
            | op::THERMAL_ZONE
            | op::FIELD
            | op::INDEX_FIELD
            | op::BANK_FIELD
    )
}

impl<H: Host> Aml<H> {
    /// Steps over one term (or term argument) in `scope`.
    pub(crate) fn skip_term(&mut self, code: &mut Code, scope: NodeId) -> Result<()> {
        self.nested(|s| {
            s.tick()?;
            if starts_name(code.peek()?) {
                return s.skip_call(code, scope);
            }
            let opc = opcode(code)?;
            if has_pkg_length(opc) {
                code.pos = code.pkg_length()?;
                return Ok(());
            }
            let args = shape(opc).ok_or(Error::Malformed("unknown opcode"))?;
            for &arg in args {
                match arg {
                    Term => s.skip_term(code, scope)?,
                    Target if code.peek()? == 0 => code.pos += 1,
                    Target | Super => s.skip_super(code, scope)?,
                    Name => {
                        code.name()?;
                    }
                    Data if starts_name(code.peek()?) => {
                        code.name()?;
                    }
                    Data => s.skip_term(code, scope)?,
                    Bytes(n) => code.skip(n)?,
                    Ascii => {
                        code.ascii()?;
                    }
                }
            }
            Ok(())
        })
    }

    /// Steps over a name and, if it is a method, its arguments.
    fn skip_call(&mut self, code: &mut Code, scope: NodeId) -> Result<()> {
        let name = code.name()?;
        let count = match self.ns.lookup(scope, &name).map(|n| self.ns.through_alias(n)) {
            Some(n) => match self.ns.object(n) {
                Some(Object::Method(m)) => m.args,
                Some(Object::BuiltIn(BuiltIn::Osi)) => 1,
                Some(Object::External(a)) => *a,
                _ => 0,
            },
            None => 0,
        };
        for _ in 0..count {
            self.skip_term(code, scope)?;
        }
        Ok(())
    }

    /// Steps over a `SuperName`: a name (never called here), a local or argument, the debug
    /// object, or `RefOf`, `DerefOf` and `Index`.
    fn skip_super(&mut self, code: &mut Code, scope: NodeId) -> Result<()> {
        if starts_name(code.peek()?) {
            code.name()?;
            return Ok(());
        }
        match peek_opcode(code)? {
            op::LOCAL0..=op::LOCAL7 | op::ARG0..=op::ARG6 => code.pos += 1,
            op::DEBUG => code.pos += 2,
            _ => self.skip_term(code, scope)?,
        }
        Ok(())
    }
}
