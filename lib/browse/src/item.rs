// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! One entry of a folder, and putting entries in order.

use crate::kind::kind_of;
use alloc::string::String;
use alloc::vec::Vec;
use core::cmp::Ordering;

/// One thing in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    /// The name, without the folder's path.
    pub name: String,
    /// Whether it is a folder (otherwise a file).
    pub folder: bool,
    /// The size in bytes (0 for a folder).
    pub size: u64,
    /// When it was last changed, in seconds since 1970 (0 if unknown).
    pub modified: u64,
    /// Whether it is hidden (shown only when the user asks).
    pub hidden: bool,
}

/// What to put in order by.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    /// By name, naturally ("file2" before "file10").
    Name,
    /// By size (folders by name).
    Size,
    /// By when it was changed (folders by name).
    Modified,
    /// By the title of its kind of file, then name.
    Kind,
}

/// A way to put items in order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    /// What to order by.
    pub by: SortBy,
    /// Smallest or earliest first (otherwise the other way round).
    pub ascending: bool,
}

impl Default for Sort {
    /// By name, from the start of the alphabet.
    fn default() -> Sort {
        Sort { by: SortBy::Name, ascending: true }
    }
}

/// Compares two runs of ASCII digits by their number, however long: leading zeros are skipped,
/// then the longer run is the bigger number, then they are compared digit by digit. (They are
/// not turned into a number, which could be too big for any integer type.)
fn compare_numbers(a: &str, b: &str) -> Ordering {
    let a = a.trim_start_matches('0');
    let b = b.trim_start_matches('0');
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

/// Compares two names the way a person would: without regard to case (`É` and `é` are the
/// same letter), and each run of digits as a number, so `file2` comes before `file10`.
/// When names differ only in how many leading zeros their numbers have (`a1`, `a01`), the one
/// with the shorter run comes first, but only if nothing later in the names differs.
fn natural(a: &str, b: &str) -> Ordering {
    let (mut rest_a, mut rest_b) = (a, b);
    // The first difference in the length of equal-valued number runs, kept until the end.
    let mut zeros = Ordering::Equal;
    loop {
        // `chars().next()` is the next character, if any (a `char` is one Unicode character).
        match (rest_a.chars().next(), rest_b.chars().next()) {
            (None, None) => return zeros,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(ca), Some(cb)) if ca.is_ascii_digit() && cb.is_ascii_digit() => {
                let end_a = rest_a.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest_a.len());
                let end_b = rest_b.find(|c: char| !c.is_ascii_digit()).unwrap_or(rest_b.len());
                let (run_a, run_b) = (&rest_a[..end_a], &rest_b[..end_b]);
                let order = compare_numbers(run_a, run_b);
                if order != Ordering::Equal {
                    return order;
                }
                if zeros == Ordering::Equal {
                    zeros = run_a.len().cmp(&run_b.len());
                }
                rest_a = &rest_a[end_a..];
                rest_b = &rest_b[end_b..];
            }
            (Some(ca), Some(cb)) => {
                // A letter can lower-case to several (`İ`), so compare the whole lower-case
                // text of each, not just its first character.
                let order = ca.to_lowercase().cmp(cb.to_lowercase());
                if order != Ordering::Equal {
                    return order;
                }
                rest_a = &rest_a[ca.len_utf8()..];
                rest_b = &rest_b[cb.len_utf8()..];
            }
        }
    }
}

/// The order of two names: natural, then (so no two different names are ever "equal", and the
/// result does not depend on where the items started) the exact bytes.
fn by_name(a: &Item, b: &Item) -> Ordering {
    natural(&a.name, &b.name).then_with(|| a.name.as_bytes().cmp(b.name.as_bytes()))
}

/// The whole order: folders first, then the chosen column, then name.
fn compare(a: &Item, b: &Item, how: Sort) -> Ordering {
    match (a.folder, b.folder) {
        (true, false) => return Ordering::Less,
        (false, true) => return Ordering::Greater,
        _ => {}
    }
    let column = if a.folder {
        // Folders have no size to speak of, and one title: they go by name.
        Ordering::Equal
    } else {
        match how.by {
            SortBy::Name => Ordering::Equal,
            SortBy::Size => a.size.cmp(&b.size),
            SortBy::Modified => a.modified.cmp(&b.modified),
            SortBy::Kind => kind_of(&a.name, false).title().cmp(kind_of(&b.name, false).title()),
        }
    };
    let order = column.then_with(|| by_name(a, b));
    if how.ascending { order } else { order.reverse() }
}

/// Puts `items` in order: folders always first (whichever the direction), then by the chosen
/// column, then by name, so the order is the same on every run.
pub fn sort(items: &mut [Item], sort: Sort) {
    // `sort_by` keeps equal items in their old order (it is "stable"), though with the name
    // as a last tie-break nothing compares equal unless the two items are the same.
    items.sort_by(|a, b| compare(a, b, sort));
}

/// The items to show: all of them, or without the hidden ones.
pub fn visible(items: Vec<Item>, show_hidden: bool) -> Vec<Item> {
    items.into_iter().filter(|item| show_hidden || !item.hidden).collect()
}
