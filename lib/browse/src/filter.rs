// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Which files to show: "Pictures (bmp png)", and always "All files".

use crate::item::Item;
use crate::path;
use alloc::string::String;
use alloc::vec::Vec;

/// How many filters an app can pass the picker (the protocol's `list<Filter, 8>`).
const MAX_FILTERS: usize = 8;

/// A choice in the picker's file-type list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Filter {
    /// What the user reads, such as "Pictures".
    pub title: String,
    /// The extensions that match: lower case, no dots. Empty means every file.
    pub extensions: Vec<String>,
}

impl Filter {
    /// A filter from a title and extensions written the way apps write them: `"bmp png"` or
    /// `"bmp, .PNG"`. The text is split on spaces and commas, dots are dropped, extensions
    /// made lower case, and empty ones and repeats left out.
    pub fn new(title: &str, extensions: &str) -> Filter {
        let mut list: Vec<String> = Vec::new();
        for word in extensions.split(|c: char| c == ',' || c.is_whitespace()) {
            let word = word.trim_matches('.');
            if word.is_empty() {
                continue;
            }
            let word = word.to_lowercase();
            if !list.contains(&word) {
                list.push(word);
            }
        }
        Filter { title: String::from(title), extensions: list }
    }

    /// "All files": no extensions, so it matches everything.
    pub fn all() -> Filter {
        Filter { title: String::from("All files"), extensions: Vec::new() }
    }

    /// Whether the item should be shown: folders always (so a person can open them to look
    /// further), and a file if its extension is one of the filter's, or the filter has none.
    pub fn matches(&self, item: &Item) -> bool {
        if item.folder || self.extensions.is_empty() {
            return true;
        }
        match path::extension(&item.name) {
            Some(ext) => {
                let ext = ext.to_lowercase();
                self.extensions.contains(&ext)
            }
            None => false,
        }
    }

    /// The first extension, which a save adds to a name that has none.
    pub fn first_extension(&self) -> Option<&str> {
        self.extensions.first().map(String::as_str)
    }
}

/// The filters an app asked for (the first 8 of them), then "All files" last, unless one of
/// them already matches everything.
pub fn with_all(filters: &[Filter]) -> Vec<Filter> {
    let mut out: Vec<Filter> = filters.iter().take(MAX_FILTERS).cloned().collect();
    if !out.iter().any(|f| f.extensions.is_empty()) {
        out.push(Filter::all());
    }
    out
}
