// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Picks the linker script for the architecture being built.

fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").expect("cargo sets CARGO_CFG_TARGET_ARCH");
    let dir = std::env::var("CARGO_MANIFEST_DIR").expect("cargo sets CARGO_MANIFEST_DIR");
    let script = format!("{dir}/src/arch/{arch}/linker.ld");
    println!("cargo:rerun-if-changed={script}");
    println!("cargo:rustc-link-arg-bins=-T{script}");
    println!("cargo:rustc-link-arg-bins=--no-pie");
    println!("cargo:rustc-link-arg-bins=-zmax-page-size=0x1000");
}
