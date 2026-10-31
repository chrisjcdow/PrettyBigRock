// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The interface compiler: reads `.pbi` interface files and generates Rust
//! (docs/design/0008-messages-and-interfaces.md, sections 7 to 9).
//!
//! - [`syntax`] reads a file into a syntax tree.
//! - [`check`] enforces the rules and works out every message's layout.
//! - [`rust`] generates the Rust module.
//!
//! It uses only Rust's standard library, so it runs anywhere Rust does. `lib/interfaces` runs
//! it from its build script. The compiler is GPL-3.0-or-later; what it generates is under the
//! MIT licence, like the interface files themselves.

pub mod check;
pub mod rust;
pub mod syntax;

use std::fmt;
use std::fmt::Write;

/// A problem with an interface file, and where it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    /// Where.
    pub pos: syntax::Pos,
    /// What's wrong.
    pub message: String,
}

impl Diagnostic {
    /// A problem at `pos`.
    pub fn new(pos: syntax::Pos, message: impl Into<String>) -> Self {
        Diagnostic { pos, message: message.into() }
    }
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}: {}", self.pos.line, self.pos.col, self.message)
    }
}

/// Reads and checks an interface file's text.
pub fn load(src: &str) -> Result<check::Interface, Diagnostic> {
    check::check(&syntax::parse(src)?)
}

/// Compiles an interface file's text to a Rust module. `source` names the file in the
/// generated documentation.
pub fn compile(src: &str, source: &str) -> Result<String, Diagnostic> {
    Ok(rust::generate(&load(src)?, source))
}

/// The layouts of an interface, one line each, for checking against an independent
/// calculation (`tools/pbi_check.py`):
///
/// ```text
/// interface pbr.file 1
/// struct Name <size> <align>
/// op <name> <number> <request fixed> <request buffer> <request caps> <reply fixed> <reply buffer> <reply caps>
/// ```
pub fn layout_report(i: &check::Interface) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "interface {} {}", i.name.join("."), i.version);
    for t in &i.types {
        if let check::TypeDef::Struct { name, size, align, .. } = t {
            let _ = writeln!(s, "struct {name} {size} {align}");
        }
    }
    for o in &i.ops {
        let (q, r) = (o.request, o.reply);
        let _ = writeln!(
            s,
            "op {} {} {} {} {} {} {} {}",
            o.name, o.op, q.fixed, q.var, q.caps, r.fixed, r.var, r.caps
        );
    }
    s
}
