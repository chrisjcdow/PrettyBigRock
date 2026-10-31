// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

//! Compiles every interface file in `interfaces/` into one Rust file in `OUT_DIR`.

use std::path::Path;
use std::{env, fs};

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../interfaces");
    println!("cargo::rerun-if-changed={}", dir.display());
    let mut files: Vec<_> = fs::read_dir(&dir)
        .expect("the interfaces folder")
        .map(|e| e.expect("a folder entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "pbi"))
        .collect();
    files.sort();
    let mut out = pbr_interface_compiler::rust::header();
    for f in &files {
        println!("cargo::rerun-if-changed={}", f.display());
        let name = f.file_name().expect("a file name").to_string_lossy().into_owned();
        let src = fs::read_to_string(f).expect("an interface file");
        match pbr_interface_compiler::compile(&src, &name) {
            Ok(code) => {
                out.push('\n');
                out.push_str(&code);
            }
            Err(d) => panic!("interfaces/{name}:{d}"),
        }
    }
    let dest = Path::new(&env::var("OUT_DIR").expect("OUT_DIR")).join("interfaces.rs");
    fs::write(dest, out).expect("writing the generated code");
}
