// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Running PrettyBigRock in QEMU: by hand (`cargo xtask run`) and as automatic boot tests
//! (`cargo xtask qemu-test`).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The line the kernel prints when its start-up report is done (kernel/src/main.rs).
const REPORT_DONE: &str = "Start-up report complete.";

/// One way of booting.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// A short name for messages.
    pub name: &'static str,
    /// `x86_64` or `aarch64`.
    pub arch: &'static str,
    /// Start through the BIOS rather than UEFI (x86-64 only).
    pub bios: bool,
    /// Start the ARM processor at EL2 (tests the EL2 stub).
    pub el2: bool,
    /// Lines the kernel's report must contain.
    pub expect: &'static [&'static str],
}

/// Every configuration the boot tests run.
pub const TESTS: &[Config] = &[
    Config {
        name: "x86-64, UEFI",
        arch: "x86_64",
        bios: false,
        el2: false,
        expect: &[
            "on x86-64, started by UEFI.",
            "Cores:         4 found, 4 running",
            "Screens:       1 (",
            "Randomness:    seeded",
            "Self-test passed",
        ],
    },
    Config {
        name: "x86-64, BIOS",
        arch: "x86_64",
        bios: true,
        el2: false,
        expect: &[
            "on x86-64, started by BIOS.",
            "Cores:         4 found, 4 running",
            "Screens:       1 (",
            "Time:          2",
            "Self-test passed",
        ],
    },
    Config {
        name: "ARM64, UEFI",
        arch: "aarch64",
        bios: false,
        el2: false,
        expect: &[
            "on ARM64, started by UEFI.",
            "exception level 1",
            "Cores:         4 found, 4 running",
            "Screens:       1 (",
            "Randomness:    seeded",
            "Self-test passed",
        ],
    },
    Config {
        name: "ARM64, UEFI, starting at EL2",
        arch: "aarch64",
        bios: false,
        el2: true,
        expect: &[
            "exception level 1",
            "Cores:         4 found, 4 running",
            "EL2 stub:      present and answering",
            "Self-test passed",
        ],
    },
];

fn first_existing(paths: &[&str]) -> Option<PathBuf> {
    paths.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// The QEMU command for a configuration, with the firmware found on this computer.
fn command(cfg: &Config, image: &Path, work: &Path) -> Result<Command, String> {
    let drive = format!("format=raw,file={},snapshot=on", image.display());
    let mut cmd;
    if cfg.arch == "x86_64" {
        cmd = Command::new("qemu-system-x86_64");
        cmd.args(["-machine", "q35", "-m", "512", "-smp", "4"]);
        if !cfg.bios {
            let code = first_existing(&[
                "/usr/share/OVMF/OVMF_CODE_4M.fd",
                "/usr/share/OVMF/OVMF_CODE.fd",
                "/usr/share/ovmf/OVMF.fd",
                "/usr/share/edk2/x64/OVMF_CODE.fd",
                "/usr/share/qemu/edk2-x86_64-code.fd",
            ])
            .ok_or("can't find OVMF (UEFI for QEMU): install the `ovmf` package")?;
            cmd.arg("-drive")
                .arg(format!("if=pflash,format=raw,readonly=on,file={}", code.display()));
            let vars_src = first_existing(&[
                "/usr/share/OVMF/OVMF_VARS_4M.fd",
                "/usr/share/OVMF/OVMF_VARS.fd",
            ]);
            if let Some(src) = vars_src {
                let vars = work.join("ovmf-vars.fd");
                fs::copy(src, &vars).map_err(|e| e.to_string())?;
                cmd.arg("-drive").arg(format!("if=pflash,format=raw,file={}", vars.display()));
            }
        }
        cmd.arg("-drive").arg(drive);
        cmd.args(["-device", "virtio-rng-pci"]);
    } else {
        cmd = Command::new("qemu-system-aarch64");
        let machine = if cfg.el2 { "virt,virtualization=on" } else { "virt" };
        cmd.args(["-machine", machine, "-cpu", "cortex-a72", "-m", "512", "-smp", "4"]);
        let fw = first_existing(&[
            "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
            "/usr/share/AAVMF/AAVMF_CODE.fd",
            "/usr/share/edk2/aarch64/QEMU_EFI.fd",
            "/usr/share/qemu/edk2-aarch64-code.fd",
        ])
        .ok_or("can't find UEFI for ARM64 QEMU: install the `qemu-efi-aarch64` package")?;
        cmd.arg("-bios").arg(fw);
        cmd.args(["-device", "ramfb", "-device", "qemu-xhci", "-device", "usb-kbd"]);
        cmd.arg("-drive").arg(format!("if=none,id=disk,{drive}"));
        cmd.args(["-device", "virtio-blk-pci,drive=disk", "-device", "virtio-rng-pci"]);
    }
    cmd.arg("-no-reboot");
    Ok(cmd)
}

/// Runs one configuration in a window, for a person to watch.
pub fn run(cfg: &Config, image: &Path, work: &Path) -> Result<(), String> {
    let mut cmd = command(cfg, image, work)?;
    cmd.args(["-serial", "stdio"]);
    let st = cmd.status().map_err(|e| format!("couldn't start QEMU: {e}"))?;
    if st.success() { Ok(()) } else { Err("QEMU failed".into()) }
}

/// Boots one configuration without a window, and waits for the kernel's start-up report on the
/// serial port. Returns what the kernel printed.
pub fn test(cfg: &Config, image: &Path, work: &Path, limit: Duration) -> Result<String, String> {
    let log = work.join(format!(
        "serial-{}-{}{}.txt",
        cfg.arch,
        if cfg.bios { "bios" } else { "uefi" },
        if cfg.el2 { "-el2" } else { "" }
    ));
    let _ = fs::remove_file(&log);
    let mut cmd = command(cfg, image, work)?;
    cmd.args(["-display", "none", "-monitor", "none"]);
    cmd.arg("-serial").arg(format!("file:{}", log.display()));
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start QEMU: {e}"))?;
    let start = Instant::now();
    let result = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        if text.contains(REPORT_DONE) {
            break Ok(text);
        }
        if let Ok(Some(st)) = child.try_wait() {
            break Err(format!(
                "QEMU stopped ({st}) before the kernel finished.\n--- serial output ---\n{text}"
            ));
        }
        if start.elapsed() > limit {
            break Err(format!(
                "no start-up report after {} seconds.\n--- serial output ---\n{text}",
                limit.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = child.kill();
    let _ = child.wait();
    let text = result?.replace('\r', "");
    let missing: Vec<&str> = cfg.expect.iter().copied().filter(|e| !text.contains(e)).collect();
    if missing.is_empty() {
        Ok(text)
    } else {
        Err(format!("the report is missing {missing:?}.\n--- serial output ---\n{text}"))
    }
}
