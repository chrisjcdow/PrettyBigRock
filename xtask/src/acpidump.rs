// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `cargo xtask acpidump`: saves QEMU's ACPI tables, as the boot tests' machines have them, for
//! the AML interpreter's host tests (docs/design/0028-acpi.md, section 10).
//!
//! It boots the stick image, with `acpidump` on the kernel's command line, in the first x86-64
//! configuration (QEMU's `q35`) and the first ARM64 one (`virt`). The kernel writes every table
//! to the serial log in hexadecimal and then carries on; the tool stops QEMU when the kernel
//! says it is done, and writes each table to `tests/data/acpi/qemu-q35/` or `qemu-virt/`, named
//! as Linux's `/sys/firmware/acpi/tables` names them (the SSDTs `SSDT1`, `SSDT2` and so on).

use crate::qemu;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Boots each machine and saves its tables.
pub fn run(root: &Path, stick: &Path, work: &Path) -> Result<(), String> {
    for (arch, folder) in [("x86_64", "qemu-q35"), ("aarch64", "qemu-virt")] {
        let cfg = qemu::TESTS
            .iter()
            .find(|c| c.arch == arch && !c.bios && !c.el2)
            .ok_or("no configuration to boot")?;
        let text = boot(cfg, stick, work)?;
        let tables = parse(&text)?;
        let dir = root.join("tests/data/acpi").join(folder);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for (name, bytes) in &tables {
            fs::write(dir.join(name), bytes).map_err(|e| format!("writing {name}: {e}"))?;
        }
        let names: Vec<&str> = tables.keys().map(String::as_str).collect();
        println!("acpidump: {} ({}): {}", folder, cfg.name, names.join(" "));
    }
    Ok(())
}

/// Boots `cfg` until the kernel has written its tables; returns the serial log.
fn boot(cfg: &qemu::Config, image: &Path, work: &Path) -> Result<String, String> {
    let log = work.join(format!("acpidump-{}.txt", cfg.arch));
    let _ = fs::remove_file(&log);
    let mut cmd = qemu::command(cfg, image, work, true)?;
    cmd.args(["-display", "none", "-monitor", "none"]);
    cmd.arg("-serial").arg(format!("file:{}", log.display()));
    // A second serial port, as the boot tests give x86-64 (QEMU describes only the ports the
    // machine has, so the tables would otherwise have no COM2).
    if cfg.arch == "x86_64" {
        cmd.args(["-serial", "null"]);
    }
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start QEMU: {e}"))?;
    let start = Instant::now();
    let result = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        if text.contains("acpidump: done") {
            break Ok(text);
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(format!("QEMU stopped ({status}) before the tables were written"));
        }
        if start.elapsed() > Duration::from_secs(300) {
            break Err("the tables weren't written within 5 minutes".into());
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = child.kill();
    let _ = child.wait();
    result
}

/// Reads the kernel's lines (`acpidump: SIGN offset: hex`) back into tables. A table starts
/// again at offset 0; a second table with the same signature is numbered, as Linux numbers
/// SSDTs.
fn parse(text: &str) -> Result<BTreeMap<String, Vec<u8>>, String> {
    let mut found: Vec<(String, Vec<u8>)> = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.split("acpidump: ").nth(1) else { continue };
        let Some((sig, rest)) = rest.split_once(' ') else { continue };
        let Some((offset, hex)) = rest.split_once(": ") else { continue };
        let offset = usize::from_str_radix(offset, 16).map_err(|_| format!("bad line: {line}"))?;
        let bytes: Vec<u8> = (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16))
            .collect::<Result<_, _>>()
            .map_err(|_| format!("bad line: {line}"))?;
        if offset == 0 {
            found.push((sig.to_string(), Vec::new()));
        }
        let Some((s, t)) = found.last_mut() else {
            return Err(format!("a table without its start: {line}"));
        };
        if s != sig || t.len() != offset {
            return Err(format!("lines out of order at: {line}"));
        }
        t.extend(bytes);
    }
    let mut out = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let total = |sig: &str| found.iter().filter(|(s, _)| s == sig).count();
    for (sig, bytes) in &found {
        let n = counts.entry(sig.clone()).or_default();
        *n += 1;
        let name = if total(sig) > 1 || sig == "SSDT" { format!("{sig}{n}") } else { sig.clone() };
        out.insert(name, bytes.clone());
    }
    if out.is_empty() {
        return Err("the kernel wrote no tables".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn tables_are_read_back() {
        let log = "PrettyBigRock\n\
            acpidump: FACP 00000: 46414350\n\
            acpidump: SSDT 00000: 0102\n\
            acpidump: SSDT 00000: 03\n\
            acpidump: DSDT 00000: 44534454\n\
            acpidump: DSDT 00004: 0506\n\
            acpidump: done\n";
        let t = parse(log).unwrap();
        assert_eq!(t["FACP"], b"FACP");
        assert_eq!(t["SSDT1"], [1, 2]);
        assert_eq!(t["SSDT2"], [3]);
        assert_eq!(t["DSDT"], [0x44, 0x53, 0x44, 0x54, 5, 6]);
        assert!(parse("acpidump: DSDT 00004: 05\n").is_err());
    }
}
