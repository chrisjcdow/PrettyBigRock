// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! PrettyBigRock's build tool. Run it through Cargo:
//!
//! ```text
//! cargo xtask build [--release]               build everything, and the boot disk image
//! cargo xtask run [--arch NAME] [--bios] [--el2] [--release]
//!                                             start PrettyBigRock in QEMU, in a window
//! cargo xtask qemu-test [--release]           boot every configuration and check the kernel starts
//! cargo xtask check                           formatting and lints, everywhere
//! cargo xtask test                            unit tests that run on this computer
//! cargo xtask ci                              all of the above, as GitHub Actions runs it
//! cargo xtask classic                         build and test PrettyBigRock 1.x (classic/)
//! cargo xtask doctor                          explain how the Rust toolchain was found
//! ```
//!
//! It uses nothing outside Rust's standard library, so it builds anywhere Rust does.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::Duration;

mod elf;
mod image;
mod qemu;

/// A processor family PrettyBigRock is built for.
struct Arch {
    /// Our name for it, as used in `kernel/src/arch/<name>` and `build/<name>`.
    name: &'static str,
    /// The Rust target the kernel is built with.
    kernel_target: &'static str,
    /// The Rust target the UEFI loader is built with.
    uefi_target: &'static str,
    /// The UEFI loader's file name on the EFI system partition.
    uefi_file: &'static str,
    /// The ELF machine number the kernel must have.
    elf_machine: u16,
}

/// Every architecture. Adding one starts here (docs/design/0001-repository-layout.md).
const ARCHES: &[Arch] = &[
    Arch {
        name: "x86_64",
        kernel_target: "x86_64-unknown-none",
        uefi_target: "x86_64-unknown-uefi",
        uefi_file: "BOOTX64.EFI",
        elf_machine: elf::EM_X86_64,
    },
    Arch {
        name: "aarch64",
        kernel_target: "aarch64-unknown-none-softfloat",
        uefi_target: "aarch64-unknown-uefi",
        uefi_file: "BOOTAA64.EFI",
        elf_machine: elf::EM_AARCH64,
    },
];

/// The BIOS loader's Rust target (x86-64 only).
const BIOS_TARGET: &str = "x86_64-unknown-none";

/// Where the kernel is linked (see kernel/src/arch/*/linker.ld).
const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

/// How long a boot test may take. Emulating ARM on a PC is slow, so this is generous.
const BOOT_TEST_LIMIT: Duration = Duration::from_secs(120);

type Result<T = ()> = std::result::Result<T, String>;

/// Extra `cargo` arguments, and environment variables, needed for one target.
type CargoExtras = (Vec<&'static str>, Vec<(&'static str, &'static str)>);

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result {
    let root = repo_root()?;
    let (command, rest) = args.split_first().map(|(c, r)| (c.as_str(), r)).unwrap_or(("help", &[]));
    let opts = Options::parse(rest)?;
    match command {
        "build" => build_all(&root, &Toolchain::find(&root)?, opts.release).map(|_| ()),
        "run" => {
            let img = build_all(&root, &Toolchain::find(&root)?, opts.release)?;
            let want = opts.arch.as_deref().unwrap_or("x86_64");
            let arch = ARCHES.iter().map(|a| a.name).find(|n| *n == want).unwrap_or("x86_64");
            let cfg =
                qemu::Config { name: "by hand", arch, bios: opts.bios, el2: opts.el2, expect: &[] };
            qemu::run(&cfg, &img, &work_dir(&root)?)
        }
        "qemu-test" => {
            let img = build_all(&root, &Toolchain::find(&root)?, opts.release)?;
            qemu_test(&root, &img)
        }
        "check" => check(&root, &Toolchain::find(&root)?),
        "test" => test(&root),
        "ci" => {
            let tc = Toolchain::find(&root)?;
            check(&root, &tc)?;
            test(&root)?;
            build_all(&root, &tc, true)?;
            let img = build_all(&root, &tc, false)?;
            qemu_test(&root, &img)?;
            println!("ci: all passed");
            Ok(())
        }
        "classic" => classic(&root),
        "doctor" => Toolchain::find(&root).map(|tc| tc.report()),
        "help" | "--help" | "-h" => {
            println!("{}", HELP.trim());
            Ok(())
        }
        other => Err(format!("unknown command `{other}`\n\n{}", HELP.trim())),
    }
}

const HELP: &str = "
PrettyBigRock build tool

  cargo xtask build [--release]          build everything, and the boot disk image
  cargo xtask run [--arch NAME] [--bios] [--el2] [--release]
                                         start PrettyBigRock in QEMU, in a window
  cargo xtask qemu-test [--release]      boot every configuration; check the kernel starts
  cargo xtask check                      formatting and lints
  cargo xtask test                       unit tests on this computer
  cargo xtask ci                         check, test, build and boot everything
  cargo xtask classic                    build and test PrettyBigRock 1.x
  cargo xtask doctor                     show how the Rust toolchain was found

Architectures: x86_64 (the default for `run`), aarch64
";

#[derive(Default)]
struct Options {
    arch: Option<String>,
    release: bool,
    bios: bool,
    el2: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut opts = Options::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--release" => opts.release = true,
                "--bios" => opts.bios = true,
                "--el2" => opts.el2 = true,
                "--arch" => {
                    let name = it.next().ok_or("--arch needs a name")?;
                    if !ARCHES.iter().any(|x| x.name == name) {
                        return Err(format!("unknown architecture `{name}`"));
                    }
                    opts.arch = Some(name.clone());
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        if opts.bios && opts.arch.as_deref() == Some("aarch64") {
            return Err("BIOS booting is for x86-64 only".into());
        }
        Ok(opts)
    }
}

/// Where QEMU's working files go.
fn work_dir(root: &Path) -> Result<PathBuf> {
    let d = root.join("build").join("qemu");
    fs::create_dir_all(&d).map_err(|e| e.to_string())?;
    Ok(d)
}

/// The repository's top folder: the parent of this crate.
fn repo_root() -> Result<PathBuf> {
    let manifest = env::var("CARGO_MANIFEST_DIR").map_err(|_| "run this through `cargo xtask`")?;
    Path::new(&manifest)
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "can't find the repository".to_string())
}

// ------------------------------------------------------------------------------------------
// The toolchain

/// The Rust toolchain, and how each kernel target's core library will be provided.
struct Toolchain {
    version: String,
    sysroot: PathBuf,
    /// The library source, if installed (the `rust-src` component).
    source: Option<PathBuf>,
}

impl Toolchain {
    /// Finds the toolchain and checks it's the pinned version.
    fn find(root: &Path) -> Result<Self> {
        let pinned = pinned_version(root)?;
        let version = output(Command::new("rustc").arg("--version"))?;
        if !version.starts_with(&format!("rustc {pinned} ")) {
            return Err(format!(
                "PrettyBigRock is built with Rust {pinned} (rust-toolchain.toml), but `rustc` is: \
                 {version}\nInstall it with `rustup toolchain install {pinned}`."
            ));
        }
        let sysroot = PathBuf::from(output(Command::new("rustc").args(["--print", "sysroot"]))?);
        let src = sysroot.join("lib/rustlib/src/rust/library");
        let source = src.join("core/Cargo.toml").is_file().then_some(src);
        Ok(Toolchain { version, sysroot, source })
    }

    /// True if the core library for `target` came ready-built with the toolchain.
    fn has_prebuilt(&self, target: &str) -> bool {
        self.sysroot.join("lib/rustlib").join(target).join("lib").is_dir()
    }

    /// Extra `cargo` arguments and environment for building for `target`.
    ///
    /// Normally the toolchain has the target ready-built. Where it can't be downloaded (as in
    /// Claude's workspace, whose network blocks static.rust-lang.org), Cargo builds the core
    /// library from the Rust source instead. That needs an unstable Cargo feature, enabled with
    /// RUSTC_BOOTSTRAP on this exact stable compiler. See docs/design/0002-coding-rules.md.
    fn target_args(&self, target: &str) -> Result<CargoExtras> {
        if self.has_prebuilt(target) {
            Ok((vec![], vec![]))
        } else if self.source.is_some() {
            Ok((
                vec![
                    "-Zbuild-std=core,alloc,compiler_builtins",
                    "-Zbuild-std-features=compiler-builtins-mem",
                ],
                vec![("RUSTC_BOOTSTRAP", "1")],
            ))
        } else {
            Err(format!(
                "the Rust toolchain has no core library for {target}, and no library source to \
                 build one from.\nInstall it with `rustup target add {target}`."
            ))
        }
    }

    fn report(&self) {
        println!("toolchain: {}", self.version);
        println!("sysroot:   {}", self.sysroot.display());
        for target in ARCHES.iter().flat_map(|a| [a.kernel_target, a.uefi_target]) {
            let how = if self.has_prebuilt(target) {
                "ready-built core library".to_string()
            } else if let Some(src) = &self.source {
                format!("core library built from source ({}), via RUSTC_BOOTSTRAP", src.display())
            } else {
                "MISSING: run `rustup target add`".to_string()
            };
            println!("  {target:<32} {how}");
        }
    }
}

/// The `channel` from rust-toolchain.toml.
fn pinned_version(root: &Path) -> Result<String> {
    let text = fs::read_to_string(root.join("rust-toolchain.toml"))
        .map_err(|e| format!("rust-toolchain.toml: {e}"))?;
    parse_channel(&text).ok_or_else(|| "rust-toolchain.toml has no `channel`".to_string())
}

fn parse_channel(toml: &str) -> Option<String> {
    toml.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "channel").then(|| value.trim().trim_matches('"').to_string())
    })
}

// ------------------------------------------------------------------------------------------
// The commands

/// Builds one binary for one target and returns its bytes.
fn build_artifact(
    root: &Path,
    tc: &Toolchain,
    package: &str,
    bin: &str,
    target: &str,
    release: bool,
) -> Result<Vec<u8>> {
    let (extra, envs) = tc.target_args(target)?;
    let mut cmd = cargo(root);
    cmd.args(["build", "--package", package, "--target", target]);
    if release {
        cmd.arg("--release");
    }
    cmd.args(&extra).envs(envs.iter().copied());
    status(&mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    let built = root.join("target").join(target).join(profile).join(bin);
    fs::read(&built).map_err(|e| format!("{}: {e}", built.display()))
}

fn build_kernel(root: &Path, tc: &Toolchain, arch: &Arch, release: bool) -> Result<Vec<u8>> {
    let bytes = build_artifact(root, tc, "pbr-kernel", "kernel", arch.kernel_target, release)?;
    let header =
        elf::Header::parse(&bytes).map_err(|e| format!("the {} kernel: {e}", arch.name))?;
    if header.machine != arch.elf_machine {
        return Err(format!(
            "the {} kernel was built for the wrong machine ({})",
            arch.name, header.machine
        ));
    }
    if header.entry < KERNEL_BASE {
        return Err(format!(
            "the {} kernel's entry point {:#x} isn't in the kernel's range",
            arch.name, header.entry
        ));
    }
    Ok(bytes)
}

fn write_out(root: &Path, rel: &str, bytes: &[u8]) -> Result<PathBuf> {
    let out = root.join(rel);
    if let Some(dir) = out.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("built {rel} ({} KiB)", bytes.len().div_ceil(1024));
    Ok(out)
}

/// Builds the kernels and loaders for every architecture, and the boot disk image
/// (docs/design/0004-loaders-and-boot-disk.md). Returns the image's path.
fn build_all(root: &Path, tc: &Toolchain, release: bool) -> Result<PathBuf> {
    let profile = if release { "release" } else { "debug" };
    let mut kernels = Vec::new();
    let mut loaders = Vec::new();
    for arch in ARCHES {
        let k = build_kernel(root, tc, arch, release)?;
        write_out(root, &format!("build/{}/{profile}/kernel.elf", arch.name), &k)?;
        let l = build_artifact(
            root,
            tc,
            "pbr-boot-uefi",
            "pbr-boot-uefi.efi",
            arch.uefi_target,
            release,
        )?;
        write_out(root, &format!("build/{}/{profile}/{}", arch.name, arch.uefi_file), &l)?;
        kernels.push(k);
        loaders.push(l);
    }
    let cmdline = b"";
    let (mbr, bios) = bios_loader(root, tc, release, &kernels[0], cmdline)?;
    let mut files: Vec<(String, &[u8])> = Vec::new();
    for (i, arch) in ARCHES.iter().enumerate() {
        files.push((format!("EFI/BOOT/{}", arch.uefi_file), &loaders[i]));
        files.push((format!("PBR/{}/kernel.elf", arch.name), &kernels[i]));
    }
    files.push(("PBR/cmdline.txt".to_string(), cmdline));
    let refs: Vec<(&str, &[u8])> = files.iter().map(|(p, d)| (p.as_str(), *d)).collect();
    let img =
        image::build(&image::Contents { mbr_code: &mbr, bios_partition: &bios, esp_files: &refs })?;
    write_out(root, &format!("build/{profile}/prettybigrock.img"), &img)
}

/// The BIOS loader: its first stage (for the MBR, with where to find the rest filled in) and
/// what goes in the BIOS boot partition (the rest of the loader, then the payload).
fn bios_loader(
    root: &Path,
    tc: &Toolchain,
    release: bool,
    kernel: &[u8],
    cmdline: &[u8],
) -> Result<(Vec<u8>, Vec<u8>)> {
    // The BIOS loader lives at low addresses, so it needs the "small" code model rather than
    // the kernel target's default. It builds in its own target folder so the different flags
    // don't make Cargo rebuild the kernel's dependencies every time.
    let (extra, envs) = tc.target_args(BIOS_TARGET)?;
    let mut cmd = cargo(root);
    cmd.args([
        "build",
        "--package",
        "pbr-boot-bios",
        "--target",
        BIOS_TARGET,
        "--target-dir",
        "target/bios",
    ]);
    if release {
        cmd.arg("--release");
    }
    cmd.args(&extra).envs(envs.iter().copied());
    cmd.env("RUSTFLAGS", "-C code-model=small -C relocation-model=static");
    status(&mut cmd)?;
    let profile = if release { "release" } else { "debug" };
    let path = root.join("target/bios").join(BIOS_TARGET).join(profile).join("pbr-boot-bios");
    let file = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let flat = elf::flat_binary(&file, 0x7C00).map_err(|e| format!("the BIOS loader: {e}"))?;
    if flat.len() <= 0x400 {
        return Err("the BIOS loader has no second stage".into());
    }
    let rest = &flat[0x400..]; // from 0x8000
    let sectors = rest.len().div_ceil(512);
    let mut mbr = flat[..432.min(flat.len())].to_vec();
    mbr.resize(440, 0);
    mbr[432..436].copy_from_slice(&(image::BIOS_PARTITION_LBA as u32).to_le_bytes());
    mbr[436..438].copy_from_slice(&(sectors as u16).to_le_bytes());

    let mut part = rest.to_vec();
    part.resize(sectors * 512, 0);
    part.extend_from_slice(&payload(&[("kernel.elf", kernel), ("cmdline.txt", cmdline)]));
    Ok((mbr, part))
}

/// The BIOS payload (note 0004): a 512-byte header listing the files, then the files, each
/// starting on a sector.
fn payload(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = vec![0u8; 512];
    out[..8].copy_from_slice(b"PBRPAYLD");
    out[8..12].copy_from_slice(&(files.len() as u32).to_le_bytes());
    for (i, (name, data)) in files.iter().enumerate() {
        let e = 16 + i * 48;
        let at = (out.len() / 512) as u32;
        out[e..e + 4].copy_from_slice(&at.to_le_bytes());
        out[e + 4..e + 8].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[e + 8..e + 8 + name.len()].copy_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }
    let total = (out.len() / 512) as u32;
    out[12..16].copy_from_slice(&total.to_le_bytes());
    out
}

fn qemu_test(root: &Path, img: &Path) -> Result {
    let work = work_dir(root)?;
    let mut failed = Vec::new();
    for cfg in qemu::TESTS {
        print!("boot test: {} ... ", cfg.name);
        use std::io::Write as _;
        let _ = std::io::stdout().flush();
        match qemu::test(cfg, img, &work, BOOT_TEST_LIMIT) {
            Ok(_) => println!("ok"),
            Err(e) => {
                println!("FAILED");
                eprintln!("{e}");
                failed.push(cfg.name);
            }
        }
    }
    if failed.is_empty() {
        println!("qemu-test: all {} configurations started", qemu::TESTS.len());
        Ok(())
    } else {
        Err(format!("boot tests failed: {}", failed.join(", ")))
    }
}

fn check(root: &Path, tc: &Toolchain) -> Result {
    status(cargo(root).args(["fmt", "--all", "--check"]))?;
    status(cargo(root).args([
        "clippy",
        "--package",
        "xtask",
        "--package",
        "pbr-bootinfo",
        "--package",
        "pbr-boot-common",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    let (extra, envs) = tc.target_args(BIOS_TARGET)?;
    status(
        cargo(root)
            .args([
                "clippy",
                "--package",
                "pbr-boot-bios",
                "--target",
                BIOS_TARGET,
                "--target-dir",
                "target/bios",
            ])
            .args(&extra)
            .envs(envs.iter().copied())
            .env("RUSTFLAGS", "-C code-model=small -C relocation-model=static")
            .args(["--", "-D", "warnings"]),
    )?;
    for arch in ARCHES {
        for (package, target) in
            [("pbr-kernel", arch.kernel_target), ("pbr-boot-uefi", arch.uefi_target)]
        {
            let (extra, envs) = tc.target_args(target)?;
            status(
                cargo(root)
                    .args(["clippy", "--package", package, "--target", target])
                    .args(&extra)
                    .envs(envs.iter().copied())
                    .args(["--", "-D", "warnings"]),
            )?;
        }
    }
    check_licence_headers(root)?;
    println!("check: all passed");
    Ok(())
}

fn test(root: &Path) -> Result {
    status(cargo(root).args([
        "test",
        "--package",
        "pbr-bootinfo",
        "--package",
        "pbr-boot-common",
        "--package",
        "xtask",
    ]))?;
    status(Command::new("python3").current_dir(root).arg("tools/test_rockfs.py"))?;
    println!("test: all passed");
    Ok(())
}

fn classic(root: &Path) -> Result {
    let dir = root.join("classic");
    status(Command::new("sh").current_dir(&dir).arg("build.sh"))?;
    let host = root.join("target").join("rfs_host");
    fs::create_dir_all(root.join("target")).map_err(|e| e.to_string())?;
    status(
        Command::new("gcc")
            .current_dir(&dir)
            .args(["-O1", "-Ikernel", "-include", "stdlib.h", "-o"])
            .arg(&host)
            .args(["tools/hosttest/rfs_host.c", "kernel/rockfs.c", "kernel/disk.c"]),
    )?;
    let work = env::temp_dir().join("pbr-crosscheck");
    fs::create_dir_all(&work).map_err(|e| e.to_string())?;
    status(
        Command::new("python3")
            .current_dir(&dir)
            .arg("tools/hosttest/crosscheck.py")
            .arg(&host)
            .arg(&work),
    )?;
    println!("classic: built and cross-checked");
    Ok(())
}

/// Every source file must start with an SPDX licence line (docs/design/0002-coding-rules.md).
fn check_licence_headers(root: &Path) -> Result {
    let mut missing = Vec::new();
    for dir in ["boot", "kernel", "lib", "xtask"] {
        walk(&root.join(dir), &mut |path| {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "rs" | "ld" | "toml" | "s") {
                let text = fs::read_to_string(path).unwrap_or_default();
                if !text.lines().take(3).any(|l| l.contains("SPDX-License-Identifier:")) {
                    missing.push(path.strip_prefix(root).unwrap_or(path).display().to_string());
                }
            }
        });
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!("these files have no SPDX licence line:\n  {}", missing.join("\n  ")))
    }
}

fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, f);
        } else {
            f(&path);
        }
    }
}

// ------------------------------------------------------------------------------------------
// Running things

fn cargo(root: &Path) -> Command {
    let mut cmd = Command::new(env::var("CARGO").unwrap_or_else(|_| "cargo".into()));
    cmd.current_dir(root);
    cmd
}

fn status(cmd: &mut Command) -> Result {
    let shown = format!("{cmd:?}");
    let st = cmd.status().map_err(|e| format!("couldn't run {shown}: {e}"))?;
    if st.success() { Ok(()) } else { Err(format!("failed: {shown}")) }
}

fn output(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().map_err(|e| format!("couldn't run {cmd:?}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{cmd:?} failed: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_channel() {
        let toml = "[toolchain]\n# a comment\nchannel = \"1.97.0\"\ncomponents = []\n";
        assert_eq!(parse_channel(toml).as_deref(), Some("1.97.0"));
        assert_eq!(parse_channel("[toolchain]\n"), None);
    }

    #[test]
    fn every_arch_has_a_kernel_module_and_linker_script() {
        let root = repo_root().unwrap();
        for arch in ARCHES {
            let dir = root.join("kernel/src/arch").join(arch.name);
            assert!(dir.join("mod.rs").is_file(), "{} has no mod.rs", arch.name);
            assert!(dir.join("linker.ld").is_file(), "{} has no linker.ld", arch.name);
        }
    }

    #[test]
    fn options() {
        let o = Options::parse(&["--arch".into(), "aarch64".into(), "--release".into()]).unwrap();
        assert!(o.release);
        assert_eq!(o.arch.as_deref(), Some("aarch64"));
        assert!(Options::parse(&["--arch".into(), "z80".into()]).is_err());
        assert!(Options::parse(&["--nonsense".into()]).is_err());
        assert!(Options::parse(&["--arch".into(), "aarch64".into(), "--bios".into()]).is_err());
    }
}
