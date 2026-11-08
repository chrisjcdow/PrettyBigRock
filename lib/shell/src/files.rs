// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The shell's file commands: `ls`, `cd`, `cat`, `info`, `mkdir`, `write`, `append`, `cp`, `mv`,
//! `rm` and `df`.
//!
//! [`FileShell`] holds the current folder, the shape of the tree ([`Layout`]) and something that
//! does the real file work ([`Fs`]). For each command it:
//! 1. makes the typed path absolute (`..` and `.` are resolved by text, so a path can never
//!    climb out of what the shell holds);
//! 2. asks the layout what is there: the folders above the attached volumes exist only in the
//!    shell and are answered here, and are never changed; the top of each attached volume
//!    (`/Home`, `/Volumes/Data`) can't be removed, moved or replaced; everything inside is up to
//!    the filesystem;
//! 3. does the work, in pieces small enough for a small program: files are copied 64 KB at a
//!    time, and folders are followed to a fixed depth, so a hostile or damaged tree can't make
//!    the shell run out of stack or memory.
//!
//! A command that works gives back the text to print (every line ending in `\n`) or nothing; one
//! that fails gives back a [`Failure`], which prints as `<path>: <reason>`. Where the failing
//! thing is exactly what the user typed, the path is shown as typed.
//!
//! The ideas used in the rest of this file: `&self` is a method that only looks at its object,
//! `&mut self` one that may change it; `?` ends the function early with the error if a `Result`
//! is an `Err`; `map_err` changes the error to another type on the way.

use crate::format;
use crate::fs::{Fs, Item, Stat};
use crate::layout::{self, HOME, Layout, Place};
use crate::reason::{Failure, Reason};
use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// How much of a file `cat` shows, and the most it will read.
pub const MAX_CAT: u64 = 64 * 1024;
/// How much of a file is read and written at a time when copying.
const CHUNK: u32 = 64 * 1024;
/// How many folders deep `cp -r`, `rm -r` and moves between volumes follow.
const MAX_DEPTH: usize = 64;

/// The shell's view of its files: where it is, and what it can do there.
pub struct FileShell<F: Fs> {
    fs: F,
    layout: Layout,
    cwd: String,
}

impl<F: Fs> FileShell<F> {
    /// A shell over `fs`, whose tree has these entries (path, whether a folder), starting in
    /// `/Home` (or `/` if there is no home).
    pub fn new(fs: F, entries: &[(String, bool)]) -> FileShell<F> {
        let layout = Layout::new(entries);
        let cwd = if matches!(layout.place(HOME), Place::Nowhere) { "/" } else { HOME };
        FileShell { fs, layout, cwd: String::from(cwd) }
    }

    /// The thing doing the real file work.
    pub fn fs(&self) -> &F {
        &self.fs
    }

    /// The current folder: absolute and tidy.
    pub fn cwd(&self) -> &str {
        &self.cwd
    }

    /// `typed` made absolute, as a failure labelled with `typed` if it can't be.
    pub fn absolute(&self, typed: &str) -> Result<String, Failure> {
        layout::resolve(&self.cwd, typed).map_err(|r| Failure::new(typed, r))
    }

    /// `ls [-a] [PATH]`: the names in a folder (or the one line for a file).
    pub fn ls(&self, all: bool, path: Option<&str>) -> Result<String, Failure> {
        let typed = path.unwrap_or(".");
        let abs = self.absolute(typed)?;
        let shown = || -> Result<String, Failure> {
            let st = self.stat_at(&abs).map_err(|r| Failure::new(&abs, r))?;
            if !st.folder {
                let name = layout::base_name(&abs).unwrap_or(&abs);
                return Ok(format!("{name}  {}\n", format::size(st.size)));
            }
            let items = self.list_at(&abs).map_err(|r| Failure::new(&abs, r))?;
            Ok(format::listing(&items, all))
        };
        shown().map_err(|f| relabel(f, &abs, typed))
    }

    /// `cd [PATH]`: `/Home` if there is no path. Fails, leaving the folder as it was, on files
    /// and on paths that aren't there.
    pub fn cd(&mut self, path: Option<&str>) -> Result<(), Failure> {
        let typed = path.unwrap_or(HOME);
        let abs = self.absolute(typed)?;
        let st = self.stat_at(&abs).map_err(|r| Failure::new(typed, r))?;
        if !st.folder {
            return Err(Failure::new(typed, Reason::NotAFolder));
        }
        self.cwd = abs;
        Ok(())
    }

    /// `cat FILE`: its text. Bytes that aren't UTF-8 are replaced, `\r` is dropped (the
    /// console would draw it as `?`), and a final `\n` is added if the file doesn't end with one.
    pub fn cat(&self, path: &str) -> Result<String, Failure> {
        let abs = self.absolute(path)?;
        let read = || -> Result<Vec<u8>, Reason> {
            let st = self.stat_at(&abs)?;
            if st.folder {
                return Err(Reason::NotAFile);
            }
            if st.size > MAX_CAT {
                return Err(Reason::TooBigToShow);
            }
            self.fs.read_at(&abs, 0, MAX_CAT as u32)
        };
        let bytes = read().map_err(|r| Failure::new(path, r))?;
        let mut text = String::from_utf8_lossy(&bytes).replace('\r', "");
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(text)
    }

    /// `info PATH`: what is known about a file or folder.
    pub fn info(&self, path: &str) -> Result<String, Failure> {
        let abs = self.absolute(path)?;
        let st = self.stat_at(&abs).map_err(|r| Failure::new(path, r))?;
        Ok(format::info(&abs, &st))
    }

    /// `mkdir PATH`.
    pub fn mkdir(&self, path: &str) -> Result<(), Failure> {
        let abs = self.absolute(path)?;
        self.make_folder(&abs).map_err(|r| Failure::new(path, r))
    }

    /// `write FILE TEXT…`: the file becomes `text` and a line end. Makes the file if needed.
    pub fn write(&self, path: &str, text: &str) -> Result<(), Failure> {
        let abs = self.absolute(path)?;
        let go = || -> Result<(), Reason> {
            self.check_spot(&abs, true)?;
            self.fs.replace(&abs, format!("{text}\n").as_bytes())
        };
        go().map_err(|r| Failure::new(path, r))
    }

    /// `append FILE TEXT…`: adds `text` and a line end at the end of the file. Makes the file
    /// if needed.
    pub fn append(&self, path: &str, text: &str) -> Result<(), Failure> {
        let abs = self.absolute(path)?;
        let go = || -> Result<(), Reason> {
            self.check_spot(&abs, true)?;
            self.fs.append(&abs, format!("{text}\n").as_bytes())
        };
        go().map_err(|r| Failure::new(path, r))
    }

    /// `rm [-r] PATH`: a file or an empty folder, or with `recursive` a folder and everything
    /// in it. The top of an attached volume, and the folders above them, are refused.
    pub fn rm(&self, recursive: bool, path: &str) -> Result<(), Failure> {
        let abs = self.absolute(path)?;
        let go = || -> Result<(), Failure> {
            self.check_removable(&abs).map_err(|r| Failure::new(&abs, r))?;
            let st = self.fs.stat(&abs).map_err(|r| Failure::new(&abs, r))?;
            if st.folder && recursive {
                self.remove_tree(&abs, 0)
            } else {
                self.fs.remove(&abs).map_err(|r| Failure::new(&abs, r))
            }
        };
        go().map_err(|f| relabel(f, &abs, path))
    }

    /// `cp [-r] FROM TO`: copies a file (or with `recursive` a folder and everything in it).
    /// If TO is a folder the copy goes inside it under the same name. A file that is there
    /// already is replaced. Works between volumes.
    pub fn cp(&self, recursive: bool, from: &str, to: &str) -> Result<(), Failure> {
        let (from_abs, to_abs) = (self.absolute(from)?, self.absolute(to)?);
        let go = || -> Result<(), Failure> {
            let src = self.stat_at(&from_abs).map_err(|r| Failure::new(&from_abs, r))?;
            if src.folder && !recursive {
                return Err(Failure::new(&from_abs, Reason::NeedsRecursive));
            }
            let dest = self.destination(&from_abs, &to_abs)?;
            self.check_not_same_or_inside(&from_abs, &dest, src.folder)?;
            self.copy_item(&from_abs, &dest, src.folder, 0)
        };
        go().map_err(|f| relabel(relabel(f, &from_abs, from), &to_abs, to))
    }

    /// `mv FROM TO`: renames or moves, into TO if it is a folder. Never replaces something
    /// that is there. Within one attached volume this is the filesystem's own rename; between
    /// volumes it copies, then removes the original.
    pub fn mv(&self, from: &str, to: &str) -> Result<(), Failure> {
        let (from_abs, to_abs) = (self.absolute(from)?, self.absolute(to)?);
        let go = || -> Result<(), Failure> {
            self.check_removable(&from_abs).map_err(|r| Failure::new(&from_abs, r))?;
            let src = self.fs.stat(&from_abs).map_err(|r| Failure::new(&from_abs, r))?;
            let dest = self.destination(&from_abs, &to_abs)?;
            self.check_not_same_or_inside(&from_abs, &dest, src.folder)?;
            // Where it would land has to be a real place with nothing in it already.
            match self.layout.place(&dest) {
                Place::Inside { .. } => {}
                Place::Mount { .. } | Place::Virtual(_) => {
                    return Err(Failure::new(&dest, Reason::AlreadyExists));
                }
                Place::Nowhere => {
                    return Err(Failure::new(&dest, self.missing_spot_reason(&dest)));
                }
            }
            if self.fs.stat(&dest).is_ok() {
                return Err(Failure::new(&dest, Reason::AlreadyExists));
            }
            // The folder it would go in must be there: otherwise a rename's "not found" would
            // read as if the thing being moved was missing.
            match self.stat_at(layout::parent(&dest)) {
                Ok(st) if st.folder => {}
                Ok(_) => return Err(Failure::new(&dest, Reason::NotAFolder)),
                Err(reason) => return Err(Failure::new(&dest, reason)),
            }
            if self.layout.same_entry(&from_abs, &dest) {
                return self.fs.rename(&from_abs, &dest).map_err(|r| Failure::new(&from_abs, r));
            }
            // Between volumes there is no rename: copy it all, and only if that worked, remove
            // the original.
            self.copy_item(&from_abs, &dest, src.folder, 0)?;
            let removed = if src.folder {
                self.remove_tree(&from_abs, 0)
            } else {
                self.fs.remove(&from_abs).map_err(|r| Failure::new(&from_abs, r))
            };
            removed.map_err(|f| Failure::new(f.path, Reason::LeftBehind(Box::new(f.reason))))
        };
        go().map_err(|f| relabel(relabel(f, &from_abs, from), &to_abs, to))
    }

    /// `df`: the size and free space of the home folder and each volume, and a failure for
    /// each one that could not be asked.
    pub fn df(&self) -> (String, Vec<Failure>) {
        let mut text = String::new();
        let mut failures = Vec::new();
        for root in self.layout.volume_roots() {
            match self.fs.space(&root) {
                Ok(space) => text.push_str(&format::space_line(&root, &space)),
                Err(reason) => failures.push(Failure::new(root, reason)),
            }
        }
        (text, failures)
    }

    // ---- What is at a path ----

    /// Facts about `abs`, answered here for the folders that only hold other folders.
    fn stat_at(&self, abs: &str) -> Result<Stat, Reason> {
        match self.layout.place(abs) {
            Place::Virtual(_) => Ok(Stat::virtual_folder()),
            Place::Nowhere => Err(Reason::NotFound),
            Place::Mount { .. } | Place::Inside { .. } => self.fs.stat(abs),
        }
    }

    /// What is in the folder `abs`.
    fn list_at(&self, abs: &str) -> Result<Vec<Item>, Reason> {
        match self.layout.place(abs) {
            // The layout knows the names but not the sizes of attached files (`rules.txt`):
            // the filesystem does.
            Place::Virtual(mut items) => {
                for item in items.iter_mut().filter(|i| !i.folder) {
                    if let Ok(st) = self.fs.stat(&layout::join(abs, &item.name)) {
                        item.size = st.size;
                    }
                }
                Ok(items)
            }
            Place::Nowhere => Err(Reason::NotFound),
            Place::Mount { .. } | Place::Inside { .. } => self.fs.list(abs),
        }
    }

    /// Why nothing can be made at `abs`, which is in no attached volume: it is in a folder that
    /// only holds other folders, or the folder it would go in isn't there at all.
    fn missing_spot_reason(&self, abs: &str) -> Reason {
        match self.layout.place(layout::parent(abs)) {
            Place::Virtual(_) => Reason::UnderVirtual,
            _ => Reason::NotFound,
        }
    }

    /// Whether something may be made or written at `abs` as far as the shape of the tree can
    /// tell: `Ok` means the filesystem should be asked. `file` says what is wanted: a file to
    /// write, or (not `file`) a new folder.
    fn check_spot(&self, abs: &str, file: bool) -> Result<(), Reason> {
        match self.layout.place(abs) {
            Place::Inside { .. } => Ok(()),
            // The top of an attached file can be written (if its rights allow); anything else
            // that is already there can't be made again, and a folder can't be written.
            Place::Mount { folder, .. } if file && !folder => Ok(()),
            Place::Mount { .. } | Place::Virtual(_) if file => Err(Reason::NotAFile),
            Place::Mount { .. } | Place::Virtual(_) => Err(Reason::AlreadyExists),
            Place::Nowhere => Err(self.missing_spot_reason(abs)),
        }
    }

    /// Makes the folder `abs`.
    fn make_folder(&self, abs: &str) -> Result<(), Reason> {
        self.check_spot(abs, false)?;
        self.fs.mkdir(abs)
    }

    /// Whether `abs` is something that may be removed or moved away: not a folder above the
    /// volumes, not the top of one, and there.
    fn check_removable(&self, abs: &str) -> Result<(), Reason> {
        match self.layout.place(abs) {
            Place::Inside { .. } => Ok(()),
            Place::Mount { .. } => Err(Reason::MountPoint),
            Place::Virtual(_) => Err(Reason::Virtual),
            Place::Nowhere => Err(Reason::NotFound),
        }
    }

    // ---- Copying and removing ----

    /// Where a copy or move of `from` to `to` lands: inside `to` if that is a folder, under
    /// `from`'s own name; otherwise at `to`.
    fn destination(&self, from: &str, to: &str) -> Result<String, Failure> {
        match self.stat_at(to) {
            Ok(st) if st.folder => match layout::base_name(from) {
                Some(name) => Ok(layout::join(to, name)),
                None => Err(Failure::new(from, Reason::BadArgument)),
            },
            _ => Ok(String::from(to)),
        }
    }

    /// Refuses a copy or move onto itself, and of a folder into itself (which would never end).
    fn check_not_same_or_inside(
        &self,
        from: &str,
        dest: &str,
        folder: bool,
    ) -> Result<(), Failure> {
        if from == dest {
            return Err(Failure::new(from, Reason::SameFile));
        }
        if folder && layout::is_within(from, dest) {
            return Err(Failure::new(from, Reason::IntoItself));
        }
        Ok(())
    }

    /// Copies the file or folder `from` to `to`. A folder is made at `to` if it isn't there (if
    /// it is, its contents are merged in) and each thing in it is copied in turn.
    fn copy_item(&self, from: &str, to: &str, folder: bool, depth: usize) -> Result<(), Failure> {
        if !folder {
            return self.copy_file(from, to);
        }
        if depth >= MAX_DEPTH {
            return Err(Failure::new(from, Reason::TooDeep));
        }
        match self.stat_at(to) {
            Ok(st) if st.folder => {}
            Ok(_) => return Err(Failure::new(to, Reason::NotAFolder)),
            Err(Reason::NotFound) => self.make_folder(to).map_err(|r| Failure::new(to, r))?,
            Err(r) => return Err(Failure::new(to, r)),
        }
        for item in self.list_at(from).map_err(|r| Failure::new(from, r))? {
            let (child_from, child_to) =
                (layout::join(from, &item.name), layout::join(to, &item.name));
            self.copy_item(&child_from, &child_to, item.folder, depth + 1)?;
        }
        Ok(())
    }

    /// Copies one file, a chunk at a time (so a big file doesn't need to fit in memory).
    fn copy_file(&self, from: &str, to: &str) -> Result<(), Failure> {
        self.check_spot(to, true).map_err(|r| Failure::new(to, r))?;
        let mut offset = 0u64;
        loop {
            let data = self.fs.read_at(from, offset, CHUNK).map_err(|r| Failure::new(from, r))?;
            // The first chunk makes (or empties) the file, even if there is nothing to write.
            let written = if offset == 0 {
                self.fs.replace(to, &data)
            } else if data.is_empty() {
                Ok(())
            } else {
                self.fs.append(to, &data)
            };
            written.map_err(|r| Failure::new(to, r))?;
            if data.len() < CHUNK as usize {
                return Ok(());
            }
            offset += data.len() as u64;
        }
    }

    /// Removes the folder `abs` and everything in it, the inside first.
    fn remove_tree(&self, abs: &str, depth: usize) -> Result<(), Failure> {
        if depth >= MAX_DEPTH {
            return Err(Failure::new(abs, Reason::TooDeep));
        }
        for item in self.fs.list(abs).map_err(|r| Failure::new(abs, r))? {
            let child = layout::join(abs, &item.name);
            if item.folder {
                self.remove_tree(&child, depth + 1)?;
            } else {
                self.fs.remove(&child).map_err(|r| Failure::new(&child, r))?;
            }
        }
        self.fs.remove(abs).map_err(|r| Failure::new(abs, r))
    }
}

/// If the failure is about exactly the absolute path `abs`, says `typed` instead: what the
/// user typed is what they recognise.
fn relabel(mut failure: Failure, abs: &str, typed: &str) -> Failure {
    if failure.path == abs {
        failure.path = String::from(typed);
    }
    failure
}

#[cfg(test)]
mod tests;
