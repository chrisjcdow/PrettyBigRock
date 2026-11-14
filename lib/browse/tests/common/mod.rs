// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! What the tests share: short ways to make items.

#![allow(dead_code)]

use pbr_browse::Item;

/// A file.
pub fn file(name: &str, size: u64, modified: u64) -> Item {
    Item { name: name.to_string(), folder: false, size, modified, hidden: false }
}

/// A folder.
pub fn folder(name: &str) -> Item {
    Item { name: name.to_string(), folder: true, size: 0, modified: 0, hidden: false }
}

/// The names of the items, in order.
pub fn names(items: &[Item]) -> Vec<&str> {
    items.iter().map(|i| i.name.as_str()).collect()
}

/// A small, repeatable random number generator (xorshift64).
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}
