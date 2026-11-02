// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `rockfs-rs`: RockFS disk images from the command line, using the same Rust code the system
//! itself uses (`lib/rockfs`). It does what `tools/rockfs.py` does, with the same arguments and
//! output, so the two can be compared, and adds `stress` and `verify` for
//! `tools/rockfs_cross.py`.
//!
//! ```text
//! rockfs-rs IMAGE mkfs [--size N[K|M|G]] [--label L] [--whole-disk]
//! rockfs-rs IMAGE info
//! rockfs-rs IMAGE ls [-R] [PATH]
//! rockfs-rs IMAGE cat PATH
//! rockfs-rs IMAGE get PATH [DEST]
//! rockfs-rs IMAGE put SOURCE PATH
//! rockfs-rs IMAGE mkdir PATH
//! rockfs-rs IMAGE rm PATH
//! rockfs-rs IMAGE mv FROM TO
//! rockfs-rs IMAGE check [--repair]
//! rockfs-rs IMAGE stress SEED ROUNDS MANIFEST
//! rockfs-rs IMAGE verify [--exact] MANIFEST
//! ```
//!
//! Exit codes: 0 for success, 1 for an error (the message goes to standard error as
//! "rockfs: ..."), 2 for `check` finding problems that remain.

mod commands;
mod disk;
mod stress;
mod tree;
mod util;

use std::process::ExitCode;

const USAGE: &str = "usage: rockfs-rs IMAGE COMMAND [ARGS]

commands: mkfs [--size N[K|M|G]] [--label L] [--whole-disk]
          info | ls [-R] [PATH] | cat PATH | get PATH [DEST] | put SOURCE PATH
          mkdir PATH | rm PATH | mv FROM TO | check [--repair]
          stress SEED ROUNDS MANIFEST | verify [--exact] MANIFEST";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        // Like the Python tool: help is not an error when asked for.
        println!("{USAGE}");
        return ExitCode::from(u8::from(!args.get(1).is_some_and(|a| a == "-h" || a == "--help")));
    }
    let (image, command, rest) = (&args[1], args[2].as_str(), &args[3..]);
    let outcome = match command {
        "mkfs" => commands::mkfs(image, rest),
        "info" => commands::info(image),
        "ls" => commands::ls(image, rest),
        "cat" => commands::cat(image, rest),
        "get" => commands::get(image, rest),
        "put" => commands::put(image, rest),
        "mkdir" => commands::mkdir(image, rest),
        "rm" => commands::rm(image, rest),
        "mv" => commands::mv(image, rest),
        "check" => commands::check(image, rest),
        "stress" => stress::stress(image, rest),
        "verify" => stress::verify(image, rest),
        other => Err(format!("unknown command \"{other}\"")),
    };
    match outcome {
        Ok(code) => ExitCode::from(code as u8),
        Err(message) => {
            eprintln!("rockfs: {message}");
            ExitCode::from(1)
        }
    }
}
