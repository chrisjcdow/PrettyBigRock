// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Links the BIOS loader at the low addresses the BIOS loads it to.

fn main() {
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    for f in ["linker.ld", "src/stage1.s", "src/stage2.s"] {
        println!("cargo:rerun-if-changed={dir}/{f}");
    }
    println!("cargo:rustc-link-arg-bins=-T{dir}/linker.ld");
    println!("cargo:rustc-link-arg-bins=--no-pie");
}
