// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `pbr-interface-compiler [--layout] FILE.pbi…`: prints the Rust generated from each file
//! (or, with `--layout`, each message's layout). The generated code is MIT-licensed.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut layout = false;
    let mut files = Vec::new();
    for a in std::env::args().skip(1) {
        match a.as_str() {
            "--layout" => layout = true,
            "-h" | "--help" => {
                println!("usage: pbr-interface-compiler [--layout] FILE.pbi...");
                return ExitCode::SUCCESS;
            }
            _ => files.push(a),
        }
    }
    if files.is_empty() {
        eprintln!("usage: pbr-interface-compiler [--layout] FILE.pbi...");
        return ExitCode::FAILURE;
    }
    let mut out = if layout { String::new() } else { pbr_interface_compiler::rust::header() };
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{f}: {e}");
                return ExitCode::FAILURE;
            }
        };
        let name = std::path::Path::new(f)
            .file_name()
            .map_or(f.clone(), |n| n.to_string_lossy().into_owned());
        match pbr_interface_compiler::load(&src) {
            Ok(i) if layout => out.push_str(&pbr_interface_compiler::layout_report(&i)),
            Ok(i) => {
                out.push('\n');
                out.push_str(&pbr_interface_compiler::rust::generate(&i, &name));
            }
            Err(d) => {
                eprintln!("{f}:{d}");
                return ExitCode::FAILURE;
            }
        }
    }
    print!("{out}");
    ExitCode::SUCCESS
}
