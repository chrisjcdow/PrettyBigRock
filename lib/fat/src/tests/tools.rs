// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Running `mkfs.fat`, `fsck.fat` and `mtools` (the `dosfstools` and `mtools` packages): other
//! systems' FAT programs, used only in tests as independent checks of what we read and write.
//! They are not part of PrettyBigRock.
//!
//! A test that needs them calls `need_tools!` first: if one is missing it prints a line saying
//! so and the test ends, passing, so that the pure-Rust tests still run on a computer without
//! them. (CLAUDE.md says a stage is not finished until these have run.)

use super::{Tree, folded};
use pbr_fs::memdisk::MemDisk;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::string::{String, ToString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::vec::Vec;

/// Where `name` is, looking in the usual places (the `sbin` folders are not always on a user's
/// path).
pub fn find(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&path).collect();
    dirs.extend(["/usr/sbin", "/sbin", "/usr/local/bin"].map(PathBuf::from));
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// Whether all of `names` are installed; if not, says which and why the test is skipped.
pub fn have(test: &str, names: &[&str]) -> bool {
    let missing: Vec<&&str> = names.iter().filter(|n| find(n).is_none()).collect();
    if missing.is_empty() {
        return true;
    }
    // Written straight to standard error: the test harness only hides what `eprintln!` writes.
    let _ = writeln!(
        std::io::stderr(),
        "SKIPPED {test}: needs {missing:?}, which is not installed (apt install dosfstools mtools)"
    );
    false
}

/// Ends the test, passing, with a message, if a tool it needs is missing.
macro_rules! need_tools {
    ($($tool:expr),+) => {
        if !$crate::tests::tools::have(module_path!(), &[$($tool),+]) {
            return;
        }
    };
}
pub(crate) use need_tools;

/// A folder that is removed, with everything in it, when this is dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> TempDir {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(std::format!(
            "pbr-fat-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path).unwrap();
        TempDir(path)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// What a tool printed, and whether it succeeded.
pub struct Output {
    pub ok: bool,
    pub text: String,
}

fn run(tool: &str, args: &[&str]) -> Output {
    let program = find(tool).unwrap_or_else(|| PathBuf::from(tool));
    let out = Command::new(program)
        .args(args)
        // `mtools` checks the disk's geometry against a table unless told not to; it also
        // needs a UTF-8 locale to convert non-ASCII names.
        .env("MTOOLS_SKIP_CHECK", "1")
        .env("LC_ALL", "C.UTF-8")
        .output()
        .unwrap_or_else(|e| panic!("cannot run {tool}: {e}"));
    let mut text = String::from_utf8_lossy(&out.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&out.stderr));
    Output { ok: out.status.success(), text }
}

/// A disk image in a file, made and used by the outside tools.
pub struct Image {
    dir: TempDir,
    pub file: PathBuf,
}

impl Image {
    /// A new FAT32 volume of `kib` KiB, made by `mkfs.fat -F 32` with `extra` arguments.
    pub fn mkfs(kib: u32, extra: &[&str]) -> Image {
        let dir = TempDir::new();
        let file = dir.path().join("disk.img");
        let file_name = file.to_string_lossy().to_string();
        let size = kib.to_string();
        let mut args = std::vec!["-F", "32"];
        args.extend_from_slice(extra);
        args.extend(["-C", &file_name, &size]);
        let out = run("mkfs.fat", &args);
        assert!(out.ok, "mkfs.fat failed: {}", out.text);
        Image { dir, file }
    }

    /// Runs an `mtools` command on the image, which must succeed. `args` follow `-i image`.
    pub fn mtools(&self, tool: &str, args: &[&str]) -> String {
        let file = self.file.to_string_lossy().to_string();
        let mut all = std::vec!["-i", &file];
        all.extend_from_slice(args);
        let out = run(tool, &all);
        assert!(out.ok, "{tool} {args:?} failed: {}", out.text);
        out.text
    }

    /// Copies everything in the host folder `dir` into the root folder of the image.
    pub fn copy_tree(&self, dir: &Path) {
        let mut entries: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().path().to_string_lossy().to_string())
            .collect();
        entries.sort();
        let mut args: Vec<&str> = std::vec!["-s", "-m"];
        args.extend(entries.iter().map(|e| e.as_str()));
        args.push("::");
        self.mtools("mcopy", &args);
    }

    /// Writes `data` as a file called `name` (a path within the image, starting `::/`).
    pub fn put(&self, name: &str, data: &[u8]) {
        let source = self.dir.path().join("source.tmp");
        std::fs::write(&source, data).unwrap();
        self.mtools("mcopy", &["-o", &source.to_string_lossy(), name]);
    }

    /// The image as a disk in memory.
    pub fn disk(&self) -> MemDisk {
        MemDisk::from(std::fs::read(&self.file).unwrap())
    }

    /// Replaces the image with the contents of `disk`.
    pub fn store(&self, disk: &MemDisk) {
        std::fs::write(&self.file, &disk.data).unwrap();
    }

    /// Runs `fsck.fat -n` (check, never change) on the image.
    pub fn fsck(&self) -> Output {
        run("fsck.fat", &["-n", &self.file.to_string_lossy()])
    }

    /// Asserts `fsck.fat` finds nothing wrong: it succeeds and prints only its banner and the
    /// summary line (`image: 5 files, 5/78736 clusters`).
    pub fn assert_clean(&self, what: &str) {
        let out = self.fsck();
        let lines: Vec<&str> = out.text.lines().filter(|l| !l.starts_with("fsck.fat ")).collect();
        let summary =
            lines.len() == 1 && lines[0].contains(" files, ") && lines[0].contains(" clusters");
        assert!(out.ok && summary, "{what}: fsck.fat -n found problems:\n{}", out.text);
    }

    /// Everything in the image, read out by `mtools` (`mcopy -s`) into a folder and walked.
    pub fn export(&self) -> Tree {
        let to = self.dir.path().join("export");
        let _ = std::fs::remove_dir_all(&to);
        std::fs::create_dir_all(&to).unwrap();
        self.mtools("mcopy", &["-s", "-n", "-m", "::", &to.to_string_lossy()]);
        let mut tree = Tree::new();
        walk_host(&to, "", &mut tree);
        tree
    }

    /// Asserts the image holds exactly `expected`, as `mtools` reads it. Names are compared
    /// ignoring A-Z case, since `mtools` may spell a short name in capitals.
    pub fn assert_holds(&self, expected: &Tree, what: &str) {
        let got = folded(&self.export());
        let want = folded(expected);
        assert_eq!(
            got.keys().collect::<Vec<_>>(),
            want.keys().collect::<Vec<_>>(),
            "{what}: the names mtools sees"
        );
        for (name, data) in &want {
            assert!(got[name] == *data, "{what}: contents of {name}");
        }
    }
}

fn walk_host(dir: &Path, prefix: &str, tree: &mut Tree) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().to_string_lossy().to_string();
        let path = std::format!("{prefix}/{name}");
        if entry.file_type().unwrap().is_dir() {
            tree.insert(path.clone(), None);
            walk_host(&entry.path(), &path, tree);
        } else {
            tree.insert(path, Some(std::fs::read(entry.path()).unwrap()));
        }
    }
}
