// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! [`Folders`] for a program's own file tree (`lib/files`, note 0014, section 8): what the
//! picker and Files browse on PrettyBigRock. Built only for PrettyBigRock itself; the host
//! tests use [`crate::MemTree`].

use crate::item::Item;
use crate::path;
use crate::tree::{BrowseError, Folders};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_files::{Error, Files, NodeKind};
use pbr_interfaces::pbr_file::{FileError, FileRights};

/// How `lib/files`' errors are told to someone browsing.
fn error(e: Error) -> BrowseError {
    match e {
        Error::NotFound | Error::File(FileError::NotFound | FileError::Gone) => {
            BrowseError::NotFound
        }
        Error::BadPath | Error::File(FileError::BadName | FileError::BadPath) => {
            BrowseError::BadName
        }
        Error::File(FileError::NotAFolder) => BrowseError::NotAFolder,
        Error::File(FileError::AlreadyExists) => BrowseError::AlreadyExists,
        // A folder above the program's own entries is shown, but nothing can be made there.
        Error::Above | Error::File(FileError::NoRight | FileError::ReadOnly) => {
            BrowseError::NoRight
        }
        e => BrowseError::Other(pbr_files::describe(e)),
    }
}

impl Folders for Files {
    fn list(&mut self, path: &str) -> Result<Vec<Item>, BrowseError> {
        let found = Files::list(self, path).map_err(error)?;
        Ok(found
            .into_iter()
            .map(|l| Item {
                name: l.name,
                folder: l.folder,
                size: l.size,
                modified: l.modified,
                hidden: l.hidden,
            })
            .collect())
    }

    fn stat(&mut self, path: &str) -> Result<Item, BrowseError> {
        let info = self.info(path).map_err(error)?;
        Ok(Item {
            name: String::from(path::name(path)),
            folder: info.kind == NodeKind::Folder,
            size: info.size,
            modified: info.modified,
            hidden: info.hidden,
        })
    }

    fn make_folder(&mut self, path: &str) -> Result<(), BrowseError> {
        path::check_name(path::name(path))?;
        let rights = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
        self.create(path, true, rights).map(|_| ()).map_err(|e| match error(e) {
            BrowseError::Other(text) => BrowseError::Other(format!("making {path}: {text}")),
            other => other,
        })
    }
}
