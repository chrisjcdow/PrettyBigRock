// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Intel's microcode updates on the boot disk (docs/design/0027-kernel-1.4.md, section 1e;
//! note 0031, section 6).
//!
//! The files are never in the repository: they would stay in git's history for ever, growing
//! with each of Intel's releases. `cargo xtask microcode` fetches Intel's published release at
//! the tag below, checks it against the SHA-256 sum recorded beside the tag, and unpacks it in
//! `build/microcode/`. A build given `PBR_MICROCODE=<folder>` (that one, or Ubuntu's
//! `/lib/firmware/intel-ucode`) then puts every file on the EFI system partition, for the
//! UEFI loader, and after the BIOS payload, for the BIOS loader. Without it nothing changes.
//!
//! The tools used, `curl`, `sha256sum` and `tar`, run on the machine that builds and put
//! nothing of theirs into PrettyBigRock.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::{env, fs};

/// The release of `github.com/intel/Intel-Linux-Processor-Microcode-Data-Files` the release
/// images carry. Moving to a newer one is this line and the sum below, reviewed like any other
/// change.
pub const TAG: &str = "microcode-20261110";
/// The SHA-256 sum of [`TAG`]'s archive, as GitHub serves it.
pub const SHA256: &str = "989a5cd345808babee491da69ed80ae5741697bc87743a71d0ba611f3fd6afa5";

/// What `PBR_MICROCODE` named: Intel's files, by name, and Intel's licence for them.
pub struct Microcode {
    /// (file name such as `06-2a-07`, the CPUID signature it is for, its contents), by name.
    pub files: Vec<(String, u32, Vec<u8>)>,
    /// The licence text, if it was beside the files.
    pub licence: Option<Vec<u8>>,
}

/// `cargo xtask microcode`: fetches and checks [`TAG`], and unpacks its `intel-ucode` folder
/// and licence in `build/microcode/`.
pub fn fetch(root: &Path) -> Result<(), String> {
    let out = root.join("build/microcode");
    fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let archive = out.join(format!("{TAG}.tar.gz"));
    let url = format!(
        "https://github.com/intel/Intel-Linux-Processor-Microcode-Data-Files/archive/refs/tags/{TAG}.tar.gz"
    );
    println!("microcode: fetching {url}");
    let ok = Command::new("curl")
        .args(["--proto", "=https", "--tlsv1.2", "-sSfL", "-o"])
        .arg(&archive)
        .arg(&url)
        .status()
        .map_err(|e| format!("can't run curl: {e}"))?
        .success();
    if !ok {
        return Err("microcode: the download failed".into());
    }
    let sum = Command::new("sha256sum")
        .arg(&archive)
        .output()
        .map_err(|e| format!("can't run sha256sum: {e}"))?;
    let got = String::from_utf8_lossy(&sum.stdout);
    let got = got.split_whitespace().next().unwrap_or("");
    if got != SHA256 {
        let _ = fs::remove_file(&archive);
        return Err(format!(
            "microcode: {TAG}'s archive has SHA-256 {got}, not the {SHA256} recorded; nothing \
             was unpacked"
        ));
    }
    // Unpack only the two things wanted, into a fresh folder, then put them in place. GNU tar
    // refuses names that would climb out of the folder.
    let unpack = out.join("unpack");
    let _ = fs::remove_dir_all(&unpack);
    fs::create_dir_all(&unpack).map_err(|e| e.to_string())?;
    let prefix = format!("Intel-Linux-Processor-Microcode-Data-Files-{TAG}");
    let ok = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .arg("-C")
        .arg(&unpack)
        .args(["--strip-components=1", "--no-same-owner", "--no-same-permissions"])
        .arg(format!("{prefix}/intel-ucode"))
        .arg(format!("{prefix}/license"))
        .status()
        .map_err(|e| format!("can't run tar: {e}"))?
        .success();
    if !ok {
        return Err("microcode: unpacking failed".into());
    }
    let ucode = out.join("intel-ucode");
    let _ = fs::remove_dir_all(&ucode);
    fs::rename(unpack.join("intel-ucode"), &ucode).map_err(|e| e.to_string())?;
    fs::rename(unpack.join("license"), ucode.join("license")).map_err(|e| e.to_string())?;
    let _ = fs::remove_dir_all(&unpack);
    let m = read(&ucode)?;
    println!(
        "microcode: Intel's {TAG}, {} files, in build/microcode/intel-ucode; build with \
         PBR_MICROCODE=build/microcode/intel-ucode",
        m.files.len()
    );
    Ok(())
}

/// The microcode `PBR_MICROCODE` names (relative to the repository), or `None` without it.
pub fn from_env(root: &Path) -> Result<Option<Microcode>, String> {
    match env::var("PBR_MICROCODE") {
        Ok(d) if !d.is_empty() => read(&root.join(d)).map(Some),
        _ => Ok(None),
    }
}

/// Reads Intel's files from `dir`: every file named like `06-2a-07` (others, such as Ubuntu's
/// `.initramfs` copies, are left out), and the licence beside them or one folder up.
fn read(dir: &Path) -> Result<Microcode, String> {
    let entries =
        fs::read_dir(dir).map_err(|e| format!("PBR_MICROCODE: {}: {e}", dir.display()))?;
    let mut files = Vec::new();
    for e in entries.flatten() {
        let Ok(name) = e.file_name().into_string() else { continue };
        let Some(signature) = pbr_microcode::signature_of_name(&name) else { continue };
        let data = fs::read(e.path()).map_err(|x| format!("{}: {x}", e.path().display()))?;
        files.push((name, signature, data));
    }
    if files.is_empty() {
        return Err(format!("PBR_MICROCODE: no microcode files in {}", dir.display()));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let candidates: [PathBuf; 3] =
        [dir.join("license"), dir.join("LICENSE"), dir.join("../license")];
    let licence = candidates.iter().find_map(|p| fs::read(p).ok());
    Ok(Microcode { files, licence })
}

/// The files for the EFI system partition: `PBR/x86_64/ucode/<name>`, and the licence.
pub fn esp_files(m: &Microcode) -> Vec<(String, &[u8])> {
    let mut out: Vec<(String, &[u8])> =
        m.files.iter().map(|(n, _, d)| (format!("PBR/x86_64/ucode/{n}"), &d[..])).collect();
    if let Some(l) = &m.licence {
        out.push(("PBR/x86_64/ucode/license.txt".into(), &l[..]));
    }
    out
}

/// Where the BIOS payload's header records the microcode table: its first sector (from the
/// header) and how many entries it has (note 0027, section 1e).
pub const HEADER_TABLE: usize = 448;

/// Appends the microcode to a BIOS `payload` (whose header says how many sectors the rest
/// takes, all of which the second stage copies): a table of 12-byte entries, (signature masked
/// with `0x0FFF_0FFF`, first sector from the header, length in bytes), then each file from a
/// sector of its own. The second stage reads the table and only the file that matches.
pub fn append_to_payload(payload: &mut Vec<u8>, m: &Microcode) {
    let table_at = payload.len() / 512;
    let table_sectors = (m.files.len() * 12).div_ceil(512);
    let mut table = vec![0u8; table_sectors * 512];
    let mut data = Vec::new();
    let mut at = table_at + table_sectors;
    for (i, (_, signature, bytes)) in m.files.iter().enumerate() {
        let e = &mut table[i * 12..i * 12 + 12];
        e[0..4].copy_from_slice(&(signature & 0x0FFF_0FFF).to_le_bytes());
        e[4..8].copy_from_slice(&(at as u32).to_le_bytes());
        e[8..12].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        data.extend_from_slice(bytes);
        data.resize(data.len().div_ceil(512) * 512, 0);
        at = table_at + table_sectors + data.len() / 512;
    }
    payload[HEADER_TABLE..HEADER_TABLE + 4].copy_from_slice(&(table_at as u32).to_le_bytes());
    payload[HEADER_TABLE + 4..HEADER_TABLE + 8]
        .copy_from_slice(&(m.files.len() as u32).to_le_bytes());
    payload.extend_from_slice(&table);
    payload.extend_from_slice(&data);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bios_table() {
        let m = Microcode {
            files: vec![
                ("06-2a-07".into(), 0x206A7, vec![1; 700]),
                ("06-9e-09".into(), 0x906E9, vec![2; 512]),
            ],
            licence: None,
        };
        let mut p = vec![0u8; 3 * 512]; // a header and two sectors of files
        append_to_payload(&mut p, &m);
        let u32_at = |o: usize| u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        assert_eq!((u32_at(HEADER_TABLE), u32_at(HEADER_TABLE + 4)), (3, 2));
        let t = 3 * 512;
        assert_eq!((u32_at(t), u32_at(t + 4), u32_at(t + 8)), (0x206A7, 4, 700));
        assert_eq!((u32_at(t + 12), u32_at(t + 16), u32_at(t + 20)), (0x906E9, 6, 512));
        assert_eq!(p.len(), 7 * 512);
        assert_eq!(p[4 * 512], 1);
        assert_eq!(p[6 * 512], 2);
    }

    #[test]
    fn the_pinned_sum_looks_like_one() {
        assert_eq!(SHA256.len(), 64);
        assert!(SHA256.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert!(TAG.starts_with("microcode-"));
    }
}
