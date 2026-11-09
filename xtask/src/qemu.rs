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

/// The screens a configuration has. Every configuration keeps the firmware's screen (QEMU's
/// standard VGA, or `ramfb` on ARM64), which the kernel and the M5 console draw on until the
/// display drivers of M6 stage 2 exist; `TwoVirtioGpu` adds a virtio-gpu with two screens and
/// a tablet (an absolute pointer) tied to each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screens {
    /// Only the firmware's screen, with no tablet: the PS/2 mouse is the pointer to test.
    Firmware,
    /// The firmware's screen, and a tablet (not tied to it; see [`command`]).
    FirmwareWithTablet,
    /// The firmware's screen, and a virtio-gpu with two screens, a tablet on each.
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
            "Screens:       1 (",
            "Randomness:    seeded",
            "Self-test passed",
            "Program self-test passed",
            "a block device (2048 sectors, reads, writes and refusals)",
            "a driver crashed and restarted",
            "Device self-test passed",
            "Sandbox self-test passed",
            "Supervision self-test passed",
            "Shell self-test passed",
            "the framebuffer",
            "a virtio keyboard and mouse",
            "a full ring",
            "COM2 (output and loopback)",
            "a PS/2 keyboard and mouse",
            "exited with status 0.",
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
            "Shell self-test passed",
            "the framebuffer",
            "a virtio keyboard and mouse",
            "a full ring",
            "COM2 (output and loopback)",
            "a PS/2 keyboard and mouse",
            "exited with status 0.",
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
            "Shell self-test passed",
            "the framebuffer",
            "a virtio keyboard and mouse",
            "a full ring",
            "exited with status 0.",
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
            "Shell self-test passed",
            "the framebuffer",
            "a virtio keyboard and mouse",
            "a full ring",
            "exited with status 0.",
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
fn tag(cfg: &Config) -> String {
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
fn command(cfg: &Config, image: &Path, work: &Path) -> Result<Command, String> {
    let drive = format!("format=raw,file={},snapshot=on", image.display());
    let mut cmd;
    if cfg.arch == "x86_64" {
        cmd = Command::new("qemu-system-x86_64");
        cmd.args(["-machine", "q35", "-m", "512"]);
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
        cmd.args(["-vga", "none", "-device", "VGA,id=gpu"]);
    } else {
        cmd = Command::new("qemu-system-aarch64");
        let machine = if cfg.el2 { "virt,virtualization=on" } else { "virt" };
        cmd.args(["-machine", machine, "-cpu", cfg.cpu.unwrap_or("cortex-a72"), "-m", "512"]);
        cmd.arg("-smp").arg(cfg.cores.to_string());
        let fw = first_existing(&[
            "/usr/share/qemu-efi-aarch64/QEMU_EFI.fd",
            "/usr/share/AAVMF/AAVMF_CODE.fd",
            "/usr/share/edk2/aarch64/QEMU_EFI.fd",
            "/usr/share/qemu/edk2-aarch64-code.fd",
        ])
        .ok_or("can't find UEFI for ARM64 QEMU: install the `qemu-efi-aarch64` package")?;
        cmd.arg("-bios").arg(fw);
        cmd.args(["-device", "ramfb,id=gpu", "-device", "qemu-xhci", "-device", "usb-kbd"]);
        cmd.arg("-drive").arg(format!("if=none,id=disk,{drive}"));
        cmd.args(["-device", "virtio-blk-pci,drive=disk", "-device", "virtio-rng-pci"]);
    }
    let disk = test_disk(work)?;
    cmd.arg("-drive")
        .arg(format!("if=none,id=testdisk,format=raw,file={},snapshot=on", disk.display()));
    cmd.args(["-device", "virtio-blk-pci,drive=testdisk"]);
    // The storage service's test disks (note 0018, section 7, test 2).
    // The MBR and GPT disks are attached writable, so their FAT32 volume and system volume
    // can be checked after the boot ([`check_fat`], [`check_shell`]); `make` remakes them for
    // each boot.
    for (i, disk) in crate::testdisks::make(work)?.iter().enumerate() {
        let writable = disk.ends_with(crate::testdisks::MBR_DISK)
            || disk.ends_with(crate::testdisks::GPT_DISK);
        let snapshot = if writable { "" } else { ",snapshot=on" };
        cmd.arg("-drive")
            .arg(format!("if=none,id=storage{i},format=raw,file={}{snapshot}", disk.display()));
        cmd.arg("-device").arg(format!("virtio-blk-pci,drive=storage{i}"));
    }
    let classic = classic_copy(cfg, work)?;
    cmd.arg("-drive").arg(format!("if=none,id=classic,format=raw,file={}", classic.display()));
    cmd.args(["-device", "virtio-blk-pci,drive=classic"]);
    cmd.args(["-device", "virtio-keyboard-pci,id=vkbd,display=gpu"]);
    cmd.args(["-device", "virtio-mouse-pci,id=vmouse,display=gpu"]);
    // M6's screens and tablets (note 0026, section 6), after the mouse: devices are found in
    // the order they are given, and M4's driver test uses the first virtio pointer it finds.
    // QEMU ties a tablet to one screen (`display` and `head`), as a touch screen is.
    match cfg.screens {
        Screens::Firmware => {}
        Screens::FirmwareWithTablet => {
            // Not tied to the screen: the virtio mouse is, and QEMU sends a screen's buttons to
            // the last device tied to it, which would take them from the mouse M4's test uses.
            // An untied tablet gets only what is sent to no particular screen.
            cmd.args(["-device", "virtio-tablet-pci,id=tablet0"]);
        }
        Screens::TwoVirtioGpu => {
            cmd.args(["-device", "virtio-gpu-pci,id=gpu2,max_outputs=2"]);
            cmd.args(["-device", "virtio-tablet-pci,id=tablet0,display=gpu2,head=0"]);
            cmd.args(["-device", "virtio-tablet-pci,id=tablet1,display=gpu2,head=1"]);
        }
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
    let mut qmp = Qmp::new(qmp_path);
    let mut injected = [false; STAGES.len()];
    let mut typed = 0;
    let result = loop {
        let text = fs::read_to_string(&log).unwrap_or_default();
        // The shell test: a command each time the console starts waiting for a line.
        if let Some(at) = text.find(SHELL_READY) {
            let waiting = text[at..].matches(CONSOLE_READ).count();
            if typed < SHELL_COMMANDS.len() && typed < waiting {
                let (command, _, _) = SHELL_COMMANDS[typed];
                eprintln!("  (typing: {command})");
                if let Err(e) = qmp.type_line(command) {
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
        if text.contains(REPORT_DONE) {
            break Ok(text);
        }
        // A kernel exception or panic: give it a moment to finish its report, then ask QEMU
        // what every core is doing, since the report may never finish.
        if text.contains("\nException on core") || text.contains("The kernel stopped because") {
            std::thread::sleep(Duration::from_secs(3));
            let text = fs::read_to_string(&log).unwrap_or_default();
            let regs = qmp.registers(cfg.cores);
            let _ = fs::write(log.with_extension("registers.txt"), &regs);
            break Err(format!(
                "the kernel stopped.\n--- serial output ---\n{text}\n--- registers ---\n{regs}"
            ));
        }
        if let Ok(Some(st)) = child.try_wait() {
            break Err(format!(
                "QEMU stopped ({st}) before the kernel finished.\n--- serial output ---\n{text}"
            ));
        }
        if start.elapsed() > limit {
            let regs = qmp.registers(cfg.cores);
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
    ("shutdown", &["It's now safe to turn off your computer."], &[]),
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

/// A connection to QEMU's control socket, made when first needed.
struct Qmp {
    path: PathBuf,
    conn: Option<(
        std::os::unix::net::UnixStream,
        std::io::BufReader<std::os::unix::net::UnixStream>,
    )>,
}

impl Qmp {
    fn new(path: PathBuf) -> Qmp {
        Qmp { path, conn: None }
    }

    fn connect(&mut self) -> Result<(), String> {
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
    fn command(&mut self, json: &str) -> Result<(), String> {
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
        }
    }

    /// What every core was doing, for a run that failed: QEMU's own view of each virtual
    /// processor's registers, which works even when the kernel can no longer print. (A kernel
    /// that faults on a broken stack often stops halfway through its own report.)
    fn registers(&mut self, cores: u32) -> String {
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

    fn send(&mut self, virtio: bool, events: &[String]) -> Result<(), String> {
        let target = if virtio { r#""device":"gpu","head":0,"# } else { "" };
        let json = format!(
            r#"{{"execute":"input-send-event","arguments":{{{target}"events":[{}]}}}}"#,
            events.join(",")
        );
        self.command(&json)
    }

    /// Types `line` and presses Enter on the virtio keyboard, a key at a time, the way a person
    /// would: each press and release is its own event, with a pause after each so the guest
    /// has handed back its buffers (see `Flood` below).
    fn type_line(&mut self, line: &str) -> Result<(), String> {
        self.connect()?;
        let key = |name: &str, down: bool| {
            format!(
                r#"{{"type":"key","data":{{"down":{down},"key":{{"type":"qcode","data":"{name}"}}}}}}"#
            )
        };
        let mut keys: Vec<(String, bool)> = Vec::new();
        for c in line.chars() {
            keys.push(qcode(c).ok_or_else(|| format!("can't type {c:?}"))?);
        }
        keys.push((String::from("ret"), false));
        let pause = || std::thread::sleep(Duration::from_millis(15));
        for (name, shift) in keys {
            if shift {
                self.send(true, &[key("shift", true)])?;
                pause();
            }
            self.send(true, &[key(&name, true)])?;
            pause();
            self.send(true, &[key(&name, false)])?;
            pause();
            if shift {
                self.send(true, &[key("shift", false)])?;
                pause();
            }
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
