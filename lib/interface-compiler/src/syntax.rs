// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Reading a `.pbi` file: the tokens, the syntax tree, and the parser for the grammar in
//! docs/design/0008-messages-and-interfaces.md, section 7.
//!
//! The parser only checks the shape of the file. What the shapes mean (names, numbers,
//! sizes) is checked in [`crate::check`].

use crate::Diagnostic;

/// A place in the file: line and column, both counting from 1.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pos {
    /// The line.
    pub line: u32,
    /// The column, in characters.
    pub col: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Int(u64),
    Doc(String),
    Punct(char),
    Arrow,
    End,
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Ident(s) => format!("`{s}`"),
            Tok::Int(n) => format!("the number {n}"),
            Tok::Doc(_) => "a `///` comment".into(),
            Tok::Punct(c) => format!("`{c}`"),
            Tok::Arrow => "`->`".into(),
            Tok::End => "the end of the file".into(),
        }
    }
}

/// Splits the text into tokens, dropping whitespace and `//` comments.
fn lex(src: &str) -> Result<Vec<(Tok, Pos)>, Diagnostic> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, 1u32, 1u32);
    let advance = |i: &mut usize, line: &mut u32, col: &mut u32, chars: &[char]| {
        if chars[*i] == '\n' {
            *line += 1;
            *col = 1;
        } else {
            *col += 1;
        }
        *i += 1;
    };
    while i < chars.len() {
        let c = chars[i];
        let pos = Pos { line, col };
        if c == '\n' || c == ' ' || c == '\t' || c == '\r' {
            advance(&mut i, &mut line, &mut col, &chars);
        } else if c == '/' && chars.get(i + 1) == Some(&'/') {
            // `///` (but not `////`) is documentation; anything else is a comment.
            let doc = chars.get(i + 2) == Some(&'/') && chars.get(i + 3) != Some(&'/');
            let start = i + if doc { 3 } else { 2 };
            while i < chars.len() && chars[i] != '\n' {
                advance(&mut i, &mut line, &mut col, &chars);
            }
            if doc {
                let text: String = chars[start..i].iter().collect();
                let text = text.strip_prefix(' ').unwrap_or(&text).trim_end().to_string();
                out.push((Tok::Doc(text), pos));
            }
        } else if c.is_ascii_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                advance(&mut i, &mut line, &mut col, &chars);
            }
            out.push((Tok::Ident(chars[start..i].iter().collect()), pos));
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '_') {
                advance(&mut i, &mut line, &mut col, &chars);
            }
            let text: String = chars[start..i].iter().collect();
            let value = if let Some(hex) = text.strip_prefix("0x") {
                if hex.is_empty() || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
                    None
                } else {
                    u64::from_str_radix(hex, 16).ok()
                }
            } else if text.chars().all(|c| c.is_ascii_digit()) {
                text.parse::<u64>().ok()
            } else {
                None
            };
            match value {
                Some(v) => out.push((Tok::Int(v), pos)),
                None => {
                    return Err(Diagnostic::new(
                        pos,
                        format!(
                            "`{text}` isn't a number (decimal, or hexadecimal starting 0x, below 2^64)"
                        ),
                    ));
                }
            }
        } else if c == '-' && chars.get(i + 1) == Some(&'>') {
            advance(&mut i, &mut line, &mut col, &chars);
            advance(&mut i, &mut line, &mut col, &chars);
            out.push((Tok::Arrow, pos));
        } else if ";(){}<>,:@=.".contains(c) {
            advance(&mut i, &mut line, &mut col, &chars);
            out.push((Tok::Punct(c), pos));
        } else {
            return Err(Diagnostic::new(pos, format!("unexpected character {c:?}")));
        }
    }
    out.push((Tok::End, Pos { line, col }));
    Ok(out)
}

/// A name, and where it was written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    /// The name.
    pub text: String,
    /// Where.
    pub pos: Pos,
}

/// What kind of object a handle refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HandleKind {
    /// An endpoint that serves the named interface.
    Interface(Vec<String>),
    /// A memory object.
    Memory,
    /// A notification.
    Notification,
    /// A process.
    Process,
    /// A thread.
    Thread,
    /// Any capability.
    Any,
}

/// A type, as written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Type {
    /// An unsigned integer of this many bytes.
    Uint(u8),
    /// A signed integer of this many bytes.
    Int(u8),
    /// `bool`.
    Bool,
    /// `string<max>`: UTF-8, at most `max` bytes.
    String(u64),
    /// `bytes<max>`.
    Bytes(u64),
    /// `array<T, N>`.
    Array(Box<Type>, u64),
    /// `list<T, max>`.
    List(Box<Type>, u64),
    /// `option<T>`.
    Option(Box<Type>),
    /// `handle<kind>`.
    Handle(HandleKind),
    /// A struct, enum or bits type defined in the file.
    Named(Name),
}

/// A parameter or result, or a struct's field.
#[derive(Clone, Debug)]
pub struct Field {
    /// Its documentation.
    pub docs: Vec<String>,
    /// Its name.
    pub name: Name,
    /// Its type.
    pub ty: Type,
}

/// An operation.
#[derive(Clone, Debug)]
pub struct Function {
    /// Its documentation.
    pub docs: Vec<String>,
    /// Its number.
    pub op: u64,
    /// Where its number was written.
    pub op_pos: Pos,
    /// Its name.
    pub name: Name,
    /// Its parameters.
    pub params: Vec<Field>,
    /// Its results (none if there's no `->`).
    pub results: Vec<Field>,
    /// The enum its errors are drawn from, if it can fail.
    pub error: Option<Name>,
}

/// A value of an enum, or a bit of a bits type.
#[derive(Clone, Debug)]
pub struct Value {
    /// Its documentation.
    pub docs: Vec<String>,
    /// Its name.
    pub name: Name,
    /// Its number (for bits, the bit number).
    pub value: u64,
    /// Where the number was written.
    pub value_pos: Pos,
}

/// One item of the file.
#[derive(Clone, Debug)]
pub enum Item {
    /// `@N fn …`
    Function(Function),
    /// `reserved @N;`
    Reserved {
        /// The operation number.
        op: u64,
        /// Where.
        pos: Pos,
    },
    /// `struct Name { … }`
    Struct {
        /// Documentation.
        docs: Vec<String>,
        /// Its name.
        name: Name,
        /// Its fields.
        fields: Vec<Field>,
    },
    /// `[flexible] enum Name { … }`
    Enum {
        /// Documentation.
        docs: Vec<String>,
        /// Its name.
        name: Name,
        /// Whether unknown values are allowed.
        flexible: bool,
        /// Its values.
        values: Vec<Value>,
    },
    /// `bits Name: uN { … }`
    Bits {
        /// Documentation.
        docs: Vec<String>,
        /// Its name.
        name: Name,
        /// Its size in bytes.
        size: u8,
        /// Its bits.
        bits: Vec<Value>,
    },
    /// `const NAME: uN = value;`
    Const {
        /// Documentation.
        docs: Vec<String>,
        /// Its name.
        name: Name,
        /// Its size in bytes.
        size: u8,
        /// Its value.
        value: u64,
        /// Where the value was written.
        value_pos: Pos,
    },
}

/// A whole `.pbi` file.
#[derive(Clone, Debug)]
pub struct File {
    /// The interface's documentation.
    pub docs: Vec<String>,
    /// Its qualified name (`pbr.file` is `["pbr", "file"]`).
    pub name: Vec<Name>,
    /// Its major version.
    pub version: u64,
    /// Where the version was written.
    pub version_pos: Pos,
    /// Everything else, in order.
    pub items: Vec<Item>,
}

struct Parser {
    toks: Vec<(Tok, Pos)>,
    at: usize,
}

type R<T> = Result<T, Diagnostic>;

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.at].0
    }

    fn pos(&self) -> Pos {
        self.toks[self.at].1
    }

    fn next(&mut self) -> (Tok, Pos) {
        let t = self.toks[self.at].clone();
        if self.at + 1 < self.toks.len() {
            self.at += 1;
        }
        t
    }

    fn unexpected<T>(&self, wanted: &str) -> R<T> {
        Err(Diagnostic::new(
            self.pos(),
            format!("expected {wanted}, found {}", self.peek().describe()),
        ))
    }

    fn punct(&mut self, c: char) -> R<Pos> {
        if *self.peek() == Tok::Punct(c) {
            Ok(self.next().1)
        } else {
            self.unexpected(&format!("`{c}`"))
        }
    }

    fn is_punct(&self, c: char) -> bool {
        *self.peek() == Tok::Punct(c)
    }

    fn is_word(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == w)
    }

    fn word(&mut self, w: &str) -> R<()> {
        if self.is_word(w) {
            self.next();
            Ok(())
        } else {
            self.unexpected(&format!("`{w}`"))
        }
    }

    fn ident(&mut self, what: &str) -> R<Name> {
        match self.peek().clone() {
            Tok::Ident(text) => Ok(Name { text, pos: self.next().1 }),
            _ => self.unexpected(what),
        }
    }

    fn int(&mut self, what: &str) -> R<(u64, Pos)> {
        match *self.peek() {
            Tok::Int(v) => Ok((v, self.next().1)),
            _ => self.unexpected(what),
        }
    }

    fn docs(&mut self) -> Vec<String> {
        let mut d = Vec::new();
        while let Tok::Doc(text) = self.peek().clone() {
            d.push(text);
            self.next();
        }
        d
    }

    fn qname(&mut self) -> R<Vec<Name>> {
        let mut parts = vec![self.ident("a name")?];
        while self.is_punct('.') {
            self.next();
            parts.push(self.ident("a name after `.`")?);
        }
        Ok(parts)
    }

    fn uint_size(&mut self) -> R<u8> {
        let n = self.ident("`u8`, `u16`, `u32` or `u64`")?;
        match n.text.as_str() {
            "u8" => Ok(1),
            "u16" => Ok(2),
            "u32" => Ok(4),
            "u64" => Ok(8),
            _ => Err(Diagnostic::new(
                n.pos,
                format!("expected `u8`, `u16`, `u32` or `u64`, found `{}`", n.text),
            )),
        }
    }

    fn ty(&mut self) -> R<Type> {
        let n = self.ident("a type")?;
        Ok(match n.text.as_str() {
            "u8" => Type::Uint(1),
            "u16" => Type::Uint(2),
            "u32" => Type::Uint(4),
            "u64" => Type::Uint(8),
            "i8" => Type::Int(1),
            "i16" => Type::Int(2),
            "i32" => Type::Int(4),
            "i64" => Type::Int(8),
            "bool" => Type::Bool,
            "string" | "bytes" => {
                self.punct('<')?;
                let (max, _) = self.int("a maximum length")?;
                self.punct('>')?;
                if n.text == "string" { Type::String(max) } else { Type::Bytes(max) }
            }
            "array" | "list" => {
                self.punct('<')?;
                let inner = Box::new(self.ty()?);
                self.punct(',')?;
                let (count, _) =
                    self.int(if n.text == "array" { "a length" } else { "a maximum length" })?;
                self.punct('>')?;
                if n.text == "array" { Type::Array(inner, count) } else { Type::List(inner, count) }
            }
            "option" => {
                self.punct('<')?;
                let inner = Box::new(self.ty()?);
                self.punct('>')?;
                Type::Option(inner)
            }
            "handle" => {
                self.punct('<')?;
                let q = self.qname()?;
                self.punct('>')?;
                let kind = if q.len() == 1 {
                    match q[0].text.as_str() {
                        "memory" => HandleKind::Memory,
                        "notification" => HandleKind::Notification,
                        "process" => HandleKind::Process,
                        "thread" => HandleKind::Thread,
                        "any" => HandleKind::Any,
                        _ => {
                            return Err(Diagnostic::new(
                                q[0].pos,
                                "a handle's kind is `memory`, `notification`, `process`, `thread`, `any`, \
                                 or an interface's qualified name (such as `pbr.file`)",
                            ));
                        }
                    }
                } else {
                    HandleKind::Interface(q.into_iter().map(|n| n.text).collect())
                };
                Type::Handle(kind)
            }
            _ => Type::Named(n),
        })
    }

    /// `name: type` items separated by commas, up to `close`; a trailing comma is allowed.
    /// Docs are allowed before each when `docs` is set.
    fn fields(&mut self, close: char, docs: bool) -> R<Vec<Field>> {
        let mut out = Vec::new();
        loop {
            let d = if docs { self.docs() } else { Vec::new() };
            if self.is_punct(close) {
                if !d.is_empty() {
                    return self.unexpected("a field after `///`");
                }
                break;
            }
            let name = self.ident("a name")?;
            self.punct(':')?;
            let ty = self.ty()?;
            out.push(Field { docs: d, name, ty });
            if self.is_punct(',') {
                self.next();
            } else if !self.is_punct(close) {
                return self.unexpected(&format!("`,` or `{close}`"));
            }
        }
        self.punct(close)?;
        Ok(out)
    }

    /// `Name @N` items separated by commas, up to `}`.
    fn values(&mut self) -> R<Vec<Value>> {
        let mut out = Vec::new();
        loop {
            let docs = self.docs();
            if self.is_punct('}') {
                if !docs.is_empty() {
                    return self.unexpected("a value after `///`");
                }
                break;
            }
            let name = self.ident("a name")?;
            self.punct('@')?;
            let (value, value_pos) = self.int("a number")?;
            out.push(Value { docs, name, value, value_pos });
            if self.is_punct(',') {
                self.next();
            } else if !self.is_punct('}') {
                return self.unexpected("`,` or `}`");
            }
        }
        self.punct('}')?;
        Ok(out)
    }

    fn item(&mut self, docs: Vec<String>) -> R<Item> {
        if self.is_punct('@') {
            self.next();
            let (op, op_pos) = self.int("an operation number")?;
            self.word("fn")?;
            let name = self.ident("the operation's name")?;
            self.punct('(')?;
            let params = self.fields(')', false)?;
            let mut results = Vec::new();
            if *self.peek() == Tok::Arrow {
                self.next();
                self.punct('(')?;
                results = self.fields(')', false)?;
            }
            let mut error = None;
            if self.is_word("error") {
                self.next();
                error = Some(self.ident("an enum's name")?);
            }
            self.punct(';')?;
            return Ok(Item::Function(Function { docs, op, op_pos, name, params, results, error }));
        }
        let pos = self.pos();
        let Tok::Ident(w) = self.peek().clone() else {
            return self.unexpected(
                "an operation (`@N fn …`), `reserved`, `struct`, `enum`, `bits` or `const`",
            );
        };
        match w.as_str() {
            "reserved" => {
                // Documentation is allowed (a reserved line usually says why) but not kept.
                self.next();
                self.punct('@')?;
                let (op, pos) = self.int("an operation number")?;
                self.punct(';')?;
                Ok(Item::Reserved { op, pos })
            }
            "struct" => {
                self.next();
                let name = self.ident("the struct's name")?;
                self.punct('{')?;
                let fields = self.fields('}', true)?;
                Ok(Item::Struct { docs, name, fields })
            }
            "flexible" | "enum" => {
                self.next();
                let flexible = w == "flexible";
                if flexible {
                    self.word("enum")?;
                }
                let name = self.ident("the enum's name")?;
                self.punct('{')?;
                let values = self.values()?;
                Ok(Item::Enum { docs, name, flexible, values })
            }
            "bits" => {
                self.next();
                let name = self.ident("the bits type's name")?;
                self.punct(':')?;
                let size = self.uint_size()?;
                self.punct('{')?;
                let bits = self.values()?;
                Ok(Item::Bits { docs, name, size, bits })
            }
            "const" => {
                self.next();
                let name = self.ident("the constant's name")?;
                self.punct(':')?;
                let size = self.uint_size()?;
                self.punct('=')?;
                let (value, value_pos) = self.int("a number")?;
                self.punct(';')?;
                Ok(Item::Const { docs, name, size, value, value_pos })
            }
            _ => Err(Diagnostic::new(
                pos,
                format!(
                    "expected an operation (`@N fn …`), `reserved`, `struct`, `enum`, `bits` or `const`, found `{w}`"
                ),
            )),
        }
    }

    fn file(&mut self) -> R<File> {
        let docs = self.docs();
        self.word("interface")?;
        let name = self.qname()?;
        self.word("version")?;
        let (version, version_pos) = self.int("a version number")?;
        self.punct(';')?;
        let mut items = Vec::new();
        loop {
            let docs = self.docs();
            if *self.peek() == Tok::End {
                if !docs.is_empty() {
                    return self.unexpected("something to document after `///`");
                }
                break;
            }
            items.push(self.item(docs)?);
        }
        Ok(File { docs, name, version, version_pos, items })
    }
}

/// Parses a whole file.
pub fn parse(src: &str) -> Result<File, Diagnostic> {
    let toks = lex(src)?;
    Parser { toks, at: 0 }.file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        let t: Vec<Tok> = lex("/// doc\n// gone\n//// gone too\nfn x->(0x1F, 12)")
            .unwrap()
            .into_iter()
            .map(|t| t.0)
            .collect();
        assert_eq!(
            t,
            vec![
                Tok::Doc("doc".into()),
                Tok::Ident("fn".into()),
                Tok::Ident("x".into()),
                Tok::Arrow,
                Tok::Punct('('),
                Tok::Int(31),
                Tok::Punct(','),
                Tok::Int(12),
                Tok::Punct(')'),
                Tok::End
            ]
        );
        assert!(lex("12abc").is_err());
        assert!(lex("0x").is_err());
        assert!(lex("99999999999999999999").is_err());
        assert!(lex("a # b").is_err());
        assert!(lex("é").is_err());
    }

    #[test]
    fn positions() {
        let e = parse("interface a version 1;\n\n  @1 fn x(a u8);").unwrap_err();
        assert_eq!(e.pos, Pos { line: 3, col: 13 });
        assert!(e.message.contains("expected `:`"), "{}", e.message);
    }

    #[test]
    fn a_whole_file() {
        let f = parse(
            "/// An interface.\ninterface pbr.file version 2;\n\
             /// Reads.\n@1 fn read(offset: u64, max: u32,) -> (data: bytes<65536>) error E;\n\
             @2 fn ping();\nreserved @3;\n\
             struct S { a: option<list<array<i16, 3>, 4>>, b: handle<pbr.other>, }\n\
             flexible enum E { A @1, B @2 }\nbits B: u16 { X @0, Y @15, }\nconst C: u8 = 0xFF;",
        )
        .unwrap();
        assert_eq!(f.docs, vec!["An interface."]);
        assert_eq!(f.name.iter().map(|n| n.text.as_str()).collect::<Vec<_>>(), ["pbr", "file"]);
        assert_eq!(f.version, 2);
        assert_eq!(f.items.len(), 7);
        let Item::Function(read) = &f.items[0] else { panic!() };
        assert_eq!((read.op, read.params.len(), read.results.len()), (1, 2, 1));
        assert_eq!(read.error.as_ref().unwrap().text, "E");
        assert_eq!(read.docs, vec!["Reads."]);
        let Item::Function(ping) = &f.items[1] else { panic!() };
        assert!(ping.results.is_empty() && ping.error.is_none());
        let Item::Struct { fields, .. } = &f.items[3] else { panic!() };
        assert_eq!(
            fields[0].ty,
            Type::Option(Box::new(Type::List(Box::new(Type::Array(Box::new(Type::Int(2)), 3)), 4)))
        );
        assert_eq!(
            fields[1].ty,
            Type::Handle(HandleKind::Interface(vec!["pbr".into(), "other".into()]))
        );
        let Item::Enum { flexible, values, .. } = &f.items[4] else { panic!() };
        assert!(*flexible && values.len() == 2);
        let Item::Bits { size, bits, .. } = &f.items[5] else { panic!() };
        assert_eq!((*size, bits[1].value), (2, 15));
        let Item::Const { value, .. } = &f.items[6] else { panic!() };
        assert_eq!(*value, 255);
    }

    #[test]
    fn shapes_it_refuses() {
        for bad in [
            "",
            "interface a;",
            "interface a version 1",
            "interface a version 1; fn x();",
            "interface a version 1; @1 fn x() -> ;",
            "interface a version 1; @1 fn x(a: u8 b: u8);",
            "interface a version 1; @1 fn x(,);",
            "interface a version 1; struct S { a: u8 b: u8 }",
            "interface a version 1; enum E { A }",
            "interface a version 1; bits B: i8 { A @0 }",
            "interface a version 1; const C: bool = 1;",
            "interface a version 1; @1 fn x(a: handle<file>);",
            "interface a version 1; @1 fn x(a: string);",
            "interface a version 1; /// dangling",
            "interface a version 1; struct S { /// dangling\n }",
            "interface a version 1; widget W;",
        ] {
            assert!(parse(bad).is_err(), "{bad}");
        }
    }
}
