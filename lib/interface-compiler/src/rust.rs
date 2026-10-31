// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Generating Rust from a checked interface (docs/design/0008-messages-and-interfaces.md,
//! section 9).
//!
//! For an interface `pbr.file` the output is one module, `pbr_file`, holding:
//! - its types, constants, `VERSION`, `NAME`, and the largest buffers (`REQUEST_BUFFER`,
//!   `REPLY_BUFFER`) a service and a client need;
//! - a module per operation with its numbers, its `Request` (and `Results` when there are
//!   several), the functions that encode and decode each direction, and `Reply`, the responder
//!   a service answers with;
//! - `Client`, wrapping an endpoint capability; `Server`, the trait a service implements; and
//!   `dispatch`, which checks and decodes a request and calls the right method.
//!
//! Everything else is in `granite::interface`, so the generated code stays small. Paths are
//! written in full (`::granite::…`, `::alloc::…`) so no name in an interface can hide them.
//!
//! The output is under the MIT licence (note 0008, section 10).

use crate::check::{Field, Interface, Op, Ty, TypeDef, camel_to_upper, is_top_handle};
use crate::syntax::HandleKind;
use std::fmt::Write;

const GI: &str = "::granite::interface";
const VEC: &str = "::alloc::vec::Vec";
const STRING: &str = "::alloc::string::String";
const OPTION: &str = "::core::option::Option";
const RESULT: &str = "::core::result::Result";

/// How an expression handed to the encoder holds its value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Has {
    /// A reference (`&T`, or `&str`, `&[T]`).
    Ref,
    /// A place of type `T` that mustn't be moved from (`self.field`).
    Place,
}

struct Gen<'a> {
    i: &'a Interface,
    out: String,
    depth: usize,
}

fn docs_or(docs: &[String], fallback: &str) -> Vec<String> {
    if docs.is_empty() { vec![fallback.to_string()] } else { docs.to_vec() }
}

impl Gen<'_> {
    fn line(&mut self, s: &str) {
        if s.is_empty() {
            self.out.push('\n');
            return;
        }
        for _ in 0..self.depth {
            self.out.push_str("    ");
        }
        self.out.push_str(s);
        self.out.push('\n');
    }

    fn open(&mut self, s: &str) {
        self.line(s);
        self.depth += 1;
    }

    fn close(&mut self, s: &str) {
        self.depth -= 1;
        self.line(s);
    }

    fn docs(&mut self, docs: &[String]) {
        for d in docs {
            if d.is_empty() {
                self.line("///");
            } else {
                self.line(&format!("/// {d}"));
            }
        }
    }

    fn type_name(&self, i: usize) -> &str {
        self.i.types[i].name()
    }

    fn self_name(&self) -> Vec<String> {
        self.i.name.clone()
    }

    /// The Rust type a handle arrives as.
    fn handle_type(&self, k: &HandleKind) -> String {
        match k {
            HandleKind::Interface(q) if *q == self.self_name() => "Client".into(),
            HandleKind::Interface(_) => "::granite::endpoint::Endpoint".into(),
            HandleKind::Memory => "::granite::memory::Memory".into(),
            HandleKind::Notification => "::granite::notification::Notification".into(),
            HandleKind::Process => "::granite::process::Process".into(),
            HandleKind::Thread => "::granite::thread::Thread".into(),
            HandleKind::Any => "::granite::Handle".into(),
        }
    }

    /// The owned Rust type of a value.
    fn owned(&self, ty: &Ty) -> String {
        match ty {
            Ty::Uint(n) => format!("u{}", n * 8),
            Ty::Int(n) => format!("i{}", n * 8),
            Ty::Bool => "bool".into(),
            Ty::String(_) => STRING.into(),
            Ty::Bytes(_) => format!("{VEC}<u8>"),
            Ty::Array(t, n) => format!("[{}; {n}]", self.owned(t)),
            Ty::List(t, _) => format!("{VEC}<{}>", self.owned(t)),
            Ty::Option(t) => format!("{OPTION}<{}>", self.owned(t)),
            Ty::Handle(k) => self.handle_type(k),
            Ty::Struct(i) | Ty::Enum(i) | Ty::Bits(i) => self.type_name(*i).into(),
        }
    }

    /// The type a parameter is passed as: small values and handles by value, the rest
    /// borrowed.
    fn borrowed(&self, ty: &Ty) -> String {
        match ty {
            Ty::Uint(_) | Ty::Int(_) | Ty::Bool | Ty::Enum(_) | Ty::Bits(_) | Ty::Handle(_) => {
                self.owned(ty)
            }
            Ty::Option(t) if matches!(**t, Ty::Handle(_)) || Self::by_value(t) => self.owned(ty),
            Ty::String(_) => "&str".into(),
            Ty::Bytes(_) => "&[u8]".into(),
            Ty::List(t, _) => format!("&[{}]", self.owned(t)),
            Ty::Array(..) | Ty::Option(_) | Ty::Struct(_) => format!("&{}", self.owned(ty)),
        }
    }

    /// True if `borrowed` passes the type by value (small values, and options of them).
    fn by_value(ty: &Ty) -> bool {
        match ty {
            Ty::Uint(_) | Ty::Int(_) | Ty::Bool | Ty::Enum(_) | Ty::Bits(_) => true,
            Ty::Option(t) => Self::by_value(t),
            _ => false,
        }
    }

    fn handle_check(&self, k: &HandleKind, h: &str) -> Option<String> {
        let (kind, endpoint) = match k {
            HandleKind::Interface(_) => ("ENDPOINT", true),
            HandleKind::Memory => ("MEMORY", false),
            HandleKind::Notification => ("NOTIFICATION", false),
            HandleKind::Process => ("PROCESS", false),
            HandleKind::Thread => ("THREAD", false),
            HandleKind::Any => return None,
        };
        Some(format!("{GI}::check_kind(&{h}, ::granite::sys::kind::{kind}, {endpoint})?;"))
    }

    fn wrap_handle(&self, k: &HandleKind, h: &str) -> String {
        match k {
            HandleKind::Any => h.into(),
            HandleKind::Interface(q) if *q == self.self_name() => {
                format!("Client::new(::granite::endpoint::Endpoint({h}))")
            }
            _ => format!("{}({h})", self.handle_type(k)),
        }
    }

    fn unwrap_handle(&self, k: &HandleKind, v: &str) -> String {
        match k {
            HandleKind::Any => v.into(),
            HandleKind::Interface(q) if *q == self.self_name() => format!("{v}.into_endpoint().0"),
            _ => format!("{v}.0"),
        }
    }

    /// Statements encoding `x` (held as `has`) into `e_`.
    fn enc(&mut self, ty: &Ty, x: &str, has: Has) {
        let val = if has == Has::Ref { format!("*{x}") } else { x.to_string() };
        let reference = if has == Has::Ref { x.to_string() } else { format!("&{x}") };
        match ty {
            Ty::Uint(8) => self.line(&format!("e_.uint({val}, 8);")),
            Ty::Uint(n) => self.line(&format!("e_.uint(u64::from({val}), {n});")),
            Ty::Int(8) => self.line(&format!("e_.int({val}, 8);")),
            Ty::Int(n) => self.line(&format!("e_.int(i64::from({val}), {n});")),
            Ty::Bool => self.line(&format!("e_.bool({val});")),
            Ty::Enum(_) => self.line(&format!("e_.uint(u64::from(u32::from({val})), 4);")),
            Ty::Bits(i) => {
                let size = self.i.size_align(&Ty::Bits(*i)).0;
                if size == 8 {
                    self.line(&format!("e_.uint({x}.0, 8);"));
                } else {
                    self.line(&format!("e_.uint(u64::from({x}.0), {size});"));
                }
            }
            Ty::String(_) => self.line(&format!("e_.string({reference});")),
            Ty::Bytes(_) => self.line(&format!("e_.bytes({reference});")),
            Ty::List(t, _) => {
                let size = self.i.size_align(t).0;
                self.open(&format!("e_.list({reference}, {size}, |e_, v_| {{"));
                self.enc(t, "v_", Has::Ref);
                self.close("});");
            }
            Ty::Array(t, _) => {
                self.open(&format!("for v_ in {x}.iter() {{"));
                self.enc(t, "v_", Has::Ref);
                self.close("}");
            }
            Ty::Option(t) if matches!(**t, Ty::Handle(_)) => {
                let Ty::Handle(k) = &**t else { unreachable!("a handle") };
                self.open(&format!("match {x} {{"));
                self.open(&format!("{OPTION}::Some(v_) => {{"));
                self.line("e_.align(4);");
                self.line("e_.bool(true);");
                self.line("e_.align(4);");
                let h = self.unwrap_handle(k, "v_");
                self.line(&format!("e_.handle({h});"));
                self.close("}");
                self.open(&format!("{OPTION}::None => {{"));
                self.line("e_.align(4);");
                self.line("e_.bool(false);");
                self.line("e_.align(4);");
                self.line("e_.zeros(4);");
                self.close("}");
                self.close("}");
            }
            Ty::Option(t) => {
                let (size, align) = self.i.size_align(t);
                // An option of a small value is copied; any other is borrowed.
                let copied = Self::by_value(t);
                self.open(&format!("match {} {{", if copied { &val } else { &reference }));
                self.open(&format!("{OPTION}::Some(v_) => {{"));
                self.line(&format!("e_.align({align});"));
                self.line("e_.bool(true);");
                self.line(&format!("e_.align({align});"));
                self.enc(t, "v_", if copied { Has::Place } else { Has::Ref });
                self.close("}");
                self.open(&format!("{OPTION}::None => {{"));
                self.line(&format!("e_.align({align});"));
                self.line("e_.bool(false);");
                self.line(&format!("e_.align({align});"));
                self.line(&format!("e_.zeros({size});"));
                self.close("}");
                self.close("}");
            }
            Ty::Struct(_) => self.line(&format!("{x}.encode(e_);")),
            Ty::Handle(k) => {
                let h = self.unwrap_handle(k, x);
                self.line(&format!("e_.handle({h});"));
            }
        }
    }

    /// An expression decoding a value from `d_` (using `?`).
    fn dec(&self, ty: &Ty) -> String {
        match ty {
            Ty::Uint(8) => "d_.uint(8)?".into(),
            Ty::Uint(n) => format!("d_.uint({n})? as u{}", n * 8),
            Ty::Int(8) => "d_.int(8)?".into(),
            Ty::Int(n) => format!("d_.int({n})? as i{}", n * 8),
            Ty::Bool => "d_.bool()?".into(),
            Ty::Enum(i) | Ty::Bits(i) | Ty::Struct(i) => {
                format!("{}::decode(d_)?", self.type_name(*i))
            }
            Ty::String(n) => format!("d_.string({n})?"),
            Ty::Bytes(n) => format!("d_.bytes({n})?"),
            Ty::List(t, n) => {
                let size = self.i.size_align(t).0;
                let inner = self.dec(t);
                let each = match inner.strip_suffix('?') {
                    Some(call) => call.to_string(),
                    None => format!("{RESULT}::Ok({inner})"),
                };
                format!("d_.list({n}, {size}, |d_| {each})?")
            }
            Ty::Array(t, n) => format!(
                "{{ let mut a_ = {VEC}::with_capacity({n}); for _ in 0..{n} {{ a_.push({}); }} \
                 <[{}; {n}]>::try_from(a_).map_err(|_| {GI}::Malformed)? }}",
                self.dec(t),
                self.owned(t)
            ),
            Ty::Option(t) => {
                let (size, align) = self.i.size_align(t);
                format!(
                    "{{ d_.align({align})?; let p_ = d_.tag()?; d_.align({align})?; \
                     if p_ {{ {OPTION}::Some({}) }} else {{ d_.zeros({size})?; {OPTION}::None }} }}",
                    self.dec(t)
                )
            }
            Ty::Handle(k) => {
                let check = self.handle_check(k, "h_").unwrap_or_default();
                format!("{{ let h_ = d_.handle()?; {check} {} }}", self.wrap_handle(k, "h_"))
            }
        }
    }

    fn types(&mut self) {
        for t in self.i.types.clone() {
            self.line("");
            match &t {
                TypeDef::Struct { docs, name, fields, align, .. } => {
                    self.docs(&docs_or(docs, &format!("The `{name}` struct.")));
                    self.line("#[derive(Clone, Debug, PartialEq, Eq)]");
                    self.open(&format!("pub struct {name} {{"));
                    for f in fields {
                        self.docs(&docs_or(&f.docs, &format!("The `{}` field.", f.name)));
                        let ty = self.owned(&f.ty);
                        self.line(&format!("pub {}: {ty},", f.name));
                    }
                    self.close("}");
                    self.line("");
                    self.open(&format!("impl {name} {{"));
                    self.line("/// Lays the value out in a message.");
                    self.open(&format!("pub fn encode(&self, e_: &mut {GI}::Encoder) {{"));
                    self.line(&format!("e_.align({align});"));
                    for f in fields {
                        self.enc(&f.ty, &format!("self.{}", f.name), Has::Place);
                    }
                    self.line(&format!("e_.align({align});"));
                    self.close("}");
                    self.line("");
                    self.line("/// Reads and checks a value.");
                    self.open(&format!(
                        "pub fn decode(d_: &mut {GI}::Decoder<'_>) -> {RESULT}<Self, {GI}::Malformed> {{"
                    ));
                    self.line(&format!("d_.align({align})?;"));
                    if fields.is_empty() {
                        self.line("let v_ = Self {};");
                    } else {
                        self.open("let v_ = Self {");
                        for f in fields {
                            let d = self.dec(&f.ty);
                            self.line(&format!("{}: {d},", f.name));
                        }
                        self.close("};");
                    }
                    self.line(&format!("d_.align({align})?;"));
                    self.line(&format!("{RESULT}::Ok(v_)"));
                    self.close("}");
                    self.close("}");
                }
                TypeDef::Enum { docs, name, flexible, values } => {
                    self.docs(&docs_or(docs, &format!("The `{name}` enum.")));
                    self.line("#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]");
                    self.open(&format!("pub enum {name} {{"));
                    for v in values {
                        self.docs(&docs_or(&v.docs, &format!("`{}` ({}).", v.name, v.value)));
                        self.line(&format!("{},", v.name));
                    }
                    if *flexible {
                        self.line("/// A value this side doesn't know (from a newer version of the interface).");
                        self.line("Unknown(u32),");
                    }
                    self.close("}");
                    self.line("");
                    self.open(&format!("impl ::core::convert::From<{name}> for u32 {{"));
                    self.open(&format!("fn from(v: {name}) -> u32 {{"));
                    self.open("match v {");
                    for v in values {
                        self.line(&format!("{name}::{} => {},", v.name, v.value));
                    }
                    if *flexible {
                        self.line(&format!("{name}::Unknown(n) => n,"));
                    }
                    self.close("}");
                    self.close("}");
                    self.close("}");
                    self.line("");
                    self.open(&format!("impl {name} {{"));
                    if *flexible {
                        self.line("/// The value numbered `v` (`Unknown` if it isn't one this side knows).");
                    } else {
                        self.line("/// The value numbered `v`, if there is one.");
                    }
                    self.open(&format!("pub fn from_u32(v: u32) -> {OPTION}<Self> {{"));
                    self.open("match v {");
                    for v in values {
                        self.line(&format!("{} => {OPTION}::Some(Self::{}),", v.value, v.name));
                    }
                    if *flexible {
                        self.line(&format!("n => {OPTION}::Some(Self::Unknown(n)),"));
                    } else {
                        self.line(&format!("_ => {OPTION}::None,"));
                    }
                    self.close("}");
                    self.close("}");
                    self.line("");
                    self.line("/// Reads and checks a value.");
                    self.open(&format!(
                        "pub fn decode(d_: &mut {GI}::Decoder<'_>) -> {RESULT}<Self, {GI}::Malformed> {{"
                    ));
                    self.line(&format!(
                        "Self::from_u32(d_.uint(4)? as u32).ok_or({GI}::Malformed)"
                    ));
                    self.close("}");
                    self.close("}");
                }
                TypeDef::Bits { docs, name, size, bits } => {
                    let int = format!("u{}", size * 8);
                    let all: u64 = bits.iter().fold(0, |a, b| a | (1u64 << b.value));
                    self.docs(&docs_or(docs, &format!("The `{name}` bits.")));
                    self.line("#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]");
                    self.line(&format!("pub struct {name}(pub {int});"));
                    self.line("");
                    self.open(&format!("impl {name} {{"));
                    for b in bits {
                        self.docs(&docs_or(&b.docs, &format!("`{}` (bit {}).", b.name, b.value)));
                        self.line(&format!(
                            "pub const {}: Self = Self(1 << {});",
                            camel_to_upper(&b.name),
                            b.value
                        ));
                    }
                    self.line("/// Every bit this side knows.");
                    self.line(&format!("pub const ALL: Self = Self({all:#x});"));
                    self.line("");
                    self.line("/// True if every bit of `other` is set here.");
                    self.open("pub const fn contains(self, other: Self) -> bool {");
                    self.line("self.0 & other.0 == other.0");
                    self.close("}");
                    self.line("");
                    self.line(
                        "/// Reads and checks a value: bits this side doesn't know are refused.",
                    );
                    self.open(&format!(
                        "pub fn decode(d_: &mut {GI}::Decoder<'_>) -> {RESULT}<Self, {GI}::Malformed> {{"
                    ));
                    if *size == 8 {
                        self.line("let v_ = d_.uint(8)?;");
                    } else {
                        self.line(&format!("let v_ = d_.uint({size})? as {int};"));
                    }
                    self.open("if v_ & !Self::ALL.0 != 0 {");
                    self.line(&format!("return {RESULT}::Err({GI}::Malformed);"));
                    self.close("}");
                    self.line(&format!("{RESULT}::Ok(Self(v_))"));
                    self.close("}");
                    self.close("}");
                    self.line("");
                    self.open(&format!("impl ::core::ops::BitOr for {name} {{"));
                    self.line("type Output = Self;");
                    self.open("fn bitor(self, other: Self) -> Self {");
                    self.line("Self(self.0 | other.0)");
                    self.close("}");
                    self.close("}");
                }
            }
        }
    }

    /// `name: type` parameters, borrowed.
    fn params(&self, fields: &[Field]) -> String {
        fields
            .iter()
            .map(|f| format!("{}: {}", f.name, self.borrowed(&f.ty)))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn args(fields: &[Field]) -> String {
        fields.iter().map(|f| f.name.clone()).collect::<Vec<_>>().join(", ")
    }

    /// What a successful reply decodes to.
    fn results_type(&self, op: &Op, prefix: &str) -> String {
        match op.results.len() {
            0 => "()".into(),
            1 => self.owned(&op.results[0].ty),
            _ => format!("{prefix}Results"),
        }
    }

    fn error_type(&self, op: &Op) -> String {
        match op.error {
            Some(i) => self.type_name(i).into(),
            None => "::core::convert::Infallible".into(),
        }
    }

    fn encode_fn(&mut self, which: &str, fields: &[Field], size_const: &str) {
        self.line(&format!("/// Lays out a {which}."));
        let params = self.params(fields);
        self.open(&format!("pub fn encode_{which}({params}) -> {GI}::Encoded {{"));
        if fields.is_empty() {
            self.line(&format!("{GI}::Encoder::new().finish({size_const})"));
        } else {
            self.line(&format!("let mut m_ = {GI}::Encoder::new();"));
            self.line("let e_ = &mut m_;");
            for f in fields {
                let has = if Self::by_value(&f.ty) || is_top_handle(&f.ty) {
                    Has::Place
                } else {
                    Has::Ref
                };
                self.enc(&f.ty, &f.name, has);
            }
            self.line(&format!("m_.finish({size_const})"));
        }
        self.close("}");
    }

    /// `decode_<which>`: reads a whole message into `ret` (a struct with the fields, or for
    /// a reply possibly a single value or nothing).
    fn decode_fn(
        &mut self,
        which: &str,
        fields: &[Field],
        size_const: &str,
        ret: &str,
        single: bool,
    ) {
        self.line(&format!("/// Reads and checks a whole {which}."));
        let m = if fields.is_empty() { "" } else { "mut " };
        self.open(&format!(
            "pub fn decode_{which}({m}m_: {GI}::Decoder<'_>) -> {RESULT}<{ret}, {GI}::Malformed> {{"
        ));
        self.line(&format!("m_.expect({size_const})?;"));
        if !fields.is_empty() {
            self.line("let d_ = &mut m_;");
        }
        if single {
            match fields.len() {
                0 => {}
                _ => {
                    let d = self.dec(&fields[0].ty);
                    self.line(&format!("let v_ = {d};"));
                }
            }
        } else if fields.is_empty() {
            self.line(&format!("let v_ = {ret} {{}};"));
        } else {
            self.open(&format!("let v_ = {ret} {{"));
            for f in fields {
                let d = self.dec(&f.ty);
                self.line(&format!("{}: {d},", f.name));
            }
            self.close("};");
        }
        self.line("m_.finish()?;");
        if single && fields.is_empty() {
            self.line(&format!("{RESULT}::Ok(())"));
        } else {
            self.line(&format!("{RESULT}::Ok(v_)"));
        }
        self.close("}");
    }

    fn fields_struct(&mut self, name: &str, doc: &str, fields: &[Field], what: &str) {
        self.line(&format!("/// {doc}"));
        self.line("#[derive(Debug)]");
        if fields.is_empty() {
            self.line(&format!("pub struct {name} {{}}"));
            return;
        }
        self.open(&format!("pub struct {name} {{"));
        for f in fields {
            self.docs(&docs_or(&f.docs, &format!("The `{}` {what}.", f.name)));
            let ty = self.owned(&f.ty);
            self.line(&format!("pub {}: {ty},", f.name));
        }
        self.close("}");
    }

    fn op_module(&mut self, op: &Op) {
        let version = u32::from(self.i.version);
        self.line("");
        self.docs(&docs_or(&op.docs, &format!("The `{}` operation.", op.name)));
        self.line("///");
        self.line("/// Its numbers and limits, and how its messages are encoded.");
        self.open(&format!("pub mod {} {{", op.name));
        self.line("#[allow(unused_imports, reason = \"the interface's types, when the operation uses them\")]");
        self.line("use super::*;");
        self.line("");
        self.line("/// The operation's number.");
        self.line(&format!("pub const OP: u16 = {};", op.op));
        self.line("/// The request's label: the version, then the operation.");
        self.line(&format!("pub const LABEL: u32 = {:#x};", (version << 16) | u32::from(op.op)));
        self.line("/// The request's fixed part, in bytes.");
        self.line(&format!("pub const REQUEST_FIXED: usize = {};", op.request.fixed));
        self.line("/// The largest the request's attached buffer can be.");
        self.line(&format!("pub const REQUEST_BUFFER: usize = {};", op.request.var));
        self.line("/// The most capabilities the request carries.");
        self.line(&format!("pub const REQUEST_CAPS: usize = {};", op.request.caps));
        self.line("/// The reply's fixed part, in bytes.");
        self.line(&format!("pub const REPLY_FIXED: usize = {};", op.reply.fixed));
        self.line("/// The largest the reply's attached buffer can be.");
        self.line(&format!("pub const REPLY_BUFFER: usize = {};", op.reply.var));
        self.line("/// The most capabilities the reply carries.");
        self.line(&format!("pub const REPLY_CAPS: usize = {};", op.reply.caps));
        self.line("");
        self.fields_struct("Request", "The request's parameters.", &op.params, "parameter");
        if op.results.len() > 1 {
            self.line("");
            self.fields_struct("Results", "The results.", &op.results, "result");
        }
        self.line("");
        self.encode_fn("request", &op.params, "REQUEST_FIXED");
        self.line("");
        self.decode_fn("request", &op.params, "REQUEST_FIXED", "Request", false);
        self.line("");
        self.encode_fn("reply", &op.results, "REPLY_FIXED");
        self.line("");
        let ret = self.results_type(op, "");
        self.decode_fn("reply", &op.results, "REPLY_FIXED", &ret, op.results.len() <= 1);

        // The responder.
        self.line("");
        self.line("/// How a service answers this operation: [`Reply::ok`] with the results,");
        if op.error.is_some() {
            self.line("/// [`Reply::err`] with an error, or [`Reply::fail`] if it can't.");
        } else {
            self.line("/// or [`Reply::fail`] if it can't.");
        }
        self.line("///");
        self.line("/// Dropping it unanswered tells the caller the service has gone.");
        self.line("#[derive(Debug)]");
        self.open("pub struct Reply {");
        self.line("reply: ::granite::endpoint::ReplyCap,");
        self.line("badge: u64,");
        self.close("}");
        self.line("");
        self.open("impl Reply {");
        self.open("pub(super) fn new(reply: ::granite::endpoint::ReplyCap, badge: u64) -> Self {");
        self.line("Reply { reply, badge }");
        self.close("}");
        self.line("");
        self.line("/// The badge of the capability the caller used.");
        self.open("pub fn badge(&self) -> u64 {");
        self.line("self.badge");
        self.close("}");
        self.line("");
        self.line("/// Answers with the results.");
        let params = self.params(&op.results);
        self.open(&format!(
            "pub fn ok(self{}{params}) -> ::granite::Result<()> {{",
            if params.is_empty() { "" } else { ", " }
        ));
        let args = Self::args(&op.results);
        self.line(&format!("{GI}::answer(self.reply, {GI}::STATUS_OK, encode_reply({args}))"));
        self.close("}");
        if let Some(e) = op.error {
            let name = self.type_name(e).to_string();
            self.line("");
            self.line("/// Answers with an error.");
            self.open(&format!("pub fn err(self, error: {name}) -> ::granite::Result<()> {{"));
            self.line(&format!("{GI}::answer_error(self.reply, u32::from(error))"));
            self.close("}");
        }
        self.line("");
        self.line("/// Answers that the service failed.");
        self.open("pub fn fail(self) -> ::granite::Result<()> {");
        self.line(&format!("{GI}::answer_status(self.reply, {GI}::STATUS_FAILED)"));
        self.close("}");
        self.close("}");
        self.close("}");
    }

    fn client(&mut self) {
        let name = self.i.name.join(".");
        self.line("");
        self.line(&format!(
            "/// A client of `{name}`: an endpoint capability, and a deadline for its calls."
        ));
        self.line("#[derive(Debug)]");
        self.open("pub struct Client {");
        self.line("endpoint: ::granite::endpoint::Endpoint,");
        self.line("deadline: u64,");
        self.close("}");
        self.line("");
        self.open("impl Client {");
        self.line("/// A client calling `endpoint`, with no deadline.");
        self.open("pub fn new(endpoint: ::granite::endpoint::Endpoint) -> Self {");
        self.line("Client { endpoint, deadline: ::granite::time::FOREVER }");
        self.close("}");
        self.line("");
        self.line("/// The endpoint capability.");
        self.open("pub fn endpoint(&self) -> &::granite::endpoint::Endpoint {");
        self.line("&self.endpoint");
        self.close("}");
        self.line("");
        self.line("/// Gives back the endpoint capability.");
        self.open("pub fn into_endpoint(self) -> ::granite::endpoint::Endpoint {");
        self.line("self.endpoint");
        self.close("}");
        self.line("");
        self.line(
            "/// Sets the deadline (on the monotonic clock, in nanoseconds) for later calls.",
        );
        self.open("pub fn set_deadline(&mut self, deadline: u64) {");
        self.line("self.deadline = deadline;");
        self.close("}");
        for op in self.i.ops.clone() {
            self.line("");
            self.docs(&docs_or(&op.docs, &format!("Calls `{}`.", op.name)));
            let params = self.params(&op.params);
            let ret = self.results_type(&op, &format!("{}::", op.name));
            let err = self.error_type(&op);
            self.open(&format!(
                "pub fn {}(&self{}{params}) -> {RESULT}<{ret}, {GI}::CallError<{err}>> {{",
                op.name,
                if params.is_empty() { "" } else { ", " }
            ));
            let args = Self::args(&op.params);
            let m = &op.name;
            self.line(&format!(
                "let (r_, b_) = {GI}::call(&self.endpoint, {m}::LABEL, {m}::encode_request({args}), {m}::REPLY_BUFFER, self.deadline)?;"
            ));
            let error_fn = match op.error {
                Some(e) => format!("{}::from_u32", self.type_name(e)),
                None => format!("{GI}::no_error"),
            };
            self.line(&format!("{GI}::reply(r_, &b_, {m}::decode_reply, {error_fn})"));
            self.close("}");
        }
        self.close("}");
    }

    fn server(&mut self) {
        let name = self.i.name.join(".");
        self.line("");
        self.line(&format!(
            "/// What a service of `{name}` implements: one method per operation. Each"
        ));
        self.line("/// answers through its `reply`, at once or later.");
        self.open("pub trait Server {");
        let ops = self.i.ops.clone();
        for (n, op) in ops.iter().enumerate() {
            if n > 0 {
                self.line("");
            }
            self.docs(&docs_or(&op.docs, &format!("Serves `{}`.", op.name)));
            let params: Vec<String> =
                op.params.iter().map(|f| format!("{}: {}", f.name, self.owned(&f.ty))).collect();
            let mut all = vec!["&mut self".to_string()];
            all.extend(params);
            all.push(format!("reply: {}::Reply", op.name));
            self.line(&format!("fn {}({});", op.name, all.join(", ")));
        }
        self.close("}");

        self.line("");
        self.line("/// Checks and decodes a request, and calls `server`'s method for it. A malformed request");
        self.line("/// is answered as such (its capabilities closed), and an unknown operation as unknown.");
        self.line(
            "/// A request for another version of the interface is handed back, to try elsewhere.",
        );
        self.open("pub fn dispatch<S: Server + ?Sized>(");
        self.line("server: &mut S,");
        self.line("mut msg: ::granite::message::Incoming,");
        self.line("buffer: &[u8],");
        self.line("reply: ::granite::endpoint::ReplyCap,");
        self.depth -= 1;
        self.line(&format!(
            ") -> {OPTION}<(::granite::message::Incoming, ::granite::endpoint::ReplyCap)> {{"
        ));
        self.depth += 1;
        self.open("if msg.label >> 16 != u32::from(VERSION) {");
        self.line(&format!("return {OPTION}::Some((msg, reply));"));
        self.close("}");
        if ops.is_empty() {
            self.line("let _ = (server, &mut msg, buffer);");
            self.line(&format!("{GI}::unknown(reply);"));
        } else {
            self.line("let badge = msg.badge;");
            self.open("match (msg.label & 0xFFFF) as u16 {");
            for op in &ops {
                let m = &op.name;
                self.open(&format!(
                    "{m}::OP => match {m}::decode_request({GI}::Decoder::incoming(&mut msg, buffer).1) {{"
                ));
                let mut args = vec![];
                for f in &op.params {
                    args.push(format!("r_.{}", f.name));
                }
                args.push(format!("{m}::Reply::new(reply, badge)"));
                let r = if op.params.is_empty() { "_" } else { "r_" };
                self.line(&format!("{RESULT}::Ok({r}) => server.{m}({}),", args.join(", ")));
                self.line(&format!("{RESULT}::Err(_) => {GI}::reject(reply),"));
                self.close("},");
            }
            self.line(&format!("_ => {GI}::unknown(reply),"));
            self.close("}");
        }
        self.line(&format!("{OPTION}::None"));
        self.close("}");
    }

    fn module(&mut self, source: &str) {
        let name = self.i.name.join(".");
        let docs = docs_or(&self.i.docs, &format!("The `{name}` interface."));
        self.docs(&docs);
        self.line("///");
        self.line(&format!(
            "/// Generated from `{source}` (`{name}` version {}). Don't edit: change the",
            self.i.version
        ));
        self.line("/// interface file instead.");
        self.line("#[allow(clippy::too_many_arguments, reason = \"an operation has as many parameters as it needs\")]");
        self.open(&format!("pub mod {} {{", self.i.module_name()));
        self.line("/// The interface's name.");
        self.line(&format!("pub const NAME: &str = \"{name}\";"));
        self.line("/// The interface's major version.");
        self.line(&format!("pub const VERSION: u16 = {};", self.i.version));
        let req = self.i.ops.iter().map(|o| o.request.var).max().unwrap_or(0);
        let rep = self.i.ops.iter().map(|o| o.reply.var).max().unwrap_or(0);
        self.line(
            "/// The largest attached buffer any request can have: what a service receives into.",
        );
        self.line(&format!("pub const REQUEST_BUFFER: usize = {req};"));
        self.line("/// The largest attached buffer any reply can have.");
        self.line(&format!("pub const REPLY_BUFFER: usize = {rep};"));
        for c in self.i.consts.clone() {
            self.line("");
            self.docs(&docs_or(&c.docs, &format!("The `{}` constant.", c.name)));
            self.line(&format!("pub const {}: u{} = {};", c.name, c.size * 8, c.value));
        }
        self.types();
        for op in self.i.ops.clone() {
            self.op_module(&op);
        }
        self.client();
        self.server();
        self.close("}");
    }
}

/// The Rust module for `iface`, read from the file `source` (named in its documentation).
pub fn generate(iface: &Interface, source: &str) -> String {
    let mut g = Gen { i: iface, out: String::new(), depth: 0 };
    g.module(source);
    g.out
}

/// The header that starts a file of generated code.
pub fn header() -> String {
    let mut s = String::new();
    let _ = writeln!(s, "// SPDX-License-Identifier: MIT");
    let _ = writeln!(s, "//");
    let _ = writeln!(
        s,
        "// Generated by the PrettyBigRock interface compiler (lib/interface-compiler). The"
    );
    let _ = writeln!(
        s,
        "// compiler is GPL-3.0-or-later, but its output, like the interface files it reads, is"
    );
    let _ = writeln!(
        s,
        "// under the MIT licence (docs/design/0008-messages-and-interfaces.md, section 10)."
    );
    s
}
