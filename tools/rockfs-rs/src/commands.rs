// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The ordinary commands: the same ones, with the same output, as `tools/rockfs.py`.

use crate::disk::{FileDisk, MBR_TYPE, PART_START, SECTOR, open_volume};
use crate::tree::{entries, explain, read_file, resolve, resolve_parent, write_at};
use crate::util::{date_time, describe, human, new_uuid, now, uuid_text};
use pbr_fs::{BlockDevice, Filesystem, Kind};
use pbr_rockfs::RockFs;
use std::io::Write;

/// What a command reports: an error message for the person (printed as "rockfs: message").
pub type Outcome = Result<i32, String>;

/// A mounted volume on an image file.
pub type Volume = RockFs<FileDisk>;

/// Mounts the volume in `image`; `writable` for commands that change it.
pub fn mount(image: &str, writable: bool) -> Result<Volume, String> {
    let disk = open_volume(image, writable)?;
    let fs = RockFs::mount(disk, now, !writable).map_err(|e| e.to_string())?;
    if writable && fs.read_only() {
        return Err(describe(pbr_fs::FsError::ReadOnly).into());
    }
    Ok(fs)
}

/// Unmounts cleanly and makes sure the image file is safely stored.
pub fn close(mut fs: Volume) -> Result<(), String> {
    if !fs.read_only() {
        fs.unmount().map_err(|e| format!("unmounting: {}", describe(e)))?;
    }
    fs.into_device().sync();
    Ok(())
}

/// `mkfs [--size N[K|M|G]] [--label L] [--whole-disk]`
pub fn mkfs(image: &str, args: &[String]) -> Outcome {
    let (mut size, mut label, mut whole) = (32u64 << 20, String::from("Data"), false);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--size" => size = parse_size(it.next().ok_or("--size needs a value")?)?,
            "--label" => label = it.next().ok_or("--label needs a value")?.clone(),
            "--whole-disk" => whole = true,
            other => return Err(format!("unknown option {other}")),
        }
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(image)
        .map_err(|e| format!("{image}: {e}"))?;
    let have = file.metadata().map_err(|e| format!("{image}: {e}"))?.len();
    if have < size {
        file.set_len(size).map_err(|e| format!("{image}: {e}"))?;
    }
    drop(file);
    let mut disk = FileDisk::open(image, true)?;
    let total = disk.sectors();
    let (start, sectors) =
        if whole { (0, total) } else { (PART_START, total.saturating_sub(PART_START)) };
    if sectors < pbr_rockfs::MIN_BLOCKS * 8 {
        return Err("the volume is too small for RockFS (needs at least 256 KiB)".into());
    }
    if !whole {
        write_mbr(&mut disk, start, sectors)?;
    }
    let mut volume = disk.narrow(start, sectors);
    pbr_rockfs::format(&mut volume, &label, new_uuid(), now())
        .map_err(|e| describe(e).to_string())?;
    volume.sync();
    println!("Formatted {image} as RockFS ({})", human(size));
    Ok(0)
}

/// Writes a partition table with one RockFS partition.
fn write_mbr(disk: &mut FileDisk, start: u64, sectors: u64) -> Result<(), String> {
    let mut mbr = [0u8; SECTOR];
    let entry = &mut mbr[446..462];
    entry[1..4].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[4] = MBR_TYPE;
    entry[5..8].copy_from_slice(&[0xFF, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&u32::try_from(start).map_err(|_| "disk too big")?.to_le_bytes());
    entry[12..16].copy_from_slice(
        &u32::try_from(sectors.min(u64::from(u32::MAX))).unwrap_or(u32::MAX).to_le_bytes(),
    );
    mbr[440..444].copy_from_slice(&new_uuid()[..4]);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    disk.write(0, &mbr).map_err(|_| "the disk could not be written".to_string())
}

/// "32M", "256K", "1G" or a number of bytes.
fn parse_size(text: &str) -> Result<u64, String> {
    let upper = text.to_ascii_uppercase();
    let (digits, mult) = match upper.chars().last() {
        Some('K') => (&upper[..upper.len() - 1], 1u64 << 10),
        Some('M') => (&upper[..upper.len() - 1], 1 << 20),
        Some('G') => (&upper[..upper.len() - 1], 1 << 30),
        _ => (&upper[..], 1),
    };
    let n: u64 = digits.parse().map_err(|_| format!("{text}: not a size"))?;
    n.checked_mul(mult).ok_or_else(|| format!("{text}: too big"))
}

/// `info`
pub fn info(image: &str) -> Outcome {
    let fs = mount(image, false)?;
    let i = fs.info();
    let items = u64::from(i.inode_count) - 1 - u64::from(i.free_inodes.min(i.inode_count - 1));
    println!("Label:      {}", i.label);
    println!("ID:         {}", uuid_text(&i.uuid));
    println!("Size:       {} ({} blocks of 4 KiB)", human(i.block_count * 4096), i.block_count);
    println!(
        "Free:       {} ({} kept in reserve)",
        human(i.free_blocks * 4096),
        human(i.reserve_blocks * 4096)
    );
    println!(
        "Items:      {items} files and folders (room for {} more before the table grows)",
        i.free_inodes
    );
    println!("Created:    {} UTC", date_time(i.created));
    let last = if i.last_mounted == 0 {
        "never".to_string()
    } else {
        format!("{} UTC", date_time(i.last_mounted))
    };
    println!("Mounted:    {} times, last {last}", i.mount_count);
    println!("Shut down:  {}", if i.clean { "cleanly" } else { "NOT cleanly; run \"check\"" });
    Ok(0)
}

/// `ls [-R] [PATH]`
pub fn ls(image: &str, args: &[String]) -> Outcome {
    let recursive = args.iter().any(|a| a == "-R");
    let mut paths: Vec<&String> = args.iter().filter(|a| *a != "-R").collect();
    let root = String::from("/");
    if paths.is_empty() {
        paths.push(&root);
    }
    let mut fs = mount(image, false)?;
    for p in paths {
        let node = resolve(&mut fs, p)?;
        show(&mut fs, node, "", recursive).map_err(|e| explain(p, e))?;
    }
    Ok(0)
}

fn show(
    fs: &mut Volume,
    folder: u64,
    indent: &str,
    recursive: bool,
) -> Result<(), pbr_fs::FsError> {
    let mut list = entries(fs, folder)?;
    list.sort_by_key(|e| (e.kind != Kind::Folder, e.name.to_ascii_lowercase()));
    for e in list {
        let when = date_time(e.modified);
        if e.kind == Kind::Folder {
            println!("{indent}{:<40} {:>12}  {when}", format!("{}/", e.name), "");
            if recursive {
                let child = fs.lookup(folder, &e.name)?;
                show(fs, child, &format!("{indent}  "), true)?;
            }
        } else {
            println!("{indent}{:<40} {:>12}  {when}", e.name, human(e.size));
        }
    }
    Ok(())
}

/// `cat PATH`
pub fn cat(image: &str, args: &[String]) -> Outcome {
    let path = args.first().ok_or("cat needs a path")?;
    let mut fs = mount(image, false)?;
    let node = resolve(&mut fs, path)?;
    let data = read_file(&mut fs, node).map_err(|e| explain(path, e))?;
    std::io::stdout().write_all(&data).map_err(|e| e.to_string())?;
    Ok(0)
}

/// `get PATH [DEST]`
pub fn get(image: &str, args: &[String]) -> Outcome {
    let path = args.first().ok_or("get needs a path")?;
    let mut fs = mount(image, false)?;
    let node = resolve(&mut fs, path)?;
    let data = read_file(&mut fs, node).map_err(|e| explain(path, e))?;
    let dest = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| path.rsplit('/').find(|p| !p.is_empty()).unwrap_or("file").to_string());
    std::fs::write(&dest, &data).map_err(|e| format!("{dest}: {e}"))?;
    println!("Copied {path} to {dest} ({})", human(data.len() as u64));
    Ok(0)
}

/// `put SOURCE PATH`
pub fn put(image: &str, args: &[String]) -> Outcome {
    let (source, path) = match args {
        [s, p, ..] => (s, p),
        _ => return Err("put needs a source file and a path".into()),
    };
    let data = std::fs::read(source).map_err(|e| format!("{source}: {e}"))?;
    let mut fs = mount(image, true)?;
    let (folder, name) = resolve_parent(&mut fs, path)?;
    let node = match fs.lookup(folder, name) {
        Ok(n) => {
            if fs.stat(n).map_err(|e| explain(path, e))?.kind == Kind::Folder {
                return Err(format!("{path} is a folder"));
            }
            n
        }
        Err(pbr_fs::FsError::NotFound) => {
            fs.create(folder, name, Kind::File).map_err(|e| explain(path, e))?
        }
        Err(e) => return Err(explain(path, e)),
    };
    write_at(&mut fs, node, 0, &data).map_err(|e| explain(path, e))?;
    fs.set_size(node, data.len() as u64).map_err(|e| explain(path, e))?;
    fs.commit(node).map_err(|e| explain(path, e))?;
    close(fs)?;
    println!("Copied {source} to {path} ({})", human(data.len() as u64));
    Ok(0)
}

/// `mkdir PATH`
pub fn mkdir(image: &str, args: &[String]) -> Outcome {
    let path = args.first().ok_or("mkdir needs a path")?;
    let mut fs = mount(image, true)?;
    let (folder, name) = resolve_parent(&mut fs, path)?;
    fs.create(folder, name, Kind::Folder).map_err(|e| explain(path, e))?;
    close(fs)?;
    Ok(0)
}

/// `rm PATH`
pub fn rm(image: &str, args: &[String]) -> Outcome {
    let path = args.first().ok_or("rm needs a path")?;
    let mut fs = mount(image, true)?;
    let (folder, name) = resolve_parent(&mut fs, path)?;
    fs.remove(folder, name).map_err(|e| explain(path, e))?;
    close(fs)?;
    Ok(0)
}

/// `mv FROM TO`
pub fn mv(image: &str, args: &[String]) -> Outcome {
    let (from, to) = match args {
        [f, t, ..] => (f, t),
        _ => return Err("mv needs two paths".into()),
    };
    let mut fs = mount(image, true)?;
    let (from_folder, from_name) = resolve_parent(&mut fs, from)?;
    let (to_folder, to_name) = resolve_parent(&mut fs, to)?;
    fs.rename(from_folder, from_name, to_folder, to_name, false).map_err(|e| explain(to, e))?;
    close(fs)?;
    Ok(0)
}

/// `check [--repair]`: exit 0 if nothing is wrong (or all was repaired), 2 if a check found
/// problems it was not asked to repair or could not repair.
pub fn check(image: &str, args: &[String]) -> Outcome {
    let repair = args.iter().any(|a| a == "--repair");
    let mut fs = mount(image, repair)?;
    // A repair at mount time (the volume was not shut down cleanly) is part of the answer.
    let mut problems = fs.mount_report().to_vec();
    problems.extend(fs.check(repair).map_err(|e| describe(e).to_string())?);
    if problems.is_empty() {
        println!("No problems found.");
    }
    let mut unresolved = false;
    for p in &problems {
        let note = if p.harmless { " (harmless)" } else { "" };
        if p.fixed {
            println!("Fixed: {}{note}", p.text);
        } else {
            unresolved = true;
            println!("Problem: {}{note}", p.text);
        }
    }
    close(fs)?;
    Ok(if unresolved { 2 } else { 0 })
}
