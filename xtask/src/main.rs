// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! PrettyBigRock's build tool. Run it through Cargo:
//!
//! ```text
//! cargo xtask build [--arch NAME] [--release]   build the kernel for one or all architectures
//! cargo xtask check                             formatting and lints, everywhere
//! cargo xtask test                              unit tests that run on this computer
//! cargo xtask ci                                everything above, as GitHub Actions runs it
//! cargo xtask classic                           build and test PrettyBigRock 1.x (classic/)
//! cargo xtask doctor                            explain how the Rust toolchain was found
//! ```
//!
//! It uses nothing outside Rust's standard library, so it builds anywhere Rust does.

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

mod elf;

/// A processor family PrettyBigRock is built for.
struct Arch {
    /// Our name for it, as used in `kernel/src/arch/<name>` and `build/<name>`.
    name: &'static str,
    /// The Rust target the kernel is built with.
    kernel_target: &'static str,
    /// The ELF machine number the kernel must have.
    elf_machine: u16,
}

/// Every architecture. Adding one starts here (docs/design/0001-repository-layout.md).
const ARCHES: &[Arch] = &[
    Arch { name: "x86_64", kernel_target: "x86_64-unknown-none", elf_machine: elf::EM_X86_64 },
    Arch {
        name: "aarch64",
        kernel_target: "aarch64-unknown-none-softfloat",
        elf_machine: elf::EM_AARCH64,
    },
];

/// Where the kernel is linked (see kernel/src/arch/*/linker.ld).
const KERNEL_BASE: u64 = 0xFFFF_FFFF_8000_0000;

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
        "build" => {
            let tc = Toolchain::find(&root)?;
            for arch in opts.arches()? {
                build_kernel(&root, &tc, arch, opts.release)?;
            }
            Ok(())
        }
        "check" => check(&root, &Toolchain::find(&root)?),
        "test" => test(&root),
        "ci" => {
            let tc = Toolchain::find(&root)?;
            check(&root, &tc)?;
            test(&root)?;
            for arch in ARCHES {
                build_kernel(&root, &tc, arch, false)?;
                build_kernel(&root, &tc, arch, true)?;
            }
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

  cargo xtask build [--arch NAME] [--release]   build the kernel (default: every architecture)
  cargo xtask check                             formatting and lints
  cargo xtask test                              unit tests on this computer
  cargo xtask ci                                check, test, and build everything
  cargo xtask classic                           build and test PrettyBigRock 1.x
  cargo xtask doctor                            show how the Rust toolchain was found

Architectures: x86_64, aarch64
";

#[derive(Default)]
struct Options {
    arch: Option<String>,
    release: bool,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut opts = Options::default();
        let mut it = args.iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--release" => opts.release = true,
                "--arch" => {
                    opts.arch = Some(it.next().ok_or("--arch needs a name")?.clone());
                }
                other => return Err(format!("unknown option `{other}`")),
            }
        }
        Ok(opts)
    }

    fn arches(&self) -> Result<Vec<&'static Arch>> {
        match &self.arch {
            None => Ok(ARCHES.iter().collect()),
            Some(name) => ARCHES
                .iter()
                .find(|a| a.name == name)
                .map(|a| vec![a])
                .ok_or_else(|| format!("unknown architecture `{name}`")),
        }
    }
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
        for arch in ARCHES {
            let how = if self.has_prebuilt(arch.kernel_target) {
                "ready-built core library".to_string()
            } else if let Some(src) = &self.source {
                format!("core library built from source ({}), via RUSTC_BOOTSTRAP", src.display())
            } else {
                "MISSING: run `rustup target add`".to_string()
            };
            println!("{:<8}   {}: {how}", arch.name, arch.kernel_target);
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

fn build_kernel(root: &Path, tc: &Toolchain, arch: &Arch, release: bool) -> Result {
    let (extra, envs) = tc.target_args(arch.kernel_target)?;
    let mut cmd = cargo(root);
    cmd.args(["build", "--package", "pbr-kernel", "--target", arch.kernel_target]);
    if release {
        cmd.arg("--release");
    }
    cmd.args(&extra).envs(envs.iter().copied());
    status(&mut cmd)?;

    let profile = if release { "release" } else { "debug" };
    let built = root.join("target").join(arch.kernel_target).join(profile).join("kernel");
    let bytes = fs::read(&built).map_err(|e| format!("{}: {e}", built.display()))?;
    let header = elf::Header::parse(&bytes).map_err(|e| format!("{}: {e}", built.display()))?;
    if header.machine != arch.elf_machine {
        return Err(format!(
            "{}: built for the wrong machine ({})",
            built.display(),
            header.machine
        ));
    }
    if header.entry < KERNEL_BASE {
        return Err(format!(
            "{}: entry point {:#x} isn't in the kernel's address range",
            built.display(),
            header.entry
        ));
    }

    let out_dir = root.join("build").join(arch.name).join(profile);
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;
    let out = out_dir.join("kernel.elf");
    fs::write(&out, &bytes).map_err(|e| e.to_string())?;
    println!(
        "built {} ({} bytes, entry {:#x})",
        out.strip_prefix(root).unwrap_or(&out).display(),
        bytes.len(),
        header.entry
    );
    Ok(())
}

fn check(root: &Path, tc: &Toolchain) -> Result {
    status(cargo(root).args(["fmt", "--all", "--check"]))?;
    status(cargo(root).args([
        "clippy",
        "--package",
        "xtask",
        "--package",
        "pbr-bootinfo",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]))?;
    for arch in ARCHES {
        let (extra, envs) = tc.target_args(arch.kernel_target)?;
        status(
            cargo(root)
                .args(["clippy", "--package", "pbr-kernel", "--target", arch.kernel_target])
                .args(&extra)
                .envs(envs.iter().copied())
                .args(["--", "-D", "warnings"]),
        )?;
    }
    check_licence_headers(root)?;
    println!("check: all passed");
    Ok(())
}

fn test(root: &Path) -> Result {
    status(cargo(root).args(["test", "--package", "pbr-bootinfo", "--package", "xtask"]))?;
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
    for dir in ["kernel", "lib", "xtask"] {
        walk(&root.join(dir), &mut |path| {
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if matches!(ext, "rs" | "ld" | "toml") {
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
        assert_eq!(o.arches().unwrap().len(), 1);
        assert_eq!(Options::default().arches().unwrap().len(), ARCHES.len());
        assert!(Options::parse(&["--arch".into(), "z80".into()]).unwrap().arches().is_err());
        assert!(Options::parse(&["--nonsense".into()]).is_err());
    }
}
