// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `memfs`: a filesystem server whose filesystem is in memory, built exactly as real ones are
//! (`lib/fsserve` and a `Filesystem`), so the files self-test can check `pbr.file`, derived
//! capabilities and records between real programs before RockFS's server exists
//! (docs/design/0018-supervision-shell-and-m5-tests.md, section 7, test 3).
//!
//! Start-up roles: its own endpoint (`OWN`, every right) and its parent's `pbr.fs.host`
//! (`PARENT`).

#![no_std]
#![no_main]

use granite::endpoint::Endpoint;
use granite::process::role;
use pbr_fsserve::memfs::MemFs;

granite::main!(main);

fn main(mut start: granite::Start) -> i32 {
    let (Some(own), Some(parent)) = (start.take(role::OWN), start.take(role::PARENT)) else {
        granite::println!("memfs: started without its endpoints");
        return 1;
    };
    let mut fs = MemFs::new("Test");
    fs.clock = granite::time::wall_seconds;
    pbr_fsserve::serve::run(fs, Endpoint(own), Endpoint(parent), "Test")
}
