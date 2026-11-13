// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Running PrettyBigRock in QEMU: by hand (`cargo xtask run`) and as automatic boot tests
//! (`cargo xtask qemu-test`).

use crate::pictures;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The line the kernel prints when its start-up report is done (kernel/src/main.rs).
const REPORT_DONE: &str = "Start-up report complete.";
/// The root program's line when every program self-test has passed.
const SELFTEST_PASSED: &str = "Program self-test passed";
/// The kernel's line when `machine_power` is about to restart the machine.
const RESTARTING: &str = "Restarting";

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
    /// How many processor cores.
    pub cores: u32,
    /// The processor model, if not QEMU's usual one for the machine. `max` turns on every
    /// feature QEMU can emulate, which tests the paths that use them (SMEP, SMAP and XSAVE on
    /// x86-64; PAN on ARM64).
    pub cpu: Option<&'static str>,
    /// Lines the kernel's report must contain.
    pub expect: &'static [&'static str],
    /// Which screens and pointers it has for the desktop (note 0026, section 6).
    pub screens: Screens,
}

/// The screens a configuration has (note 0026, section 6): the firmware's (QEMU's standard
/// VGA, or `ramfb` on ARM64), which the framebuffer driver serves, or a virtio-gpu's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screens {
    /// Only the firmware's screen, with no tablet: the PS/2 mouse is the pointer to test.
    Firmware,
    /// The firmware's screen, and a tablet (not tied to it; see [`command`]).
    FirmwareWithTablet,
    /// No firmware screen: a virtio-gpu with two screens, a tablet tied to each. Nothing is
    /// drawn until its driver runs, so the kernel's own messages are only on the serial port.
    TwoVirtioGpu,
}

/// Every configuration the boot tests run.
pub const TESTS: &[Config] = &[
    Config {
        name: "x86-64, UEFI",
        arch: "x86_64",
        bios: false,
        el2: false,
        cores: 4,
        cpu: Some("max"),
        expect: &[
            "on x86-64, started by UEFI.",
            "XSAVE, SMEP, SMAP",
            "Cores:         4 found, 4 running",
            "Screens:       0",
            "Randomness:    seeded",
            "Self-test passed",
            "Program self-test passed",
            "a block device (2048 sectors, reads, writes and refusals)",
            "a driver crashed and restarted",
            "Device self-test passed",
            "Sandbox self-test passed",
            "Supervision self-test passed",
            "Input self-test passed",
            "Shell self-test passed",
            "the screens",
            "a virtio keyboard and mouse",
            "a full ring",
            "COM2 (output and loopback)",
            "a PS/2 keyboard and mouse",
            "receiver watches and the machine capability",
            "Restarting",
        ],
        screens: Screens::TwoVirtioGpu,
    },
    Config {
        name: "x86-64, BIOS",
        arch: "x86_64",
        bios: true,
        el2: false,
        cores: 4,
        cpu: None,
        expect: &[
            "on x86-64, started by BIOS.",
            "FXSAVE",
            "Cores:         4 found, 4 running",
            "Screens:       1 (",
            "Time:          2",
            "Self-test passed",
            "Program self-test passed",
            "a block device (2048 sectors, reads, writes and refusals)",
            "a driver crashed and restarted",
            "Device self-test passed",
            "Sandbox self-test passed",
            "Supervision self-test passed",
            "Input self-test passed",
            "Shell self-test passed",
            "the screens",
            "a virtio keyboard and mouse",
            "a full ring",
            "COM2 (output and loopback)",
            "a PS/2 keyboard and mouse",
            "receiver watches and the machine capability",
            "Restarting",
        ],
        screens: Screens::Firmware,
    },
    Config {
        name: "ARM64, UEFI",
        arch: "aarch64",
        bios: false,
        el2: false,
        cores: 4,
        cpu: None,
        expect: &[
            "on ARM64, started by UEFI.",
            "exception level 1",
            "Cores:         4 found, 4 running",
            "Screens:       1 (",
            "Randomness:    seeded",
            "Self-test passed",
            "Program self-test passed",
            "a block device (2048 sectors, reads, writes and refusals)",
            "a driver crashed and restarted",
            "Device self-test passed",
            "Sandbox self-test passed",
            "Supervision self-test passed",
            "Input self-test passed",
            "Shell self-test passed",
            "the screens",
            "a virtio keyboard and mouse",
            "a full ring",
            "receiver watches and the machine capability",
            "Restarting",
        ],
        screens: Screens::FirmwareWithTablet,
    },
    Config {
        name: "ARM64, UEFI, starting at EL2",
        arch: "aarch64",
        bios: false,
        el2: true,
        cores: 4,
        cpu: Some("max"),
        expect: &[
            "exception level 1",
            ", PAN",
            "Cores:         4 found, 4 running",
            "EL2 stub:      present and answering",
            "Self-test passed",
            "Program self-test passed",
            "a block device (2048 sectors, reads, writes and refusals)",
            "a driver crashed and restarted",
            "Device self-test passed",
            "Sandbox self-test passed",
            "Supervision self-test passed",
            "Input self-test passed",
            "Shell self-test passed",
            "the screens",
            "a virtio keyboard and mouse",
            "a full ring",
            "receiver watches and the machine capability",
            "Restarting",
        ],
        screens: Screens::TwoVirtioGpu,
    },
];

fn first_existing(paths: &[&str]) -> Option<PathBuf> {
    paths.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// The sectors on the test disk.
const TEST_DISK_SECTORS: u64 = 2048;

/// Writes the disk the block device test reads and writes (`snapshot=on` throws the writes
/// away). What each byte is must match `pattern` in `services/root/src/blocktest.rs`.
fn test_disk(work: &Path) -> Result<PathBuf, String> {
    let path = work.join("test-disk.img");
    let mut data = Vec::with_capacity(TEST_DISK_SECTORS as usize * 512);
    for s in 0..TEST_DISK_SECTORS {
        for i in 0..512usize {
            data.push(
                (s.wrapping_mul(31) as u8).wrapping_add((i as u8).wrapping_mul(7)).wrapping_add(3),
            );
        }
    }
    fs::write(&path, data).map_err(|e| format!("writing the test disk: {e}"))?;
    Ok(path)
}

/// A short name for a configuration, for its files: `x86_64-uefi`, `aarch64-uefi-el2`.
pub(crate) fn tag(cfg: &Config) -> String {
    format!(
        "{}-{}{}",
        cfg.arch,
        if cfg.bios { "bios" } else { "uefi" },
        if cfg.el2 { "-el2" } else { "" }
    )
}

/// A fresh copy of the data disk 1.3 made (`tests/data/classic-1.3.img.gz`) for one boot. It is
/// attached writable, not as a snapshot, so the disk can be checked afterwards
/// ([`check_classic`]).
fn classic_copy(cfg: &Config, work: &Path) -> Result<PathBuf, String> {
    let gz = Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/data/classic-1.3.img.gz");
    let out = Command::new("gzip")
        .arg("-dc")
        .arg(&gz)
        .output()
        .map_err(|e| format!("couldn't run gzip: {e}"))?;
    if !out.status.success() {
        return Err(format!("gzip couldn't read {}", gz.display()));
    }
    let copy = work.join(format!("classic-{}.img", tag(cfg)));
    fs::write(&copy, &out.stdout).map_err(|e| format!("{}: {e}", copy.display()))?;
    Ok(copy)
}

/// After a boot: the data disk 1.3 made must have been shut down cleanly, with the self-test's
/// changes and nothing else, as `tools/check_classic_after.py` checks with the Python tool and
/// (given `rfs_host`) 1.3's own code.
pub fn check_classic(cfg: &Config, work: &Path, rfs_host: Option<&Path>) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut cmd = Command::new("python3");
    cmd.arg(root.join("tools/check_classic_after.py"))
        .arg(work.join(format!("classic-{}.img", tag(cfg))))
        .arg(root.join("tests/data/classic-1.3.manifest"));
    if let Some(host) = rfs_host {
        cmd.arg(host);
    }
    let out = cmd.output().map_err(|e| format!("couldn't run python3: {e}"))?;
    let text =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("the disk 1.3 made, after the boot:\n{text}"))
    }
}

/// After a boot: the MBR test disk's FAT32 volume must pass `fsck.fat -n` and read back what
/// the self-test wrote, as `tools/check_fat_after.py` checks with `mtools`.
pub fn check_fat(work: &Path) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let (start, sectors) = crate::testdisks::MBR_FAT;
    let out = Command::new("python3")
        .arg(root.join("tools/check_fat_after.py"))
        .arg(work.join(crate::testdisks::MBR_DISK))
        .arg(start.to_string())
        .arg(sectors.to_string())
        .output()
        .map_err(|e| format!("couldn't run python3: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the FAT32 test volume, after the boot:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// After a boot: the system volume the shell test changed must have been shut down cleanly,
/// with the shell's changes on it, as `tools/check_shell_after.py` checks.
pub fn check_shell(work: &Path) -> Result<(), String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = Command::new("python3")
        .arg(root.join("tools/check_shell_after.py"))
        .arg(work.join(crate::testdisks::GPT_DISK))
        .output()
        .map_err(|e| format!("couldn't run python3: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "the system volume, after the shell test:\n{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// The QEMU command for a configuration, with the firmware found on this computer.
///
/// `headless` is a boot test: nobody is looking, so on the configurations with a virtio-gpu
/// the test runner stands in for the viewer (see [`Screens::TwoVirtioGpu`]).
pub(crate) fn command(
    cfg: &Config,
    image: &Path,
    work: &Path,
    headless: bool,
) -> Result<Command, String> {
    command_on(cfg, image, work, headless, true)
}

/// [`command`], on the disks the last boot left if `fresh` is false (the desktop boot starts
/// the same disks again after a restart, note 0026, section 6).
pub(crate) fn command_on(
    cfg: &Config,
    image: &Path,
    work: &Path,
    headless: bool,
    fresh: bool,
) -> Result<Command, String> {
    let firmware_screen = !matches!(cfg.screens, Screens::TwoVirtioGpu);
    let drive = format!("format=raw,file={},snapshot=on", image.display());
    let mut cmd;
    if cfg.arch == "x86_64" {
        cmd = Command::new("qemu-system-x86_64");
        cmd.args(["-machine", "q35", "-m", MEMORY_MIB]);
        if let Some(cpu) = cfg.cpu {
            cmd.args(["-cpu", cpu]);
        }
        cmd.arg("-smp").arg(cfg.cores.to_string());
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
        // The screen has a name so that the keyboard and mouse below can be tied to it: the
        // test runner then sends them keys without the PS/2 pair getting any (and the other
        // way round).
        cmd.args(["-vga", "none"]);
        if firmware_screen {
            cmd.args(["-device", "VGA,id=gpu"]);
        }
    } else {
        cmd = Command::new("qemu-system-aarch64");
        let machine = if cfg.el2 { "virt,virtualization=on" } else { "virt" };
        cmd.args(["-machine", machine, "-cpu", cfg.cpu.unwrap_or("cortex-a72"), "-m", MEMORY_MIB]);
        cmd.arg("-smp").arg(cfg.cores.to_string());
        let fw = first_existing(&[
            "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
            "/usr/share/AAVMF/AAVMF_CODE.fd",
            "/usr/share/edk2/aarch64/QEMU_EFI.fd",
            "/usr/share/qemu/edk2-aarch64-code.fd",
        ])
        .ok_or("can't find UEFI for ARM64 QEMU: install the `qemu-efi-aarch64` package")?;
        cmd.arg("-bios").arg(fw);
        if firmware_screen {
            cmd.args(["-device", "ramfb,id=gpu"]);
        }
        cmd.args(["-device", "qemu-xhci", "-device", "usb-kbd"]);
        cmd.arg("-drive").arg(format!("if=none,id=disk,{drive}"));
        cmd.args(["-device", "virtio-blk-pci,drive=disk", "-device", "virtio-rng-pci"]);
    }
    let disk = if fresh { test_disk(work)? } else { work.join("test-disk.img") };
    cmd.arg("-drive")
        .arg(format!("if=none,id=testdisk,format=raw,file={},snapshot=on", disk.display()));
    cmd.args(["-device", "virtio-blk-pci,drive=testdisk"]);
    // The storage service's test disks (note 0018, section 7, test 2).
    // The MBR and GPT disks are attached writable, so their FAT32 volume and system volume
    // can be checked after the boot ([`check_fat`], [`check_shell`]); `make` remakes them for
    // each boot.
    let disks = if fresh { crate::testdisks::make(work)? } else { crate::testdisks::paths(work) };
    for (i, disk) in disks.iter().enumerate() {
        let writable = disk.ends_with(crate::testdisks::MBR_DISK)
            || disk.ends_with(crate::testdisks::GPT_DISK);
        let snapshot = if writable { "" } else { ",snapshot=on" };
        cmd.arg("-drive")
            .arg(format!("if=none,id=storage{i},format=raw,file={}{snapshot}", disk.display()));
        cmd.arg("-device").arg(format!("virtio-blk-pci,drive=storage{i}"));
    }
    let classic = if fresh {
        classic_copy(cfg, work)?
    } else {
        work.join(format!("classic-{}.img", tag(cfg)))
    };
    cmd.arg("-drive").arg(format!("if=none,id=classic,format=raw,file={}", classic.display()));
    cmd.args(["-device", "virtio-blk-pci,drive=classic"]);
    // The virtio keyboard and mouse are tied to a screen, so that the runner can send them
    // keys without the PS/2 pair getting any (and the other way round): see [`input_screen`].
    // The devices come in this order because devices are found in the order they are given,
    // and M4's driver test uses the first virtio pointer it finds.
    let (device, head) = input_screen(cfg, headless);
    if matches!(cfg.screens, Screens::TwoVirtioGpu) {
        // The only screens there are (note 0020): a virtio-gpu with two outputs. In a boot
        // test it has a third, which never has anything plugged in: it is there for the
        // keyboard and mouse to be tied to, because QEMU sends a screen's clicks to the last
        // pointer tied to it, and the first two screens each have a tablet.
        let outputs = if headless { 3 } else { 2 };
        cmd.arg("-device").arg(format!("virtio-gpu-pci,id=gpu2,max_outputs={outputs}"));
    }
    if firmware_screen || headless {
        cmd.arg("-device").arg(format!("virtio-keyboard-pci,id=vkbd,display={device},head={head}"));
        cmd.arg("-device").arg(format!("virtio-mouse-pci,id=vmouse,display={device},head={head}"));
    } else {
        cmd.args([
            "-device",
            "virtio-keyboard-pci,id=vkbd",
            "-device",
            "virtio-mouse-pci,id=vmouse",
        ]);
    }
    // M6's tablets (note 0026, section 6). QEMU ties a tablet to one screen (`display` and
    // `head`), as a touch screen is.
    match cfg.screens {
        Screens::Firmware => {}
        Screens::FirmwareWithTablet => {
            // Not tied to the screen: the virtio mouse is, and its clicks would go to the
            // tablet instead. An untied tablet gets only what is sent to no particular screen.
            cmd.args(["-device", "virtio-tablet-pci,id=tablet0"]);
        }
        Screens::TwoVirtioGpu => {
            cmd.args(["-device", "virtio-tablet-pci,id=tablet0,display=gpu2,head=0"]);
            cmd.args(["-device", "virtio-tablet-pci,id=tablet1,display=gpu2,head=1"]);
        }
    }
    cmd.arg("-no-reboot");
    Ok(cmd)
}

/// The QEMU screen the virtio keyboard and mouse are tied to, and through which the runner
/// reaches them: its device's name and which of its outputs.
pub(crate) fn input_screen(cfg: &Config, headless: bool) -> (&'static str, u32) {
    match cfg.screens {
        Screens::TwoVirtioGpu if headless => ("gpu2", 2),
        Screens::TwoVirtioGpu => ("gpu2", 0),
        _ => ("gpu", 0),
    }
}

/// Runs one configuration in a window, for a person to watch.
pub fn run(cfg: &Config, image: &Path, work: &Path) -> Result<(), String> {
    let mut cmd = command(cfg, image, work, false)?;
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
    let mut cmd = command(cfg, image, work, true)?;
    cmd.args(["-display", "none", "-monitor", "none"]);
    // The second screen of a virtio-gpu is plugged in, and later out and in again, by the
    // runner pretending to be a viewer of it (`vnc.rs` says why and how).
    let vnc_path = work.join(format!("vnc-{}{}.sock", cfg.arch, if cfg.el2 { "-el2" } else { "" }));
    let _ = fs::remove_file(&vnc_path);
    let two_screens = matches!(cfg.screens, Screens::TwoVirtioGpu);
    if two_screens {
        cmd.arg("-vnc").arg(format!("unix:{},display=gpu2,head=1", vnc_path.display()));
    }
    cmd.arg("-serial").arg(format!("file:{}", log.display()));
    // The second serial port (x86-64: COM2) goes to a file the runner reads afterwards.
    let com2 = work.join(format!("com2-{}.txt", if cfg.bios { "bios" } else { "uefi" }));
    let _ = fs::remove_file(&com2);
    if cfg.arch == "x86_64" {
        cmd.arg("-serial").arg(format!("file:{}", com2.display()));
    }
    let qmp_path = work.join(format!(
        "qmp-{}-{}{}.sock",
        cfg.arch,
        if cfg.bios { "bios" } else { "uefi" },
        if cfg.el2 { "-el2" } else { "" }
    ));
    let _ = fs::remove_file(&qmp_path);
    cmd.arg("-qmp").arg(format!("unix:{},server=on,wait=off", qmp_path.display()));
    cmd.stdout(Stdio::null()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("couldn't start QEMU: {e}"))?;
    let start = Instant::now();
    let mut qmp = Qmp::new(qmp_path, input_screen(cfg, true));
    let mut vnc = None;
    if two_screens {
        // Plug the second screen in before the system starts looking.
        match plug(&vnc_path) {
            Ok(v) => vnc = Some(v),
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("plugging in the second screen: {e}"));
            }
        }
    }
    // The guest's requests to unplug the second screen and plug it in again, done so far.
    let mut plugs = [false; 2];
    let mut injected = [false; STAGES.len()];
    let mut typed = 0;
    // How many tablets have been pointed with.
    let mut tablets = 0u32;
    // The same for the input service's self-test, and which of its key requests are done.
    let mut seat_tablets = 0u32;
    let mut seat_keys = [false; SEAT_KEYS.len()];
    // The pictures already looked at for the display self-test: (picture, device).
    let mut looked: Vec<(u32, String)> = Vec::new();
    let result = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        // The shell test: a command each time the console starts waiting for a line.
        if let Some(at) = text.find(SHELL_READY) {
            let waiting = text[at..].matches(CONSOLE_READ).count();
            let ready = |command: &str| {
                SHELL_WAITS.iter().all(|(c, line)| *c != command || text.contains(line))
            };
            if typed < SHELL_COMMANDS.len() && typed < waiting && ready(SHELL_COMMANDS[typed].0) {
                let (command, _, _) = SHELL_COMMANDS[typed];
                eprintln!("  (typing: {command})");
                let keys = SHELL_KEYS.iter().find(|(c, _)| *c == command).map_or(command, |k| k.1);
                if let Err(e) = qmp.type_line(keys) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("typing into QEMU: {e}\n--- serial output ---\n{text}"));
                }
                typed += 1;
            }
        }
        // The guest says when it is waiting for keys; the keys are pressed from here.
        for (i, stage) in STAGES.iter().enumerate() {
            if !injected[i] && text.contains(stage.marker) {
                injected[i] = true;
                eprintln!("  (sending input for: {})", stage.marker);
                if let Err(e) = qmp.inject(stage) {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "sending input to QEMU: {e}\n--- serial output ---\n{text}"
                    ));
                }
            }
        }
        // The input service's self-test: a key held, let go, and tapped.
        for (i, (marker, name, moves)) in SEAT_KEYS.iter().enumerate() {
            if seat_keys[i] || !text.contains(marker) {
                continue;
            }
            seat_keys[i] = true;
            eprintln!("  (sending input for: {marker})");
            let sent = moves.iter().try_for_each(|down| {
                let r = qmp.key(name, *down);
                std::thread::sleep(Duration::from_millis(100));
                r
            });
            if let Err(e) = sent {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("sending input to QEMU: {e}\n--- serial output ---\n{text}"));
            }
        }
        // And its tablets, pointed with as the driver test's are.
        if seat_tablets < 2
            && text.contains(&format!("Seat self-test: point tablet {seat_tablets}"))
        {
            eprintln!("  (pointing with tablet {seat_tablets} for the input service)");
            let screen = matches!(cfg.screens, Screens::TwoVirtioGpu).then_some(seat_tablets);
            if let Err(e) = qmp.point(screen, seat_tablets) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("sending input to QEMU: {e}\n--- serial output ---\n{text}"));
            }
            seat_tablets += 1;
        }
        // The tablets: the guest asks for each in turn (they are numbered in the order they
        // are on the command line), and gets a position of its own and a click.
        if tablets < 2 && text.contains(&format!("Device self-test: inject tablet {tablets}")) {
            eprintln!("  (pointing with tablet {tablets})");
            // A tablet tied to a screen is reached through that screen; one that isn't gets
            // whatever is sent to no screen in particular.
            let screen = matches!(cfg.screens, Screens::TwoVirtioGpu).then_some(tablets);
            if let Err(e) = qmp.point(screen, tablets) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("sending input to QEMU: {e}\n--- serial output ---\n{text}"));
            }
            tablets += 1;
        }
        // The display self-test asks for the second screen to be unplugged, then plugged in.
        for (i, (marker, size)) in [
            ("Device self-test: unplug screen 1", (0, 0)),
            ("Device self-test: plug screen 1", SECOND_SCREEN),
        ]
        .into_iter()
        .enumerate()
        {
            if !plugs[i] && text.contains(marker) {
                plugs[i] = true;
                eprintln!("  ({})", marker.trim_start_matches("Device self-test: "));
                let done = match vnc.as_mut() {
                    Some(v) => v.set_size(size.0, size.1),
                    None => Err("this configuration has no second screen".into()),
                };
                if let Err(e) = done {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("{marker}: {e}\n--- serial output ---\n{text}"));
                }
            }
        }
        // The display self-test: the guest says which picture every screen of a display
        // shows; look at each, and press a key to say so (note 0020, section 7).
        if let Some((number, device)) = next_look(&text, &looked) {
            eprintln!("  (looking at picture {number} on {device})");
            let seen = look(&mut qmp, &text, number, &device, &log).and_then(|()| qmp.press_a());
            if let Err(e) = seen {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "picture {number} on {device}: {e}\n--- serial output ---\n{text}"
                ));
            }
            looked.push((number, device));
        }
        if text.contains(REPORT_DONE) {
            break Ok(text);
        }
        // A self-test that passes ends by restarting the machine (note 0019, section 5), and
        // QEMU, told `-no-reboot`, stops instead. That is the test's last check: QEMU must
        // have stopped because the guest reset it, after the kernel said it was restarting.
        if said_restarting(&text) {
            let exited = (0..50).any(|_| {
                std::thread::sleep(Duration::from_millis(100));
                matches!(child.try_wait(), Ok(Some(_)))
            });
            if !exited {
                let regs = name_addresses(qmp.registers(cfg.cores), cfg, image);
                let _ = fs::write(log.with_extension("registers.txt"), &regs);
                break Err(format!(
                    "the kernel said it was restarting, but the machine didn't restart.\n--- \
                     serial output ---\n{text}\n--- registers ---\n{regs}"
                ));
            }
            break reset_by_guest(&mut qmp, text);
        }
        // A kernel exception or panic: give it a moment to finish its report, then ask QEMU
        // what every core is doing, since the report may never finish.
        if text.contains("\nException on core") || text.contains("The kernel stopped because") {
            std::thread::sleep(Duration::from_secs(3));
            let text = fs::read_to_string(&log).unwrap_or_default();
            let regs = name_addresses(qmp.registers(cfg.cores), cfg, image);
            let _ = fs::write(log.with_extension("registers.txt"), &regs);
            break Err(format!(
                "the kernel stopped.\n--- serial output ---\n{text}\n--- registers ---\n{regs}"
            ));
        }
        if let Ok(Some(st)) = child.try_wait() {
            // It may have stopped between the log being read and now: read it once more.
            let text = fs::read_to_string(&log).unwrap_or_default();
            if said_restarting(&text) {
                break reset_by_guest(&mut qmp, text);
            }
            break Err(format!(
                "QEMU stopped ({st}) before the kernel finished.\n--- serial output ---\n{text}"
            ));
        }
        if start.elapsed() > limit {
            let regs = name_addresses(qmp.registers(cfg.cores), cfg, image);
            let _ = fs::write(log.with_extension("registers.txt"), &regs);
            break Err(format!(
                "no start-up report after {} seconds.\n--- serial output ---\n{text}\n--- \
                 registers ---\n{regs}",
                limit.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(200));
    };
    let _ = child.kill();
    let _ = child.wait();
    let text = result?.replace('\r', "");
    check_conversation(&text)?;
    let missing: Vec<&str> = cfg.expect.iter().copied().filter(|e| !text.contains(e)).collect();
    // What the guest wrote to its second serial port.
    if cfg.arch == "x86_64" {
        let out = fs::read_to_string(&com2).unwrap_or_default();
        if !out.contains("PrettyBigRock COM2 test\n") {
            return Err(format!(
                "COM2 output was {out:?}, not the test line.\n--- serial output ---\n{text}"
            ));
        }
    }
    if missing.is_empty() {
        Ok(text)
    } else {
        Err(format!("the report is missing {missing:?}.\n--- serial output ---\n{text}"))
    }
}

/// The shell test (docs/design/0018, section 7, test 8; `services/root/src/shelltest.rs`): the
/// root program prints this once the shell is running on the console.
const SHELL_READY: &str = "Shell self-test: type commands";
/// What the console writes to the kernel log, in self-test runs, when it starts waiting for a
/// line, and in front of every line it shows (`services/console`), after the tag the kernel puts
/// in front of every line a program logs (its name).
const CONSOLE_READ: &str = "[console] [console:read] ";
const CONSOLE_LINE: &str = "[console] [console] ";

/// The commands typed into the shell, in order, each when the console next waits for a line:
/// (the command; text the console must show in answer; text it mustn't). The answer is
/// everything shown between the command and the next prompt. `tools/check_shell_after.py`
/// checks the disk afterwards, so keep the folder, names and text the same as there.
#[rustfmt::skip]
const SHELL_COMMANDS: &[(&str, &[&str], &[&str])] = &[
    ("whoami", &["chris"], &[]),
    ("ls", &["Documents/", "Pictures/"], &[]),
    ("mkdir Shelltest", &[], &[]),
    ("cd Shelltest", &[], &[]),
    ("pwd", &["/Home/Shelltest"], &[]),
    ("write hello.txt Hello from the shell", &[], &[]),
    ("cat hello.txt", &["Hello from the shell"], &[]),
    ("mv hello.txt greeting.txt", &[], &[]),
    ("write gone.txt bye", &[], &[]),
    ("rm gone.txt", &[], &[]),
    ("ls", &["greeting.txt"], &["hello.txt", "gone.txt"]),
    ("cat /Home/Shelltest/nothing.txt", &["not found"], &[]),
    ("run sandboxed greeting.txt -- idle", &["Started sandboxed as app"], &[]),
    ("grants sandboxed", &[
        "/Home/Shelltest/greeting.txt  read write  chosen",
        "/Home/Documents  read write  manifest",
        "/Home/Pictures  read  manifest",
    ], &["Music"]),
    ("revoke sandboxed /Home/Shelltest/greeting.txt",
        &["Took /Home/Shelltest/greeting.txt back from sandboxed."], &[]),
    ("grants sandboxed", &["/Home/Documents"], &["greeting.txt"]),
    ("stop sandboxed", &[], &[]),
    ("volumes", &["Home", "PICTURES", "FAT32"], &[]),
    ("frobnicate", &["frobnicate: no such command"], &[]),
    // Keyboard layouts (note 0021, section 9). Each `echo` is typed with the keys in
    // `SHELL_KEYS`, which only type this on that layout. After `set layout gb` the guest
    // crashes its input service (`SHELL_WAITS`): the line after it also shows that the
    // console found the new one and that the new one was told the layout.
    ("settings", &["layout us", "timezone +00:00"], &[]),
    ("set layout klingon", &["set: the settings are layout"], &[]),
    ("set layout gb", &[], &["set:"]),
    ("echo £@€é¬\\|#~", &["£@€é¬\\|#~"], &[]),
    ("set layout us-intl", &[], &["set:"]),
    ("echo êëôûïñ€^x¿", &["êëôûïñ€^x¿"], &[]),
    ("set timezone +02:00", &[], &["set:"]),
    ("set layout us", &[], &["set:"]),
    ("settings", &["layout us", "timezone +02:00"], &["gb", "intl"]),
    ("shutdown", &["It's now safe to turn off your computer."], &[]),
];

/// The keys that type the commands a US keyboard's keys wouldn't: (the command as the console
/// shows it, the keys in `Qmp::type_line`'s notation).
const SHELL_KEYS: &[(&str, &str)] = &[
    // UK: Shift+3, Shift+apostrophe, AltGr+4, AltGr+e, Shift+grave, the key beside the left
    // Shift plain and shifted, and the key beside Enter plain and shifted.
    (
        "echo £@€é¬\\|#~",
        "echo {S-3}{S-apostrophe}{G-4}{G-e}{S-grave_accent}{less}{S-less}{backslash}{S-backslash}",
    ),
    // US international: the circumflex (Shift+6) and diaeresis (Shift+apostrophe) dead keys
    // before letters, AltGr+n and AltGr+5, a dead key before a letter it doesn't combine
    // with (which types both), and AltGr+slash.
    (
        "echo êëôûïñ€^x¿",
        "echo {S-6}e{S-apostrophe}e{S-6}o{S-6}u{S-apostrophe}i{G-n}{G-5}{S-6}x{G-slash}",
    ),
];

/// Commands that are typed only once the guest has printed a line: (the command, the line).
/// `services/root/src/shelltest.rs` crashes the input service once the layout is `gb`, and
/// says when the new one is ready to be typed at.
const SHELL_WAITS: &[(&str, &str)] =
    &[("echo £@€é¬\\|#~", "Shell self-test: the input service was restarted")];

/// What the input service's self-test asks for, and the key to press or let go
/// (`services/root/src/seattest.rs`): (the line, the key, down or up, then up again).
const SEAT_KEYS: &[(&str, &str, &[bool])] = &[
    ("Seat self-test: hold b", "b", &[true]),
    ("Seat self-test: release b", "b", &[false]),
    ("Seat self-test: tap a", "a", &[true, false]),
];

/// Checks the console's answer to every command in [`SHELL_COMMANDS`].
fn check_conversation(text: &str) -> Result<(), String> {
    let Some(at) = text.find(SHELL_READY) else { return Ok(()) };
    let lines: Vec<&str> = text[at..].lines().collect();
    let mut i = 0;
    for (command, want, unwanted) in SHELL_COMMANDS {
        // The line the console showed when the command was entered: the prompt, then it.
        let entered = lines[i..]
            .iter()
            .position(|l| l.starts_with(CONSOLE_LINE) && l.ends_with(&format!("> {command}")))
            .ok_or_else(|| format!("the console never showed the command `{command}` entered"))?;
        i += entered + 1;
        // Its answer: what the console showed before it next waited for a line. (Other
        // programs' log lines may be mixed in; only the console's count.)
        let end = lines[i..]
            .iter()
            .position(|l| l.starts_with(CONSOLE_READ))
            .map_or(lines.len(), |n| i + n);
        let answer: Vec<&str> =
            lines[i..end].iter().filter_map(|l| l.strip_prefix(CONSOLE_LINE)).collect();
        let answer = answer.join("\n");
        for w in *want {
            if !answer.contains(w) {
                return Err(format!("`{command}` answered {answer:?}, without {w:?}"));
            }
        }
        for u in *unwanted {
            if answer.contains(u) {
                return Err(format!("`{command}` answered {answer:?}, with {u:?}"));
            }
        }
    }
    Ok(())
}

/// The QEMU key (its "qcode") that types `c` on a US keyboard, and whether Shift is needed.
fn qcode(c: char) -> Option<(String, bool)> {
    let plain = |k: &str| Some((String::from(k), false));
    let shifted = |k: &str| Some((String::from(k), true));
    match c {
        'a'..='z' | '0'..='9' => plain(&c.to_string()),
        'A'..='Z' => shifted(&c.to_ascii_lowercase().to_string()),
        ' ' => plain("spc"),
        '.' => plain("dot"),
        '/' => plain("slash"),
        '-' => plain("minus"),
        '_' => shifted("minus"),
        ',' => plain("comma"),
        '\'' => plain("apostrophe"),
        '"' => shifted("apostrophe"),
        ':' => shifted("semicolon"),
        '+' => shifted("equal"),
        _ => None,
    }
}

/// Something to do to the guest's keyboards and mice when it prints `marker`.
struct Stage {
    marker: &'static str,
    /// Send to the devices tied to the named screen (the virtio ones), or to the PS/2 pair.
    virtio: bool,
    /// What to send.
    what: What,
}

enum What {
    /// Press and release A, move the mouse 10 right and 5 down with the left button down, then
    /// release the button. (QEMU's PS/2 mouse merges what arrives together into one packet,
    /// so the button goes up in a call of its own.)
    Typing,
    /// This many one-pixel movements to the right, in one call.
    Flood(u32),
}

/// The test's stages, in the order the guest asks for them (`services/root/src/drivertest.rs`).
const STAGES: [Stage; 4] = [
    Stage { marker: "Device self-test: inject virtio", virtio: true, what: What::Typing },
    Stage { marker: "Device self-test: inject flood", virtio: true, what: What::Flood(700) },
    Stage { marker: "Device self-test: inject once", virtio: true, what: What::Flood(1) },
    Stage { marker: "Device self-test: inject ps2", virtio: false, what: What::Typing },
];

/// The size the runner gives the second screen of a virtio-gpu (the first is QEMU's own
/// choice, 1280 by 800): different, so that a picture sent to the wrong screen doesn't fit.
pub(crate) const SECOND_SCREEN: (u16, u16) = (1024, 768);

/// Connects to QEMU's VNC server at `path` (waiting for QEMU to make it) and plugs the screen
/// it is for in.
pub(crate) fn plug(path: &Path) -> Result<crate::vnc::Vnc, String> {
    let end = Instant::now() + Duration::from_secs(20);
    loop {
        match crate::vnc::Vnc::connect(path) {
            Ok(mut v) => {
                v.set_size(SECOND_SCREEN.0, SECOND_SCREEN.1)?;
                return Ok(v);
            }
            Err(e) if Instant::now() > end => return Err(e),
            Err(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

/// Where the first tablet is pointed, in QEMU's units (0 to 32767 each way); the second gets
/// twice this. `services/root/src/drivertest.rs` expects the same numbers.
const TABLET_X: u32 = 4000;
const TABLET_Y: u32 = 3000;

/// The memory every test machine has, in MiB. It was 512 until M6: the services' budgets (a
/// fixed share of memory each, note 0006, section 9) then grew by a display driver's buffers
/// (note 0020), and a service being restarted needs its budget free while the one it replaces
/// still holds its own, which 512 MiB no longer left room for.
const MEMORY_MIB: &str = "1024";

/// The first picture the guest has asked the runner to look at that it hasn't yet: the
/// picture's number and the display device's name.
fn next_look(text: &str, looked: &[(u32, String)]) -> Option<(u32, String)> {
    (1..=4)
        .flat_map(|n| pictures::asked(text, n).into_iter().map(move |d| (n, d)))
        .find(|ask| !looked.contains(ask))
}

/// Takes a screenshot of every screen of `device` that the guest says shows picture
/// `number`, and compares each with that picture. A screenshot that is wrong is kept beside
/// the log, for a person to look at.
fn look(qmp: &mut Qmp, text: &str, number: u32, device: &str, log: &Path) -> Result<(), String> {
    let screens = pictures::shown(text, number, device);
    if screens.is_empty() {
        return Err("the guest didn't say which screens show it".into());
    }
    // The device manager names a device after its driver; QEMU knows it by the name given
    // on its command line ([`command`]).
    let qemu_name = if device.starts_with("virtio-gpu") { "gpu2" } else { "gpu" };
    for want in &screens {
        let file = log.with_extension(format!("picture{number}-{qemu_name}-{}.ppm", want.screen));
        let _ = fs::remove_file(&file);
        qmp.screendump(qemu_name, want.screen, &file)?;
        let bytes = fs::read(&file).map_err(|e| format!("reading {}: {e}", file.display()))?;
        let result =
            pictures::read_ppm(&bytes).and_then(|shot| pictures::compare(&shot, number, want));
        match result {
            Ok(()) => {
                let _ = fs::remove_file(&file);
            }
            Err(e) => {
                return Err(format!(
                    "screen {}: {e} (screenshot kept: {})",
                    want.screen,
                    file.display()
                ));
            }
        }
    }
    Ok(())
}

/// A connection to QEMU's control socket, made when first needed.
/// The registers and stacks QEMU gave, with the kernel function named for every address in
/// them that is inside one (see [`crate::symbols`]). The kernel that was booted is beside the
/// disk image: `build/debug/x.img` goes with `build/<architecture>/debug/kernel.elf`. If it
/// can't be read, the dump is returned as it was.
pub(crate) fn name_addresses(dump: String, cfg: &Config, image: &Path) -> String {
    let kernel = image.parent().and_then(|profile| {
        let build = profile.parent()?;
        Some(build.join(cfg.arch).join(profile.file_name()?).join("kernel.elf"))
    });
    match kernel.as_deref().and_then(crate::symbols::Symbols::of_kernel) {
        Some(symbols) => symbols.annotate(&dump),
        None => dump,
    }
}

/// Whether the serial log shows a passed program self-test followed by the kernel's line
/// that it is restarting the machine.
fn said_restarting(text: &str) -> bool {
    text.find(SELFTEST_PASSED).is_some_and(|at| text[at..].lines().any(|l| l.trim() == RESTARTING))
}

/// QEMU has stopped after the kernel said it was restarting: `Ok(text)` if QEMU says the
/// guest reset the machine, which is what a restart is.
fn reset_by_guest(qmp: &mut Qmp, text: String) -> Result<String, String> {
    match qmp.shutdown_reason().as_deref() {
        Some("guest-reset") => Ok(text),
        other => Err(format!(
            "QEMU stopped after the restart was asked for, but for the reason {other:?}, not a \
             reset by the guest.\n--- serial output ---\n{text}"
        )),
    }
}

pub(crate) struct Qmp {
    path: PathBuf,
    conn: Option<(
        std::os::unix::net::UnixStream,
        std::io::BufReader<std::os::unix::net::UnixStream>,
    )>,
    /// The events QEMU has sent so far (it sends them between answers), one JSON line each.
    events: Vec<String>,
    /// The screen the virtio keyboard and mouse are tied to ([`input_screen`]).
    input: (&'static str, u32),
}

impl Qmp {
    pub(crate) fn new(path: PathBuf, input: (&'static str, u32)) -> Qmp {
        Qmp { path, conn: None, events: Vec::new(), input }
    }

    pub(crate) fn connect(&mut self) -> Result<(), String> {
        use std::io::BufRead;
        if self.conn.is_some() {
            return Ok(());
        }
        let stream = std::os::unix::net::UnixStream::connect(&self.path)
            .map_err(|e| format!("connecting to {}: {e}", self.path.display()))?;
        stream.set_read_timeout(Some(Duration::from_secs(10))).map_err(|e| e.to_string())?;
        let mut reader = std::io::BufReader::new(stream.try_clone().map_err(|e| e.to_string())?);
        let mut greeting = String::new();
        reader.read_line(&mut greeting).map_err(|e| format!("reading QEMU's greeting: {e}"))?;
        self.conn = Some((stream, reader));
        self.command(r#"{"execute":"qmp_capabilities"}"#)
    }

    /// Sends one command and waits for its answer (skipping events).
    pub(crate) fn command(&mut self, json: &str) -> Result<(), String> {
        self.query(json).map(|_| ())
    }

    /// Sends one command and returns its answer line (skipping events).
    fn query(&mut self, json: &str) -> Result<String, String> {
        use std::io::{BufRead, Write};
        let (w, r) = self.conn.as_mut().ok_or("not connected")?;
        writeln!(w, "{json}").map_err(|e| format!("writing to QEMU: {e}"))?;
        loop {
            let mut line = String::new();
            let n = r.read_line(&mut line).map_err(|e| format!("reading QEMU's answer: {e}"))?;
            if n == 0 {
                return Err("QEMU closed the control socket".into());
            }
            if line.contains("\"error\"") {
                return Err(format!("QEMU refused {json:.200}: {line}"));
            }
            if line.contains("\"return\"") {
                return Ok(line);
            }
            if line.contains("\"event\"") {
                self.events.push(line);
            }
        }
    }

    /// Why QEMU shut down, once it has: the `reason` of its `SHUTDOWN` event, which is
    /// `guest-reset` when the guest restarted the machine (and QEMU was told `-no-reboot`) and
    /// `guest-shutdown` when it turned it off. Reads whatever QEMU sent before it went.
    pub(crate) fn shutdown_reason(&mut self) -> Option<String> {
        use std::io::BufRead;
        if let Some((_, r)) = self.conn.as_mut() {
            let mut line = String::new();
            // Until the socket closes (QEMU has exited) or nothing comes for the time-out.
            while r.read_line(&mut line).is_ok_and(|n| n > 0) {
                if line.contains("\"event\"") {
                    self.events.push(std::mem::take(&mut line));
                }
                line.clear();
            }
        }
        let event = self.events.iter().find(|e| e.contains("\"SHUTDOWN\""))?;
        let after = event.split("\"reason\"").nth(1)?;
        Some(after.split('"').nth(1)?.to_string())
    }

    /// What every core was doing, for a run that failed: QEMU's own view of each virtual
    /// processor's registers, which works even when the kernel can no longer print. (A kernel
    /// that faults on a broken stack often stops halfway through its own report.)
    pub(crate) fn registers(&mut self, cores: u32) -> String {
        if let Err(e) = self.connect() {
            return format!("(couldn't ask QEMU for the registers: {e})");
        }
        let mut out = String::new();
        for cpu in 0..cores {
            let regs = self.hmp(cpu, "info registers");
            out.push_str(&format!("--- core {cpu}: registers (QEMU's view) ---\n{regs}\n"));
            // The top of its stack too, which holds the return addresses: AArch64 calls the
            // stack pointer SP, x86-64 RSP.
            let sp = regs
                .split(|c: char| c.is_whitespace())
                .find_map(|w| w.strip_prefix("SP=").or_else(|| w.strip_prefix("RSP=")));
            if let Some(sp) = sp {
                let stack = self.hmp(cpu, &format!("x /48xg 0x{sp}"));
                out.push_str(&format!("--- core {cpu}: stack from {sp} ---\n{stack}\n"));
            }
        }
        out
    }

    /// Runs one of QEMU's monitor commands as core `cpu` sees things, and returns its text.
    fn hmp(&mut self, cpu: u32, what: &str) -> String {
        let json = format!(
            r#"{{"execute":"human-monitor-command","arguments":{{"command-line":"{what}","cpu-index":{cpu}}}}}"#
        );
        match self.query(&json) {
            Ok(line) => {
                // The answer is a JSON string: undo its escapes enough to read it.
                let body = line
                    .split_once("\"return\": \"")
                    .map_or(line.as_str(), |(_, b)| b)
                    .trim_end()
                    .trim_end_matches('}')
                    .trim_end_matches('"');
                body.replace("\\r\\n", "\n").replace("\\n", "\n")
            }
            Err(e) => format!("({e})"),
        }
    }

    pub(crate) fn send(&mut self, virtio: bool, events: &[String]) -> Result<(), String> {
        let (device, head) = self.input;
        let target =
            if virtio { format!(r#""device":"{device}","head":{head},"#) } else { String::new() };
        let json = format!(
            r#"{{"execute":"input-send-event","arguments":{{{target}"events":[{}]}}}}"#,
            events.join(",")
        );
        self.command(&json)
    }

    /// Has QEMU write a picture of what screen `head` of its device `device` shows to `file`.
    pub(crate) fn screendump(
        &mut self,
        device: &str,
        head: u32,
        file: &Path,
    ) -> Result<(), String> {
        self.connect()?;
        self.command(&format!(
            r#"{{"execute":"screendump","arguments":{{"filename":"{}","device":"{device}","head":{head}}}}}"#,
            file.display()
        ))
    }

    /// Presses and releases A on the virtio keyboard: the runner's answer to the guest.
    fn press_a(&mut self) -> Result<(), String> {
        self.connect()?;
        let key = |down: bool| {
            format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"a"}}}}}}"#
            )
        };
        self.send(true, &[key(true)])?;
        std::thread::sleep(Duration::from_millis(100));
        self.send(true, &[key(false)])
    }

    /// Types `line` and presses Enter on the virtio keyboard, a key at a time, the way a person
    /// would: each press and release is its own event, with a pause after each so the guest
    /// has handed back its buffers (see `Flood` below).
    ///
    /// A character is typed with the key that types it on a US keyboard. A key can also be
    /// named, in braces, by QEMU's name for it (its "qcode"), with `S-` in front for Shift and
    /// `G-` for AltGr (the right Alt key): `{S-3}` is Shift and 3, whatever that types on the
    /// guest's layout. That is how the layout tests press keys that don't type what a US
    /// keyboard would.
    pub(crate) fn type_line(&mut self, line: &str) -> Result<(), String> {
        self.connect()?;
        let key = |name: &str, down: bool| {
            format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"{name}"}}}}}}"#
            )
        };
        // (The key, the modifier keys held round it.)
        let mut keys: Vec<(String, Vec<&str>)> = Vec::new();
        let mut rest = line;
        while let Some(c) = rest.chars().next() {
            if c == '{' {
                let end = rest.find('}').ok_or_else(|| format!("no closing brace in {line:?}"))?;
                let mut name = &rest[1..end];
                let mut held = Vec::new();
                loop {
                    if let Some(n) = name.strip_prefix("S-") {
                        held.push("shift");
                        name = n;
                    } else if let Some(n) = name.strip_prefix("G-") {
                        held.push("alt_r");
                        name = n;
                    } else {
                        break;
                    }
                }
                keys.push((String::from(name), held));
                rest = &rest[end + 1..];
            } else {
                let (name, shift) = qcode(c).ok_or_else(|| format!("can't type {c:?}"))?;
                keys.push((name, if shift { vec!["shift"] } else { Vec::new() }));
                rest = &rest[c.len_utf8()..];
            }
        }
        keys.push((String::from("ret"), Vec::new()));
        let pause = || std::thread::sleep(Duration::from_millis(15));
        for (name, held) in keys {
            for h in &held {
                self.send(true, &[key(h, true)])?;
                pause();
            }
            self.send(true, &[key(&name, true)])?;
            pause();
            self.send(true, &[key(&name, false)])?;
            pause();
            for h in held.iter().rev() {
                self.send(true, &[key(h, false)])?;
                pause();
            }
        }
        Ok(())
    }

    /// Presses or lets go of one key on the virtio keyboard and leaves it that way.
    fn key(&mut self, name: &str, down: bool) -> Result<(), String> {
        self.connect()?;
        self.send(
            true,
            &[format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"{name}"}}}}}}"#
            )],
        )
    }

    /// Points with tablet `n` and clicks: at ([`TABLET_X`], [`TABLET_Y`]) times `n + 1`, of
    /// the 32767 QEMU's tablets measure each way, so that every tablet gets a place of its
    /// own and one tablet's events can't be taken for another's. `screen` is the screen of
    /// the second display device the tablet is tied to, if it is tied to one.
    fn point(&mut self, screen: Option<u32>, n: u32) -> Result<(), String> {
        self.connect()?;
        let target = match screen {
            Some(head) => format!(r#""device":"gpu2","head":{head},"#),
            None => String::new(),
        };
        let abs = |axis: &str, v: u32| {
            format!(r#"{{"type":"abs","data":{{"axis":"{axis}","value":{v}}}}}"#)
        };
        let button =
            |down: bool| format!(r#"{{"type":"btn","data":{{"button":"left","down":{down}}}}}"#);
        let (x, y) = (TABLET_X * (n + 1), TABLET_Y * (n + 1));
        for events in [vec![abs("x", x), abs("y", y)], vec![button(true)], vec![button(false)]] {
            self.command(&format!(
                r#"{{"execute":"input-send-event","arguments":{{{target}"events":[{}]}}}}"#,
                events.join(",")
            ))?;
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }

    fn inject(&mut self, stage: &Stage) -> Result<(), String> {
        self.connect()?;
        let key = |down: bool| {
            format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"a"}}}}}}"#
            )
        };
        let rel = |axis: &str, v: i32| {
            format!(r#"{{"type":"rel","data":{{"axis":"{axis}","value":{v}}}}}"#)
        };
        let button =
            |down: bool| format!(r#"{{"type":"btn","data":{{"button":"left","down":{down}}}}}"#);
        let pause = || std::thread::sleep(Duration::from_millis(100));
        match stage.what {
            What::Typing => {
                self.send(stage.virtio, &[key(true)])?;
                pause();
                self.send(stage.virtio, &[key(false)])?;
                pause();
                self.send(stage.virtio, &[rel("x", 10), rel("y", 5), button(true)])?;
                pause();
                self.send(stage.virtio, &[button(false)])
            }
            What::Flood(n) => {
                // In calls of 50, with a pause after each so the guest can hand its buffers back:
                // QEMU's virtio input device keeps events it has no buffer for and only
                // delivers them when the next event arrives.
                let mut left = n;
                while left > 0 {
                    let now = left.min(50);
                    let events: Vec<String> = (0..now).map(|_| rel("x", 1)).collect();
                    self.send(stage.virtio, &events)?;
                    left -= now;
                    pause();
                }
                Ok(())
            }
        }
    }
}
