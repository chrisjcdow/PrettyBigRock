// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Thousands of hostile names through everything: nothing may panic.

mod common;

use common::Rng;
use pbr_browse::filter::with_all;
use pbr_browse::format::{date, size};
use pbr_browse::item::{sort, visible};
use pbr_browse::kind::kind_of;
use pbr_browse::path::{check_name, crumbs, extension, inside, join, name, parent, tidy};
use pbr_browse::places::{place_of, places};
use pbr_browse::{Filter, Folders, Item, MemTree, Sort, SortBy};

/// A string made from random bytes: sometimes whole bytes turned into characters (so control
/// characters, DEL and Latin-1 appear), sometimes valid UTF-8 from the bytes (lossy), with
/// dots, slashes, spaces and long digit runs thrown in.
fn hostile(rng: &mut Rng) -> String {
    let pieces = rng.next() % 8;
    let mut out = String::new();
    for _ in 0..pieces {
        match rng.next() % 9 {
            0 => out.push_str(&"7".repeat((rng.next() % 60) as usize)),
            1 => out.push_str(&"0".repeat((rng.next() % 30) as usize)),
            2 => out.push_str([".", "..", "/", " ", "./", "/.."][(rng.next() % 6) as usize]),
            3 => out.push(char::from_u32((rng.next() % 0x2000) as u32).unwrap_or('?')),
            4 => out.push_str("\u{130}\u{df}\u{3a3}\u{202e}"),
            5 => {
                let bytes: Vec<u8> = (0..rng.next() % 6).map(|_| rng.next() as u8).collect();
                out.push_str(&String::from_utf8_lossy(&bytes));
            }
            6 => out.push_str([".txt", ".PNG", ".zip", ".", ".gz"][(rng.next() % 5) as usize]),
            _ => out.extend((0..rng.next() % 5).map(|_| (rng.next() as u8) as char)),
        }
    }
    out
}

#[test]
fn nothing_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut tree = MemTree::new();
    let all_places = places(true, &["Data"]);
    let mut items: Vec<Item> = Vec::new();
    for round in 0..4000 {
        let a = hostile(&mut rng);
        let b = hostile(&mut rng);
        let _ = kind_of(&a, rng.next().is_multiple_of(2));
        let _ = check_name(&a);
        let _ = extension(&a);
        let _ = parent(&a);
        let _ = name(&a);
        let _ = crumbs(&a);
        let _ = tidy(&a);
        let _ = inside(&a, &b);
        let _ = join(&a, &b);
        let _ = place_of(&a, &all_places);
        let filter = Filter::new(&a, &b);
        let probe = Item {
            name: a.clone(),
            folder: false,
            size: rng.next(),
            modified: rng.next(),
            hidden: false,
        };
        let _ = filter.matches(&probe);
        let _ = filter.first_extension();
        let _ = with_all(&[filter, Filter::new(&b, &a)]);
        let _ = places(true, &[&a, &b]);
        let _ = size(rng.next() >> (rng.next() % 64));
        let _ = date(rng.next() >> (rng.next() % 64), rng.next() as i32);

        tree.add_folder(&a);
        tree.add_file(&b, rng.next(), rng.next());
        tree.hide(&a);
        tree.read_only(&b);
        let _ = tree.stat(&a);
        let _ = tree.list(&b);
        let _ = tree.make_folder(&a);
        let _ = tree.make_folder(&join("/", &a));
        tree.add_file(&join("/d", &b), 1, 1);
        let _ = tree.list("/d");
        items.push(probe);
        if round % 500 == 499 {
            for by in [SortBy::Name, SortBy::Size, SortBy::Modified, SortBy::Kind] {
                for ascending in [true, false] {
                    let mut copy = items.clone();
                    sort(&mut copy, Sort { by, ascending });
                    let _ = visible(copy, round % 1000 == 499);
                }
            }
        }
    }
    assert!(tree.exists("/"));
}

#[test]
fn sorting_is_a_proper_order_on_hostile_names() {
    // A sort that is not a true order can panic in Rust's sort; many names with equal letters,
    // zeros and digits, in every direction, make sure it is one.
    let mut rng = Rng(0x1234_5678_9ABC_DEF1);
    let mut items: Vec<Item> = (0..3000)
        .map(|_| Item {
            name: hostile(&mut rng),
            folder: rng.next().is_multiple_of(5),
            size: rng.next() % 4,
            modified: rng.next() % 4,
            hidden: false,
        })
        .collect();
    for by in [SortBy::Name, SortBy::Size, SortBy::Modified, SortBy::Kind] {
        for ascending in [true, false] {
            sort(&mut items, Sort { by, ascending });
            // Sorting again changes nothing.
            let once = items.clone();
            sort(&mut items, Sort { by, ascending });
            assert_eq!(once, items);
        }
    }
}
