// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Walking and reading a mounted volume by path: what every command needs.

use crate::util::describe;
use pbr_fs::{Entry, Filesystem, FsError, Kind, NodeId};

/// The most one read or write request may carry (note 0014).
const CHUNK: usize = 65536;

/// Turns a failed call on `path` into a message: "path: reason".
pub fn explain(path: &str, e: FsError) -> String {
    format!("{path}: {}", describe(e))
}

/// The parts of a path, without empty parts (so "//a/" is just "a") or ".".
pub fn parts(path: &str) -> Result<Vec<&str>, String> {
    let list: Vec<&str> = path.split('/').filter(|p| !p.is_empty() && *p != ".").collect();
    if list.contains(&"..") {
        return Err(format!("{path}: \"..\" is not supported"));
    }
    Ok(list)
}

/// Finds the file or folder at `path`.
pub fn resolve<F: Filesystem>(fs: &mut F, path: &str) -> Result<NodeId, String> {
    let mut node = fs.root();
    for part in parts(path)? {
        node = fs.lookup(node, part).map_err(|e| explain(path, e))?;
    }
    Ok(node)
}

/// Finds the folder that holds `path`'s last part, and that last part.
pub fn resolve_parent<'a, F: Filesystem>(
    fs: &mut F,
    path: &'a str,
) -> Result<(NodeId, &'a str), String> {
    let list = parts(path)?;
    let Some((last, folders)) = list.split_last() else {
        return Err("that is the top-level folder".into());
    };
    let mut node = fs.root();
    for part in folders {
        node = fs.lookup(node, part).map_err(|e| explain(path, e))?;
    }
    Ok((node, last))
}

/// Everything in `folder`, in the order the volume lists it.
pub fn entries<F: Filesystem>(fs: &mut F, folder: NodeId) -> Result<Vec<Entry>, FsError> {
    let mut all = Vec::new();
    let mut cursor = 0;
    loop {
        cursor = fs.list(folder, cursor, &mut |e| {
            all.push(e);
            true
        })?;
        if cursor == 0 {
            return Ok(all);
        }
    }
}

/// The whole contents of a file.
pub fn read_file<F: Filesystem>(fs: &mut F, node: NodeId) -> Result<Vec<u8>, FsError> {
    let size = usize::try_from(fs.stat(node)?.size).map_err(|_| FsError::TooBig)?;
    let mut out = vec![0u8; size];
    let mut at = 0;
    while at < size {
        let end = (at + CHUNK).min(size);
        let got = fs.read(node, at as u64, &mut out[at..end])?;
        if got == 0 {
            return Err(FsError::Damaged);
        }
        at += got;
    }
    Ok(out)
}

/// Writes `data` at `offset`, in the pieces a request may carry.
pub fn write_at<F: Filesystem>(
    fs: &mut F,
    node: NodeId,
    offset: u64,
    data: &[u8],
) -> Result<(), FsError> {
    for (i, piece) in data.chunks(CHUNK).enumerate() {
        let n = fs.write(node, offset + (i * CHUNK) as u64, piece)?;
        if n != piece.len() {
            return Err(FsError::Disk);
        }
    }
    Ok(())
}

/// Every file and folder below `folder`, depth first: `(path, kind, size)`, with `base` as
/// the path of `folder` itself ("" for the top).
pub fn walk<F: Filesystem>(
    fs: &mut F,
    folder: NodeId,
    base: &str,
    out: &mut Vec<(String, Kind, u64)>,
) -> Result<(), String> {
    let mut list =
        entries(fs, folder).map_err(|e| explain(if base.is_empty() { "/" } else { base }, e))?;
    list.sort_by_key(|e| e.name.to_ascii_lowercase());
    for e in list {
        let path = format!("{base}/{}", e.name);
        out.push((path.clone(), e.kind, e.size));
        if e.kind == Kind::Folder {
            let child = fs.lookup(folder, &e.name).map_err(|err| explain(&path, err))?;
            walk(fs, child, &path, out)?;
        }
    }
    Ok(())
}
