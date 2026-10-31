// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Checking a parsed file against the rules of docs/design/0008-messages-and-interfaces.md
//! (sections 7 and 8), and working out every message's layout.
//!
//! The result, an [`Interface`], is what the generators work from: every name resolved, every
//! number in range, and each operation's largest request and reply known.

use crate::Diagnostic;
use crate::syntax::{self, File, HandleKind, Item, Name, Pos, Type};
use std::collections::{BTreeMap, BTreeSet};

/// The bytes of a message's fixed part (the message page's `data`).
pub const FIXED_MAX: u64 = 240;
/// The bytes of a message's variable part (the attached buffer).
pub const VARIABLE_MAX: u64 = 65536;
/// The capabilities a message can carry.
pub const CAPS_MAX: u64 = 4;

/// A resolved type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Ty {
    /// An unsigned integer of this many bytes.
    Uint(u8),
    /// A signed integer of this many bytes.
    Int(u8),
    /// `bool`.
    Bool,
    /// `string<max>`.
    String(u32),
    /// `bytes<max>`.
    Bytes(u32),
    /// `array<T, N>`.
    Array(Box<Ty>, u32),
    /// `list<T, max>`.
    List(Box<Ty>, u32),
    /// `option<T>`.
    Option(Box<Ty>),
    /// `handle<kind>` (only ever a parameter or result, or inside its `option`).
    Handle(HandleKind),
    /// The struct [`Interface::types`]`[i]`.
    Struct(usize),
    /// The enum [`Interface::types`]`[i]`.
    Enum(usize),
    /// The bits type [`Interface::types`]`[i]`.
    Bits(usize),
}

/// A struct's field, or an operation's parameter or result.
#[derive(Clone, Debug)]
pub struct Field {
    /// Its documentation.
    pub docs: Vec<String>,
    /// Its name.
    pub name: String,
    /// Its type.
    pub ty: Ty,
}

/// An enum's value, or a bit.
#[derive(Clone, Debug)]
pub struct Value {
    /// Its documentation.
    pub docs: Vec<String>,
    /// Its name.
    pub name: String,
    /// Its number (a bit's number for bits).
    pub value: u32,
}

/// A type defined in the file.
#[derive(Clone, Debug)]
pub enum TypeDef {
    /// A struct.
    Struct {
        /// Documentation.
        docs: Vec<String>,
        /// Name.
        name: String,
        /// Fields.
        fields: Vec<Field>,
        /// Its size in a fixed part.
        size: u64,
        /// Its alignment.
        align: u64,
    },
    /// An enum.
    Enum {
        /// Documentation.
        docs: Vec<String>,
        /// Name.
        name: String,
        /// Whether unknown values are accepted.
        flexible: bool,
        /// Values.
        values: Vec<Value>,
    },
    /// A bits type.
    Bits {
        /// Documentation.
        docs: Vec<String>,
        /// Name.
        name: String,
        /// Its size in bytes.
        size: u8,
        /// Its bits.
        bits: Vec<Value>,
    },
}

impl TypeDef {
    /// The type's name.
    pub fn name(&self) -> &str {
        match self {
            TypeDef::Struct { name, .. }
            | TypeDef::Enum { name, .. }
            | TypeDef::Bits { name, .. } => name,
        }
    }
}

/// A constant.
#[derive(Clone, Debug)]
pub struct Const {
    /// Documentation.
    pub docs: Vec<String>,
    /// Name.
    pub name: String,
    /// Size in bytes.
    pub size: u8,
    /// Value.
    pub value: u64,
}

/// The size of one direction of an operation's messages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    /// The fixed part's size, exactly.
    pub fixed: u64,
    /// The largest the variable part can be.
    pub var: u64,
    /// The most capabilities it carries.
    pub caps: u64,
}

/// An operation.
#[derive(Clone, Debug)]
pub struct Op {
    /// Documentation.
    pub docs: Vec<String>,
    /// Name.
    pub name: String,
    /// Number.
    pub op: u16,
    /// Parameters.
    pub params: Vec<Field>,
    /// Results.
    pub results: Vec<Field>,
    /// The error enum ([`Interface::types`] index), if it can fail.
    pub error: Option<usize>,
    /// The request's layout.
    pub request: Layout,
    /// The (successful) reply's layout.
    pub reply: Layout,
}

/// A checked interface.
#[derive(Clone, Debug)]
pub struct Interface {
    /// Documentation.
    pub docs: Vec<String>,
    /// Qualified name.
    pub name: Vec<String>,
    /// Major version.
    pub version: u16,
    /// Types, in the order written.
    pub types: Vec<TypeDef>,
    /// Constants, in the order written.
    pub consts: Vec<Const>,
    /// Operations, in the order written.
    pub ops: Vec<Op>,
}

fn round_up(v: u64, align: u64) -> u64 {
    v.div_ceil(align).saturating_mul(align)
}

impl Interface {
    /// A type's (size, alignment) in a fixed part (note 0008, section 8).
    pub fn size_align(&self, ty: &Ty) -> (u64, u64) {
        match ty {
            Ty::Uint(n) | Ty::Int(n) => (u64::from(*n), u64::from(*n)),
            Ty::Bool => (1, 1),
            Ty::String(_) | Ty::Bytes(_) | Ty::List(..) => (8, 4),
            Ty::Handle(_) | Ty::Enum(_) => (4, 4),
            Ty::Bits(i) => match &self.types[*i] {
                TypeDef::Bits { size, .. } => (u64::from(*size), u64::from(*size)),
                _ => unreachable!("bits type index"),
            },
            Ty::Struct(i) => match &self.types[*i] {
                TypeDef::Struct { size, align, .. } => (*size, *align),
                _ => unreachable!("struct index"),
            },
            Ty::Array(t, n) => {
                let (s, a) = self.size_align(t);
                (s.saturating_mul(u64::from(*n)), a)
            }
            Ty::Option(t) => {
                // A u8 tag, then the value at its own alignment. Every size is a multiple of
                // its alignment, so nothing follows the value.
                let (s, a) = self.size_align(t);
                (a.saturating_add(s), a)
            }
        }
    }

    /// The most bytes a value of `ty` can put in the variable part, counting the padding that
    /// aligns each item to 8.
    pub fn var_max(&self, ty: &Ty) -> u64 {
        match ty {
            Ty::String(n) | Ty::Bytes(n) => round_up(u64::from(*n), 8),
            Ty::List(t, n) => {
                let n = u64::from(*n);
                let block = round_up(self.size_align(t).0.saturating_mul(n), 8);
                block.saturating_add(self.var_max(t).saturating_mul(n))
            }
            Ty::Array(t, n) => self.var_max(t).saturating_mul(u64::from(*n)),
            Ty::Option(t) => self.var_max(t),
            Ty::Struct(i) => match &self.types[*i] {
                TypeDef::Struct { fields, .. } => {
                    fields.iter().fold(0u64, |acc, f| acc.saturating_add(self.var_max(&f.ty)))
                }
                _ => unreachable!("struct index"),
            },
            _ => 0,
        }
    }

    /// The (size, alignment) of a list of fields laid out like a struct.
    pub fn fields_layout(&self, fields: &[Field]) -> (u64, u64) {
        let (mut at, mut align) = (0u64, 1u64);
        for f in fields {
            let (s, a) = self.size_align(&f.ty);
            at = round_up(at, a).saturating_add(s);
            align = align.max(a);
        }
        (round_up(at, align), align)
    }

    /// The layout of a message carrying `fields`.
    pub fn message_layout(&self, fields: &[Field]) -> Layout {
        Layout {
            fixed: self.fields_layout(fields).0,
            var: fields.iter().fold(0u64, |acc, f| acc.saturating_add(self.var_max(&f.ty))),
            caps: fields.iter().filter(|f| is_top_handle(&f.ty)).count() as u64,
        }
    }

    /// The module name the interface's generated code goes in (`pbr.file` is `pbr_file`).
    pub fn module_name(&self) -> String {
        self.name.join("_")
    }
}

/// True for `handle<…>` and `option<handle<…>>`, the only places a handle may be.
pub fn is_top_handle(ty: &Ty) -> bool {
    match ty {
        Ty::Handle(_) => true,
        Ty::Option(t) => matches!(**t, Ty::Handle(_)),
        _ => false,
    }
}

fn contains_handle(ty: &Type) -> bool {
    match ty {
        Type::Handle(_) => true,
        Type::Array(t, _) | Type::List(t, _) | Type::Option(t) => contains_handle(t),
        _ => false,
    }
}

/// Rust's keywords (and reserved words), and C's: names that would need escaping in
/// generated code. C's are refused now so that a C generator (planned for M9) never meets
/// an interface it can't express.
const KEYWORDS: &[&str] = &[
    // Rust
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "type", "unsafe",
    "use", "where", "while", "abstract", "become", "box", "do", "final", "macro", "override",
    "priv", "typeof", "unsized", "virtual", "yield", "try", "union", // C
    "auto", "case", "char", "default", "double", "float", "goto", "inline", "int", "long",
    "register", "restrict", "short", "signed", "sizeof", "switch", "typedef", "unsigned", "void",
    "volatile",
];

/// Type names the generated code uses itself.
const RESERVED_TYPES: &[&str] = &["Client", "Server", "Reply", "Request", "Results", "Self"];

/// Constant names the generated code uses itself.
const RESERVED_CONSTS: &[&str] = &[
    "VERSION",
    "NAME",
    "REQUEST_BUFFER",
    "REPLY_BUFFER",
    "OP",
    "LABEL",
    "REQUEST_FIXED",
    "REPLY_FIXED",
    "REQUEST_CAPS",
    "REPLY_CAPS",
    "ALL",
];

/// Operation names that would clash with the generated client's own methods.
/// (Primitive types' names too: an operation's module of that name would hide the type.)
const RESERVED_OPS: &[&str] = &[
    "new",
    "endpoint",
    "into_endpoint",
    "set_deadline",
    "deadline",
    "u8",
    "u16",
    "u32",
    "u64",
    "u128",
    "usize",
    "i8",
    "i16",
    "i32",
    "i64",
    "i128",
    "isize",
    "f32",
    "f64",
    "bool",
    "str",
];

/// `snake_case`: lower-case words of letters and digits joined by single underscores.
fn is_snake(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && s.split('_').all(|w| {
            !w.is_empty() && w.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        })
}

/// `CamelCase`: starts upper-case, then letters and digits.
fn is_camel(s: &str) -> bool {
    s.starts_with(|c: char| c.is_ascii_uppercase()) && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `UPPER_CASE`: upper-case words of letters and digits joined by single underscores.
fn is_upper(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_uppercase())
        && s.split('_').all(|w| {
            !w.is_empty() && w.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
        })
}

/// `CamelCase` to `UPPER_CASE` (`ReadWrite` is `READ_WRITE`), for bits' constants.
pub fn camel_to_upper(s: &str) -> String {
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if c.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(c.to_ascii_uppercase());
    }
    out
}

/// `snake_case` to `CamelCase`.
pub fn snake_to_camel(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

type R<T> = Result<T, Diagnostic>;

fn err<T>(pos: Pos, message: impl Into<String>) -> R<T> {
    Err(Diagnostic::new(pos, message))
}

fn snake(n: &Name, what: &str) -> R<()> {
    if !is_snake(&n.text) {
        return err(n.pos, format!("{what} `{}` must be snake_case (such as `read_all`)", n.text));
    }
    if KEYWORDS.contains(&n.text.as_str()) {
        return err(n.pos, format!("{what} `{}` is a reserved word in Rust or C", n.text));
    }
    Ok(())
}

fn camel(n: &Name, what: &str) -> R<()> {
    if !is_camel(&n.text) {
        return err(n.pos, format!("{what} `{}` must be CamelCase (such as `FileError`)", n.text));
    }
    Ok(())
}

#[derive(Clone, Copy)]
enum Def {
    Type(usize),
    Const,
}

struct Checker<'a> {
    names: BTreeMap<String, Def>,
    /// For each type: its item in the file.
    items: Vec<&'a Item>,
}

impl Checker<'_> {
    fn resolve(&self, ty: &Type, pos: Pos) -> R<Ty> {
        Ok(match ty {
            Type::Uint(n) => Ty::Uint(*n),
            Type::Int(n) => Ty::Int(*n),
            Type::Bool => Ty::Bool,
            Type::String(n) | Type::Bytes(n) | Type::List(_, n) | Type::Array(_, n)
                if *n == 0 || *n > VARIABLE_MAX =>
            {
                return err(pos, format!("a length must be from 1 to {VARIABLE_MAX}, not {n}"));
            }
            Type::String(n) => Ty::String(*n as u32),
            Type::Bytes(n) => Ty::Bytes(*n as u32),
            Type::Array(t, n) => Ty::Array(Box::new(self.resolve(t, pos)?), *n as u32),
            Type::List(t, n) => Ty::List(Box::new(self.resolve(t, pos)?), *n as u32),
            Type::Option(t) => {
                if matches!(**t, Type::Option(_)) {
                    return err(pos, "an option can't hold another option");
                }
                Ty::Option(Box::new(self.resolve(t, pos)?))
            }
            Type::Handle(k) => {
                if let HandleKind::Interface(parts) = k {
                    for p in parts {
                        if !is_snake(p) {
                            return err(
                                pos,
                                format!(
                                    "interface names are snake_case words joined by `.`; `{p}` isn't"
                                ),
                            );
                        }
                    }
                }
                Ty::Handle(k.clone())
            }
            Type::Named(n) => match self.names.get(&n.text) {
                Some(Def::Type(i)) => match self.items[*i] {
                    Item::Struct { .. } => Ty::Struct(*i),
                    Item::Enum { .. } => Ty::Enum(*i),
                    Item::Bits { .. } => Ty::Bits(*i),
                    _ => unreachable!("types are structs, enums and bits"),
                },
                Some(Def::Const) => {
                    return err(n.pos, format!("`{}` is a constant, not a type", n.text));
                }
                None => {
                    if KEYWORDS.contains(&n.text.as_str())
                        || ["u128", "i128", "f32", "f64", "usize", "isize", "str", "char"]
                            .contains(&n.text.as_str())
                    {
                        return err(n.pos, format!("there's no type `{}` in an interface", n.text));
                    }
                    return err(
                        n.pos,
                        format!("no type called `{}` is defined in this file", n.text),
                    );
                }
            },
        })
    }
}

/// Checks a parsed file and works out its layouts.
pub fn check(file: &File) -> Result<Interface, Diagnostic> {
    for n in &file.name {
        snake(n, "an interface name's part")?;
    }
    if file.version == 0 || file.version > 0xFFFF {
        return err(file.version_pos, "the version must be from 1 to 65535");
    }

    // Names first, so types can be used before they're defined.
    let mut c = Checker { names: BTreeMap::new(), items: Vec::new() };
    let mut type_items = Vec::new();
    let mut fn_names: BTreeSet<String> = BTreeSet::new();
    for item in &file.items {
        let (name, def) = match item {
            Item::Struct { name, .. } | Item::Enum { name, .. } | Item::Bits { name, .. } => {
                camel(name, "a type name")?;
                if RESERVED_TYPES.contains(&name.text.as_str()) {
                    return err(
                        name.pos,
                        format!(
                            "`{}` is used by the generated code; choose another name",
                            name.text
                        ),
                    );
                }
                type_items.push(item);
                (name, Def::Type(type_items.len() - 1))
            }
            Item::Const { name, .. } => {
                if !is_upper(&name.text) {
                    return err(
                        name.pos,
                        format!(
                            "a constant `{}` must be UPPER_CASE (such as `MAX_NAME`)",
                            name.text
                        ),
                    );
                }
                if RESERVED_CONSTS.contains(&name.text.as_str()) {
                    return err(
                        name.pos,
                        format!(
                            "`{}` is used by the generated code; choose another name",
                            name.text
                        ),
                    );
                }
                (name, Def::Const)
            }
            Item::Function(f) => {
                snake(&f.name, "an operation name")?;
                if RESERVED_OPS.contains(&f.name.text.as_str()) {
                    return err(
                        f.name.pos,
                        format!(
                            "`{}` is used by the generated client; choose another name",
                            f.name.text
                        ),
                    );
                }
                if !fn_names.insert(f.name.text.clone()) {
                    return err(
                        f.name.pos,
                        format!("there's already an operation called `{}`", f.name.text),
                    );
                }
                continue;
            }
            Item::Reserved { .. } => continue,
        };
        if c.names.insert(name.text.clone(), def).is_some() {
            return err(name.pos, format!("`{}` is defined twice", name.text));
        }
    }
    c.items = type_items;

    // Types, without sizes yet.
    let mut types = Vec::new();
    for item in &c.items {
        types.push(match item {
            Item::Struct { docs, name, fields } => {
                let mut seen = BTreeSet::new();
                let mut out = Vec::new();
                for f in fields {
                    snake(&f.name, "a field name")?;
                    if !seen.insert(&f.name.text) {
                        return err(f.name.pos, format!("there's already a field called `{}`", f.name.text));
                    }
                    if contains_handle(&f.ty) {
                        return err(f.name.pos, "handles can only be an operation's parameters or results, not inside a struct");
                    }
                    out.push(Field { docs: f.docs.clone(), name: f.name.text.clone(), ty: Ty::Bool });
                }
                TypeDef::Struct { docs: docs.clone(), name: name.text.clone(), fields: out, size: 0, align: 1 }
            }
            Item::Enum { docs, name, flexible, values } => {
                if values.is_empty() {
                    return err(name.pos, format!("the enum `{}` has no values", name.text));
                }
                let vals = check_values(values, u64::from(u32::MAX), "value")?;
                if *flexible && vals.iter().any(|v| v.name == "Unknown") {
                    return err(name.pos, "a flexible enum can't have a value called `Unknown`: it's used for values this side doesn't know");
                }
                TypeDef::Enum { docs: docs.clone(), name: name.text.clone(), flexible: *flexible, values: vals }
            }
            Item::Bits { docs, name, size, bits } => {
                if bits.is_empty() {
                    return err(name.pos, format!("the bits type `{}` has no bits", name.text));
                }
                let vals = check_values(bits, u64::from(*size) * 8 - 1, "bit number")?;
                let mut upper = BTreeSet::new();
                for (b, v) in bits.iter().zip(&vals) {
                    let u = camel_to_upper(&v.name);
                    if u == "ALL" || !upper.insert(u) {
                        return err(b.name.pos, format!("the bit `{}` clashes with another name once written as a constant", v.name));
                    }
                }
                TypeDef::Bits { docs: docs.clone(), name: name.text.clone(), size: *size, bits: vals }
            }
            _ => unreachable!("only types"),
        });
    }
    // Field types, now every type has an index.
    for (i, item) in c.items.iter().enumerate() {
        if let Item::Struct { fields, .. } = item {
            for (j, f) in fields.iter().enumerate() {
                let ty = c.resolve(&f.ty, f.name.pos)?;
                if let TypeDef::Struct { fields, .. } = &mut types[i] {
                    fields[j].ty = ty;
                }
            }
        }
    }
    // Struct sizes, refusing a struct that contains itself.
    let mut state = vec![0u8; types.len()]; // 0 not done, 1 in progress, 2 done
    for i in 0..types.len() {
        struct_layout(i, &mut types, &mut state, &c)?;
    }

    let mut iface = Interface {
        docs: file.docs.clone(),
        name: file.name.iter().map(|n| n.text.clone()).collect(),
        version: file.version as u16,
        types,
        consts: Vec::new(),
        ops: Vec::new(),
    };

    // Lists' elements must have a size (an element of no bytes would make a list's length
    // meaningless).
    for (i, item) in c.items.iter().enumerate() {
        if let Item::Struct { fields, .. } = item
            && let TypeDef::Struct { fields: resolved, .. } = &iface.types[i]
        {
            for (f, r) in fields.iter().zip(resolved) {
                check_lists(&iface, &r.ty, f.name.pos)?;
            }
        }
    }

    // Constants.
    for item in &file.items {
        if let Item::Const { docs, name, size, value, value_pos } = item {
            if *size < 8 && *value >> (u32::from(*size) * 8) != 0 {
                return err(*value_pos, format!("{value} doesn't fit in {} bytes", size));
            }
            iface.consts.push(Const {
                docs: docs.clone(),
                name: name.text.clone(),
                size: *size,
                value: *value,
            });
        }
    }

    // Operations.
    let mut reserved = BTreeSet::new();
    for item in &file.items {
        if let Item::Reserved { op, pos } = item {
            if *op == 0 || *op > 0xFFFF {
                return err(*pos, "operation numbers are from 1 to 65535");
            }
            if !reserved.insert(*op) {
                return err(*pos, format!("operation {op} is reserved twice"));
            }
        }
    }
    let mut used = BTreeSet::new();
    for item in &file.items {
        let Item::Function(f) = item else { continue };
        if f.op == 0 || f.op > 0xFFFF {
            return err(f.op_pos, "operation numbers are from 1 to 65535");
        }
        if reserved.contains(&f.op) {
            return err(f.op_pos, format!("operation {} is reserved and can't be used", f.op));
        }
        if !used.insert(f.op) {
            return err(f.op_pos, format!("operation {} is used twice", f.op));
        }
        let params = op_fields(&c, &iface, &f.params, "parameter")?;
        let results = op_fields(&c, &iface, &f.results, "result")?;
        let error = match &f.error {
            None => None,
            Some(n) => match c.names.get(&n.text) {
                Some(Def::Type(i)) if matches!(iface.types[*i], TypeDef::Enum { .. }) => Some(*i),
                Some(_) => {
                    return err(
                        n.pos,
                        format!("an operation's error must be an enum; `{}` isn't", n.text),
                    );
                }
                None => {
                    return err(
                        n.pos,
                        format!("no enum called `{}` is defined in this file", n.text),
                    );
                }
            },
        };
        let request = iface.message_layout(&params);
        let reply = iface.message_layout(&results);
        for (l, what) in [(request, "request"), (reply, "reply")] {
            if l.fixed > FIXED_MAX {
                return err(
                    f.name.pos,
                    format!(
                        "the {what}'s fixed part is {} bytes; at most {FIXED_MAX} fit in a message",
                        l.fixed
                    ),
                );
            }
            if l.var > VARIABLE_MAX {
                return err(
                    f.name.pos,
                    format!(
                        "the {what}'s strings, bytes and lists can reach {} bytes; at most {VARIABLE_MAX} fit in a message",
                        l.var
                    ),
                );
            }
            if l.caps > CAPS_MAX {
                return err(
                    f.name.pos,
                    format!(
                        "the {what} has {} handles; at most {CAPS_MAX} fit in a message",
                        l.caps
                    ),
                );
            }
        }
        iface.ops.push(Op {
            docs: f.docs.clone(),
            name: f.name.text.clone(),
            op: f.op as u16,
            params,
            results,
            error,
            request,
            reply,
        });
    }
    Ok(iface)
}

fn check_values(values: &[syntax::Value], max: u64, what: &str) -> R<Vec<Value>> {
    let mut names = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    let mut out = Vec::new();
    for v in values {
        camel(&v.name, &format!("a {what}'s name"))?;
        if v.name.text == "Self" {
            return err(v.name.pos, "`Self` is a reserved word in Rust");
        }
        if !names.insert(&v.name.text) {
            return err(v.name.pos, format!("`{}` is used twice", v.name.text));
        }
        if v.value > max {
            return err(v.value_pos, format!("the {what} {} is too big; at most {max}", v.value));
        }
        if !numbers.insert(v.value) {
            return err(v.value_pos, format!("the {what} {} is used twice", v.value));
        }
        out.push(Value { docs: v.docs.clone(), name: v.name.text.clone(), value: v.value as u32 });
    }
    Ok(out)
}

fn struct_layout(i: usize, types: &mut [TypeDef], state: &mut [u8], c: &Checker<'_>) -> R<()> {
    if !matches!(types[i], TypeDef::Struct { .. }) || state[i] == 2 {
        return Ok(());
    }
    let pos = match c.items[i] {
        Item::Struct { name, .. } => name.pos,
        _ => unreachable!("a struct"),
    };
    if state[i] == 1 {
        return err(
            pos,
            format!(
                "the struct `{}` contains itself, so it would be infinitely big",
                types[i].name()
            ),
        );
    }
    state[i] = 1;
    let TypeDef::Struct { fields, .. } = &types[i] else { unreachable!("a struct") };
    let mut inner = Vec::new();
    for f in fields {
        collect_structs(&f.ty, &mut inner);
    }
    for j in inner {
        struct_layout(j, types, state, c)?;
    }
    // Lay the fields out (every struct they use now has its size).
    let view = Interface {
        docs: Vec::new(),
        name: Vec::new(),
        version: 0,
        types: types.to_vec(),
        consts: Vec::new(),
        ops: Vec::new(),
    };
    let TypeDef::Struct { fields, .. } = &types[i] else { unreachable!("a struct") };
    let (size, align) = view.fields_layout(fields);
    if size > VARIABLE_MAX {
        return err(
            pos,
            format!(
                "the struct `{}` is {size} bytes; nothing that big fits in a message",
                types[i].name()
            ),
        );
    }
    if let TypeDef::Struct { size: s, align: a, .. } = &mut types[i] {
        *s = size;
        *a = align;
    }
    state[i] = 2;
    Ok(())
}

/// Every struct a type's layout depends on (all of them: a list's elements too, since a list
/// of a struct needs the struct's size).
fn collect_structs(ty: &Ty, out: &mut Vec<usize>) {
    match ty {
        Ty::Struct(i) => out.push(*i),
        Ty::Array(t, _) | Ty::List(t, _) | Ty::Option(t) => collect_structs(t, out),
        _ => {}
    }
}

fn check_lists(iface: &Interface, ty: &Ty, pos: Pos) -> R<()> {
    match ty {
        Ty::List(t, _) => {
            if iface.size_align(t).0 == 0 {
                return err(
                    pos,
                    "a list's elements must take up some space (this one's are empty structs)",
                );
            }
            check_lists(iface, t, pos)
        }
        Ty::Array(t, _) | Ty::Option(t) => check_lists(iface, t, pos),
        _ => Ok(()),
    }
}

fn op_fields(
    c: &Checker<'_>,
    iface: &Interface,
    fields: &[syntax::Field],
    what: &str,
) -> R<Vec<Field>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for f in fields {
        snake(&f.name, &format!("a {what} name"))?;
        if f.name.text == "reply" {
            return err(f.name.pos, "`reply` is used by the generated server; choose another name");
        }
        if !seen.insert(&f.name.text) {
            return err(f.name.pos, format!("there's already a {what} called `{}`", f.name.text));
        }
        let top_ok = match &f.ty {
            Type::Handle(_) => true,
            Type::Option(t) => matches!(**t, Type::Handle(_)),
            _ => false,
        };
        if !top_ok && contains_handle(&f.ty) {
            return err(
                f.name.pos,
                "a handle can only be a parameter or result itself (or an option of one), not inside a list, array or option of something else",
            );
        }
        let ty = c.resolve(&f.ty, f.name.pos)?;
        check_lists(iface, &ty, f.name.pos)?;
        out.push(Field { docs: f.docs.clone(), name: f.name.text.clone(), ty });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn iface(src: &str) -> Interface {
        check(&syntax::parse(src).unwrap()).unwrap()
    }

    fn fails(src: &str) -> String {
        check(&syntax::parse(src).unwrap()).unwrap_err().message
    }

    #[test]
    fn name_styles() {
        assert!(
            is_snake("read_all2")
                && !is_snake("Read")
                && !is_snake("a__b")
                && !is_snake("_a")
                && !is_snake("a_")
        );
        assert!(
            is_camel("FileError") && is_camel("X2") && !is_camel("file") && !is_camel("File_Error")
        );
        assert!(is_upper("MAX_NAME") && !is_upper("Max") && !is_upper("A__B"));
        assert_eq!(camel_to_upper("ReadWrite"), "READ_WRITE");
        assert_eq!(snake_to_camel("read_all"), "ReadAll");
    }

    #[test]
    fn layouts() {
        let i = iface(
            "interface t version 1;\n\
             struct A { x: u8, y: u32, }\n\
             struct B { a: A, z: u8, s: string<10>, o: option<u16>, }\n\
             bits F: u16 { X @3 }\n\
             @1 fn f(a: u8, b: B, c: u64, d: list<A, 3>, e: option<handle<memory>>, f: F) -> (r: list<string<5>, 2>);",
        );
        let a = Ty::Struct(0);
        let b = Ty::Struct(1);
        assert_eq!(i.size_align(&a), (8, 4));
        // a: 0..8, z: 8, s: 12..20, o: tag 20, value 22..24.
        assert_eq!(i.size_align(&b), (24, 4));
        assert_eq!(i.size_align(&Ty::Option(Box::new(Ty::Uint(8)))), (16, 8));
        assert_eq!(i.size_align(&Ty::Option(Box::new(Ty::Bool))), (2, 1));
        let op = &i.ops[0];
        // a 0; b 4..28; c 32..40; d 40..48; e tag 48, index 52..56; f 56..58; end 64 (align 8).
        assert_eq!(op.request, Layout { fixed: 64, var: 16 + 24, caps: 1 });
        // A list of strings: its block (2 references, 16 bytes) and two strings of up to 5
        // bytes, each padded to 8.
        assert_eq!(op.reply, Layout { fixed: 8, var: 16 + 16, caps: 0 });
    }

    #[test]
    fn rules() {
        let h = "interface t version 1;\n";
        for (src, want) in [
            ("interface T version 1;", "snake_case"),
            ("interface t version 0;", "1 to 65535"),
            ("interface t version 70000;", "1 to 65535"),
            ("@0 fn f();", "1 to 65535"),
            ("@1 fn f(); @1 fn g();", "used twice"),
            ("reserved @1; @1 fn f();", "reserved"),
            ("reserved @1; reserved @1;", "reserved twice"),
            ("@1 fn f(); @2 fn f();", "already an operation"),
            ("@1 fn Read();", "snake_case"),
            ("@1 fn f(type: u8);", "reserved word"),
            ("@1 fn f(int: u8);", "reserved word"),
            ("@1 fn new();", "generated client"),
            ("@1 fn u8();", "generated client"),
            ("@1 fn f(reply: u8);", "generated server"),
            ("@1 fn f(a: u8, a: u8);", "already a parameter"),
            ("@1 fn f(a: Nope);", "no type called"),
            ("@1 fn f(a: f32);", "no type `f32`"),
            ("const C: u8 = 1; @1 fn f(a: C);", "constant, not a type"),
            ("struct S {} @1 fn f() error S;", "must be an enum"),
            ("@1 fn f() error E;", "no enum"),
            ("struct s {}", "CamelCase"),
            ("struct Client {}", "generated code"),
            ("struct S {} enum S { A @1 }", "defined twice"),
            ("struct S { a: S, }", "contains itself"),
            ("struct S { a: option<T>, } struct T { s: array<S, 2>, }", "contains itself"),
            ("struct S { l: list<S, 2>, }", "contains itself"),
            ("struct S { h: handle<memory>, }", "not inside a struct"),
            ("@1 fn f(a: list<handle<memory>, 2>);", "handle"),
            ("@1 fn f(a: option<option<u8>>);", "another option"),
            ("@1 fn f(a: string<0>);", "1 to 65536"),
            ("@1 fn f(a: bytes<65537>);", "1 to 65536"),
            ("@1 fn f(a: array<u64, 31>);", "fixed part is 248"),
            ("@1 fn f(a: bytes<65536>, b: string<1>);", "can reach 65544"),
            ("@1 fn f() -> (a: list<string<64>, 2048>);", "reply"),
            (
                "@1 fn f(a: handle<any>, b: handle<any>, c: handle<any>, d: handle<any>, e: option<handle<any>>);",
                "5 handles",
            ),
            ("@1 fn f(a: handle<pbr.File>);", "snake_case"),
            ("struct E {} @1 fn f(a: list<E, 2>);", "take up some space"),
            ("enum E {}", "no values"),
            ("enum E { A @1, A @2 }", "used twice"),
            ("enum E { A @1, B @1 }", "used twice"),
            ("enum E { A @4294967296 }", "too big"),
            ("flexible enum E { Unknown @1 }", "Unknown"),
            ("enum E { a @1 }", "CamelCase"),
            ("bits B: u8 { A @8 }", "too big"),
            ("bits B: u8 { A @1, A @2 }", "used twice"),
            ("bits B: u8 {}", "no bits"),
            ("bits B: u8 { All @1 }", "clashes"),
            ("const C: u8 = 256;", "doesn't fit"),
            ("const c: u8 = 1;", "UPPER_CASE"),
            ("const VERSION: u8 = 1;", "generated code"),
            ("struct S { a: u8, a: u8, }", "already a field"),
            ("struct S { a: array<u64, 9000>, }", "nothing that big"),
        ] {
            let full =
                if src.starts_with("interface") { src.to_string() } else { format!("{h}{src}") };
            let got = fails(&full);
            assert!(got.contains(want), "{src}: got {got:?}, wanted {want:?}");
        }
    }

    #[test]
    fn what_is_allowed() {
        let i = iface(
            "interface pbr.t version 65535;\n\
             @65535 fn f(a: handle<any>, b: handle<any>, c: handle<pbr.other>, d: option<handle<memory>>) -> (h: handle<any>);\n\
             @2 fn g(a: bytes<65536>) -> (b: array<u64, 30>);\n\
             struct S { l: list<T, 4>, }\n struct T { x: u8, }\n\
             enum E { Zero @0, Max @4294967295 }\n bits B: u64 { Top @63 }\n const C: u64 = 0xFFFFFFFFFFFFFFFF;",
        );
        assert_eq!(i.ops[0].request.caps, 4);
        assert_eq!(i.ops[1].request.var, 65536);
        assert_eq!(i.ops[1].reply.fixed, 240);
        assert_eq!(i.module_name(), "pbr_t");
    }
}
