// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The disks the boot tests attach for the storage service's tests
//! (docs/design/0018-supervision-shell-and-m5-tests.md, section 7, test 2):
//! - `test-mbr.img`: an MBR, a RockFS volume labelled `Data` (8 MiB from 1 MiB) and a FAT32
//!   volume labelled `USB` (34 MiB);
//! - `test-gpt.img`: a GPT, a RockFS volume labelled `Home` (4 MiB) and a FAT32 volume (34 MiB)
//!   typed as basic data, labelled `PICTURES`;
//! - `test-empty.img`: 2 MiB of zeros.
//!
//! RockFS volumes are formatted by `tools/rockfs.py`, the independent Python implementation, so
//! the storage service's identification is checked against a second writer. FAT32 volumes come
//! from the build tool's own writer (`image::fat32`).

use crate::image;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const SECTOR: usize = 512;
const MIB: u64 = 2048; // sectors

/// Where the MBR test disk's volumes are: (start, sectors).
pub const MBR_ROCKFS: (u64, u64) = (MIB, 8 * MIB);
pub const MBR_FAT: (u64, u64) = (9 * MIB, 34 * MIB);
/// And the GPT test disk's.
pub const GPT_ROCKFS: (u64, u64) = (MIB, 4 * MIB);
pub const GPT_FAT: (u64, u64) = (5 * MIB, 34 * MIB);

/// A README on each FAT32 test volume, so it isn't empty.
const README: &[u8] = b"A FAT32 test volume for PrettyBigRock's storage tests.\n";

/// Formats `sectors` sectors of `path` from `start` as RockFS, with `tools/rockfs.py`.
fn rockfs(path: &Path, start: u64, sectors: u64, label: &str) -> Result<(), String> {
    let tools = concat!(env!("CARGO_MANIFEST_DIR"), "/../tools");
    let script = format!(
        "import sys; sys.path.insert(0, {tools:?}); import rockfs\n\
         with open({p:?}, 'r+b') as f: rockfs.format_volume(f, {o}, {l}, {label:?})",
        p = path.display().to_string(),
        o = start * SECTOR as u64,
        l = sectors * SECTOR as u64,
    );
    let st = Command::new("python3")
        .args(["-I", "-c", &script])
        .status()
        .map_err(|e| format!("running python3: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("tools/rockfs.py couldn't format {}", path.display()))
    }
}

fn put(disk: &mut [u8], sector: u64, bytes: &[u8]) {
    let at = sector as usize * SECTOR;
    disk[at..at + bytes.len()].copy_from_slice(bytes);
}

/// An MBR entry: type, first sector, sectors.
fn mbr_entry(disk: &mut [u8], i: usize, kind: u8, start: u64, len: u64) {
    let e = 446 + 16 * i;
    disk[e + 4] = kind;
    disk[e + 8..e + 12].copy_from_slice(&(start as u32).to_le_bytes());
    disk[e + 12..e + 16].copy_from_slice(&(len as u32).to_le_bytes());
    disk[510] = 0x55;
    disk[511] = 0xAA;
}

/// Writes the MBR test disk.
pub fn mbr_disk(path: &Path) -> Result<(), String> {
    let total = MBR_FAT.0 + MBR_FAT.1;
    let mut disk = vec![0u8; total as usize * SECTOR];
    mbr_entry(&mut disk, 0, 0x7F, MBR_ROCKFS.0, MBR_ROCKFS.1);
    mbr_entry(&mut disk, 1, 0x0C, MBR_FAT.0, MBR_FAT.1);
    let fat =
        image::fat32_labelled(&[("README.TXT", README)], MBR_FAT.0, MBR_FAT.1, b"USB        ")?;
    put(&mut disk, MBR_FAT.0, &fat);
    fs::write(path, &disk).map_err(|e| format!("writing {}: {e}", path.display()))?;
    rockfs(path, MBR_ROCKFS.0, MBR_ROCKFS.1, "Data")
}

/// Writes the GPT test disk.
pub fn gpt_disk(path: &Path) -> Result<(), String> {
    let total = GPT_FAT.0 + GPT_FAT.1 + MIB; // room for the backup GPT
    let mut disk = vec![0u8; total as usize * SECTOR];
    image::protective_mbr(&mut disk[..SECTOR], total);
    let parts: [image::GptPart<'_>; 2] = [
        (
            "03971446-6A02-47DC-BB5F-6A7572E5247C", // RockFS
            "7B2E5C10-0000-4000-8000-524F434B4653",
            GPT_ROCKFS.0,
            GPT_ROCKFS.1,
            "Home",
        ),
        (
            "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7", // basic data
            "7B2E5C11-0000-4000-8000-464154333220",
            GPT_FAT.0,
            GPT_FAT.1,
            "Pictures",
        ),
    ];
    image::gpt_table(&mut disk, total, &parts);
    let fat =
        image::fat32_labelled(&[("README.TXT", README)], GPT_FAT.0, GPT_FAT.1, b"PICTURES   ")?;
    put(&mut disk, GPT_FAT.0, &fat);
    fs::write(path, &disk).map_err(|e| format!("writing {}: {e}", path.display()))?;
    rockfs(path, GPT_ROCKFS.0, GPT_ROCKFS.1, "Home")
}

/// Writes every storage test disk into `work`, and returns their paths.
pub fn make(work: &Path) -> Result<Vec<PathBuf>, String> {
    let (mbr, gpt, empty) =
        (work.join("test-mbr.img"), work.join("test-gpt.img"), work.join("test-empty.img"));
    mbr_disk(&mbr)?;
    gpt_disk(&gpt)?;
    fs::write(&empty, vec![0u8; MIB as usize * 2 * SECTOR]).map_err(|e| format!("{e}"))?;
    Ok(vec![mbr, gpt, empty])
}

#[cfg(test)]
mod tests {
    use super::*;
    use pbr_partition::{Filesystem, Table};

    struct File(Vec<u8>);

    impl pbr_partition::Disk for File {
        fn sectors(&self) -> u64 {
            (self.0.len() / SECTOR) as u64
        }
        fn read(&mut self, sector: u64, buf: &mut [u8]) -> Result<(), pbr_partition::ReadError> {
            let at = sector as usize * SECTOR;
            buf.copy_from_slice(&self.0[at..at + buf.len()]);
            Ok(())
        }
    }

    /// The test disks are what the storage self-test expects, as `lib/partition` reads them:
    /// a check of both against each other, and of `tools/rockfs.py`'s formatting.
    #[test]
    fn the_test_disks() {
        let dir = std::env::temp_dir().join(format!("pbr-testdisks-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let paths = make(&dir).unwrap();
        let scan = |p: &Path| pbr_partition::scan(&mut File(fs::read(p).unwrap())).unwrap();
        let summary = |s: &pbr_partition::Scan| -> Vec<(u32, u64, u64, Filesystem, String)> {
            s.volumes
                .iter()
                .map(|v| (v.partition, v.start, v.sectors, v.filesystem, v.label.clone()))
                .collect()
        };
        let m = scan(&paths[0]);
        assert_eq!(m.table, Table::Mbr);
        assert_eq!(
            summary(&m),
            [
                (1, MBR_ROCKFS.0, MBR_ROCKFS.1, Filesystem::RockFs, "Data".into()),
                (2, MBR_FAT.0, MBR_FAT.1, Filesystem::Fat32, "USB".into()),
            ]
        );
        let g = scan(&paths[1]);
        assert_eq!((g.table, g.problems.clone()), (Table::Gpt, vec![]));
        assert_eq!(
            summary(&g),
            [
                (1, GPT_ROCKFS.0, GPT_ROCKFS.1, Filesystem::RockFs, "Home".into()),
                (2, GPT_FAT.0, GPT_FAT.1, Filesystem::Fat32, "PICTURES".into()),
            ]
        );
        let e = scan(&paths[2]);
        assert_eq!(summary(&e), [(0, 0, 2 * MIB, Filesystem::Empty, String::new())]);
        let _ = fs::remove_dir_all(&dir);
    }
}
