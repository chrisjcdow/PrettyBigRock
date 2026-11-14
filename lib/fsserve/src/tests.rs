// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The server's rules, tested on the host with the filesystem in memory: every operation,
//! every right, the path rules, records and their going, saving when the last writer goes, the
//! record limit, and a hostile client sending random requests.

extern crate std;

use crate::core::{ALL, Core, Error, READ, WRITE, split_path};
use crate::memfs::MemFs;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_fs::Kind;

fn new() -> (Core<MemFs>, u64) {
    let mut c = Core::new(MemFs::new("Test"));
    let root = c.root().unwrap();
    (c, root)
}

#[test]
fn files_and_folders() {
    let (mut c, root) = new();
    let (docs, info) = c.create(root, "Docs", Kind::Folder, ALL).unwrap();
    assert_eq!((info.stat.kind, info.rights), (Kind::Folder, ALL));
    let (f, _) = c.create(docs, "letter.txt", Kind::File, ALL).unwrap();
    assert_eq!(c.write(f, 0, b"Dear Hema").unwrap(), 9);
    assert_eq!(c.write(f, 20, b"!").unwrap(), 1); // a gap reads as zeros
    assert_eq!(c.read(f, 0, 100).unwrap(), b"Dear Hema\0\0\0\0\0\0\0\0\0\0\0!");
    assert_eq!(c.read(f, 21, 100).unwrap(), b"");
    c.set_size(f, 4).unwrap();
    assert_eq!(c.info(f).unwrap().stat.size, 4);
    // Names compare with A-Z matching a-z; paths go through folders.
    let (g, _) = c.open(root, "docs/LETTER.TXT", READ).unwrap();
    assert_eq!(c.read(g, 0, 10).unwrap(), b"Dear");
    assert_eq!(c.create(root, "DOCS", Kind::Folder, ALL), Err(Error::AlreadyExists));
    // The wrong kind of node.
    assert_eq!(c.read(docs, 0, 1), Err(Error::NotAFile));
    assert_eq!(c.list(f, 0, 10), Err(Error::NotAFolder));
    assert_eq!(c.open(f, "x", READ).map(|_| ()), Err(Error::NotAFolder));
    assert_eq!(c.open(root, "Docs/letter.txt/x", READ).map(|_| ()), Err(Error::NotAFolder));
    assert_eq!(c.create(root, "Nope/new", Kind::File, ALL).map(|_| ()), Err(Error::NotFound));
}

#[test]
fn listing_in_pages() {
    let (mut c, root) = new();
    for i in 0..130 {
        c.create(root, &format!("file {i:03}"), Kind::File, READ).map(|(b, _)| c.gone(b)).unwrap();
    }
    let (mut cursor, mut names) = (0, Vec::new());
    loop {
        let (page, next) = c.list(root, cursor, 1000).unwrap();
        assert!(page.len() <= 56, "a page bigger than a reply can carry");
        names.extend(page.into_iter().map(|e| e.name));
        if next == 0 {
            break;
        }
        cursor = next;
    }
    let want: Vec<String> = (0..130).map(|i| format!("file {i:03}")).collect();
    assert_eq!(names, want);
    // A page of one at a time also works, and never repeats or skips.
    let (mut cursor, mut n) = (0, 0);
    loop {
        let (page, next) = c.list(root, cursor, 1).unwrap();
        n += page.len();
        if next == 0 {
            break;
        }
        cursor = next;
    }
    assert_eq!(n, 130);
}

#[test]
fn rights_only_shrink() {
    let (mut c, root) = new();
    let (docs, _) = c.create(root, "Docs", Kind::Folder, ALL).unwrap();
    let (f, _) = c.create(docs, "a.txt", Kind::File, ALL).unwrap();
    c.write(f, 0, b"x").unwrap();
    let ro = c.reduce(docs, READ).unwrap();
    assert_eq!(c.info(ro).unwrap().rights, READ);
    // A read-only folder: nothing inside it can be changed, or opened for changing.
    assert_eq!(c.create(ro, "b.txt", Kind::File, READ).map(|_| ()), Err(Error::NoRight));
    assert_eq!(c.remove(ro, "a.txt"), Err(Error::NoRight));
    assert_eq!(c.rename(ro, "a.txt", "b.txt", false), Err(Error::NoRight));
    assert_eq!(c.open(ro, "a.txt", ALL).map(|_| ()), Err(Error::NoRight));
    let (r, _) = c.open(ro, "a.txt", READ).unwrap();
    assert_eq!(c.write(r, 0, b"y"), Err(Error::NoRight));
    assert_eq!(c.set_size(r, 0), Err(Error::NoRight));
    assert_eq!(c.read(r, 0, 1).unwrap(), b"x");
    // And never grow back.
    assert_eq!(c.reduce(ro, ALL), Err(Error::NoRight));
    assert_eq!(c.reduce(ro, 4), Err(Error::BadArgument));
    // A write-only folder can't be listed.
    let wo = c.reduce(docs, WRITE).unwrap();
    assert_eq!(c.list(wo, 0, 1).map(|_| ()), Err(Error::NoRight));
    // No rights at all: still `info`.
    let none = c.reduce(docs, 0).unwrap();
    assert!(c.info(none).is_ok());
    assert_eq!(c.open(none, "a.txt", READ).map(|_| ()), Err(Error::NoRight));
}

#[test]
fn paths_only_go_down() {
    for bad in ["", "/", "/a", "a/", "a//b", ".", "..", "a/..", "../a", "a/./b", "a/../b"] {
        assert_eq!(split_path(bad), Err(Error::BadPath), "{bad:?}");
    }
    assert_eq!(split_path(&"a/".repeat(2049)), Err(Error::BadPath)); // over 4096 bytes
    for bad in ["a\0b", "tab\there", "del\u{7F}", &"x".repeat(256)] {
        assert_eq!(split_path(bad), Err(Error::BadName), "{bad:?}");
    }
    assert_eq!(split_path(&"é".repeat(255)).unwrap().len(), 1); // characters, not bytes
    assert_eq!(split_path("a/b c/ünï.txt").unwrap(), ["a", "b c", "ünï.txt"]);

    // A folder's holder can reach what's inside it, and nothing else.
    let (mut c, root) = new();
    let (docs, _) = c.create(root, "Docs", Kind::Folder, ALL).unwrap();
    c.create(root, "Secret", Kind::File, ALL).unwrap();
    assert_eq!(c.open(docs, "../Secret", READ).map(|_| ()), Err(Error::BadPath));
    assert_eq!(c.open(docs, "/Secret", READ).map(|_| ()), Err(Error::BadPath));
    assert_eq!(c.open(docs, "Secret", READ).map(|_| ()), Err(Error::NotFound));
}

#[test]
fn removing_and_renaming() {
    let (mut c, root) = new();
    let (a, _) = c.create(root, "A", Kind::Folder, ALL).unwrap();
    c.create(root, "A/B", Kind::Folder, ALL).unwrap();
    let (f, _) = c.create(root, "A/B/f.txt", Kind::File, ALL).unwrap();
    c.write(f, 0, b"data").unwrap();
    // Moving keeps the node: capabilities for it still work.
    c.rename(root, "A/B/f.txt", "g.txt", false).unwrap();
    assert_eq!(c.read(f, 0, 10).unwrap(), b"data");
    assert_eq!(c.open(root, "A/B/f.txt", READ).map(|_| ()), Err(Error::NotFound));
    // A folder can't move into itself.
    assert_eq!(c.rename(root, "A", "A/B/A", false), Err(Error::BadArgument));
    assert_eq!(c.rename(a, "B", "B/C", false), Err(Error::BadArgument));
    // Replacing a file, only when asked.
    let (h, _) = c.create(root, "h.txt", Kind::File, ALL).unwrap();
    assert_eq!(c.rename(root, "g.txt", "h.txt", false), Err(Error::AlreadyExists));
    c.rename(root, "g.txt", "h.txt", true).unwrap();
    assert_eq!(c.info(h).map(|_| ()), Err(Error::Gone)); // the replaced file is gone
    assert_eq!(c.read(f, 0, 10).unwrap(), b"data");
    assert_eq!(c.rename(root, "h.txt", "A", true), Err(Error::AlreadyExists)); // never a folder
    // Removing: only empty folders; capabilities for removed nodes are `Gone`.
    assert_eq!(c.remove(root, "A"), Err(Error::FolderNotEmpty));
    c.remove(root, "h.txt").unwrap();
    assert_eq!(c.read(f, 0, 1), Err(Error::Gone));
    assert_eq!(c.reduce(f, READ), Err(Error::Gone));
    c.remove(root, "A/B").unwrap();
    c.remove(root, "A").unwrap();
    assert_eq!(c.list(a, 0, 1).map(|_| ()), Err(Error::Gone));
    // A new node in the same place is a different node.
    let (_, _) = c.create(root, "A", Kind::Folder, ALL).unwrap();
    assert_eq!(c.info(a).map(|_| ()), Err(Error::Gone));
}

#[test]
fn records_go_and_badges_are_never_reused() {
    let (mut c, root) = new();
    let (f, _) = c.create(root, "f", Kind::File, ALL).unwrap();
    assert_eq!(c.records(), 2);
    c.gone(f);
    assert_eq!(c.records(), 1);
    // The old badge names nothing, even once its slot is used again.
    let (g, _) = c.open(root, "f", READ).unwrap();
    assert_ne!(g, f);
    assert_eq!(g & 0xFFFF_FFFF, f & 0xFFFF_FFFF); // the same slot...
    assert_eq!(c.read(f, 0, 1), Err(Error::BadArgument)); // ...but not the same record
    c.gone(f); // a second notice for a stale badge changes nothing
    assert_eq!(c.records(), 2);
    for _ in 0..1000 {
        let (b, _) = c.open(root, "f", READ).unwrap();
        c.gone(b);
    }
    assert_eq!(c.records(), 2);
    // Slot 0 is never used; the root, `g`, and one slot reused 1000 times.
    assert_eq!(c.slots_used(), 4, "records aren't reused");
}

#[test]
fn saving_when_the_last_writer_goes() {
    let (mut c, root) = new();
    let (f, _) = c.create(root, "f", Kind::File, ALL).unwrap();
    let (g, _) = c.open(root, "f", ALL).unwrap();
    let (r, _) = c.open(root, "f", READ).unwrap();
    c.write(f, 0, b"one").unwrap();
    c.write(g, 0, b"two").unwrap();
    c.gone(f);
    assert_eq!(c.fs.commits, 0); // `g` changed it too, and is still open
    c.gone(r);
    assert_eq!(c.fs.commits, 0); // a reader going saves nothing
    c.gone(g);
    assert_eq!(c.fs.commits, 1);
    // `sync` saves at once; a file nobody changed has nothing to save.
    let (h, _) = c.open(root, "f", ALL).unwrap();
    c.set_size(h, 1).unwrap();
    c.sync(h).unwrap();
    assert_eq!(c.fs.commits, 2);
    // A reader can't make a writer's changes durable before the writer says so.
    let (ro, _) = c.open(root, "f", READ).unwrap();
    c.set_size(h, 2).unwrap();
    assert_eq!(c.sync(ro), Err(Error::NoRight));
    assert_eq!(c.fs.commits, 2);
    c.gone(ro);
    c.gone(h);
    assert_eq!(c.fs.commits, 3);
}

#[test]
fn the_record_limit() {
    let mut c = Core::with_limit(MemFs::new("Test"), 3);
    let root = c.root().unwrap();
    let (a, _) = c.create(root, "a", Kind::File, ALL).unwrap();
    c.open(root, "a", READ).unwrap();
    assert_eq!(c.open(root, "a", READ).map(|_| ()), Err(Error::TooManyOpen));
    assert_eq!(c.reduce(root, READ), Err(Error::TooManyOpen));
    c.gone(a);
    assert!(c.open(root, "a", READ).is_ok());
}

#[test]
fn read_only_volumes() {
    let (mut c, root) = new();
    let (f, _) = c.create(root, "f", Kind::File, ALL).unwrap();
    c.fs.read_only = true;
    assert!(c.info(root).unwrap().read_only);
    assert_eq!(c.write(f, 0, b"x"), Err(Error::ReadOnly));
    assert_eq!(c.create(root, "g", Kind::File, ALL).map(|_| ()), Err(Error::ReadOnly));
    assert_eq!(c.remove(root, "f"), Err(Error::ReadOnly));
}

/// A small, repeatable random number generator (xorshift).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
        from[(self.next() % from.len() as u64) as usize]
    }
}

/// A client sending random requests with random paths and badges, real and made up: nothing
/// may panic, records must add up, and no badge may reach more than its rights allow.
#[test]
fn a_hostile_client() {
    let (mut c, root) = new();
    let mut rng = Rng(0x2545_F491_4F6C_DD1D);
    let paths = ["a", "b", "a/b", "a/b/c", "", "..", "a/../b", "/a", "A", "x/y", "a//b", "é"];
    let mut held: Vec<(u64, u32)> = alloc::vec![(root, ALL)];
    for _ in 0..20_000 {
        let i = (rng.next() % held.len() as u64) as usize;
        let (b, rights) = held[i];
        let b = if rng.next().is_multiple_of(10) { rng.next() & 0x7FFF_FFFF_FFFF_FFFF } else { b };
        let p = rng.pick(&paths);
        let want = (rng.next() % 4) as u32;
        let new = match rng.next() % 12 {
            0 => c.open(b, p, want).map(|x| x.0),
            1 => c
                .create(
                    b,
                    p,
                    if rng.next().is_multiple_of(2) { Kind::File } else { Kind::Folder },
                    want,
                )
                .map(|x| x.0),
            2 => c.reduce(b, want),
            3 => c.remove(b, p).map(|_| 0),
            4 => c.rename(b, p, rng.pick(&paths), rng.next().is_multiple_of(2)).map(|_| 0),
            5 => c.write(b, rng.next() % 100, b"hello").map(|_| 0),
            6 => c.read(b, rng.next() % 100, 50).map(|_| 0),
            7 => c.set_size(b, rng.next() % 200).map(|_| 0),
            8 => c.list(b, rng.next() % 3, 5).map(|_| 0),
            9 => c.sync(b).map(|_| 0),
            10 => c.info(b).map(|_| 0),
            _ => {
                if held.len() > 1 && i != 0 {
                    let (gone, _) = held.swap_remove(i);
                    c.gone(gone);
                }
                Ok(0)
            }
        };
        // A new record never has more rights than the record it came from.
        if let Ok(n) = new
            && n != 0
        {
            let r = c.info(n).map(|x| x.rights).unwrap_or(0);
            if b == held[i.min(held.len() - 1)].0 {
                assert_eq!(r & !rights, 0, "a record gained rights");
            }
            held.push((n, r));
        }
    }
    for (b, _) in held.drain(..) {
        c.gone(b);
    }
    assert_eq!(c.records(), 0);
}

/// Records per grant (note 0026, section 4): a limit holds whatever is opened through it, a
/// limit inside another counts against both, and one full share doesn't stop another.
#[test]
fn limits_share_records() {
    let (mut c, root) = new();
    let (a, _) = c.create(root, "a", Kind::Folder, ALL).unwrap();
    let (f, _) = c.create(root, "a/f", Kind::File, ALL).unwrap();
    c.gone(f);
    // A share of 3: the limited record itself and two more.
    let g = c.limit(a, ALL, 3).unwrap();
    assert_eq!(c.info(g).unwrap().rights, ALL);
    let (f1, _) = c.open(g, "f", READ).unwrap();
    let f2 = c.reduce(f1, READ).unwrap();
    assert_eq!(c.open(g, "f", READ).map(|_| ()), Err(Error::TooManyOpen));
    assert_eq!(c.reduce(g, READ), Err(Error::TooManyOpen));
    // Creating through a full share makes nothing.
    assert_eq!(c.create(g, "new", Kind::File, ALL).map(|_| ()), Err(Error::TooManyOpen));
    assert_eq!(c.open(a, "new", READ).map(|_| ()), Err(Error::NotFound));
    // Another grant of the same folder is unaffected.
    let h = c.limit(a, READ, 2).unwrap();
    let (h1, _) = c.open(h, "f", READ).unwrap();
    // And the unlimited record above can still open.
    let (x, _) = c.open(a, "f", READ).unwrap();
    c.gone(f2);
    let (f3, _) = c.open(g, "f", READ).unwrap();
    // A limit within a limit: never above it, and counting against both.
    assert_eq!(c.limit(g, READ, 4), Err(Error::BadArgument));
    c.gone(f3);
    let inner = c.limit(g, READ, 3).unwrap();
    assert_eq!(c.open(inner, "f", READ).map(|_| ()), Err(Error::TooManyOpen));
    c.gone(f1);
    let (i1, _) = c.open(inner, "f", READ).unwrap();
    // Rights still only shrink; zero records is no share at all.
    assert_eq!(c.limit(h, ALL, 1), Err(Error::NoRight));
    assert_eq!(c.limit(a, READ, 0), Err(Error::BadArgument));
    assert_eq!(c.limit(a, 4, 1), Err(Error::BadArgument));
    assert_eq!(c.limits_used(), 3);
    for b in [i1, inner, g, h1, h, x] {
        c.gone(b);
    }
    assert_eq!(c.limits_used(), 0);
    // The limits' slots are reused.
    let again = c.limit(a, READ, 1).unwrap();
    assert_eq!(c.limits_used(), 1);
    c.gone(again);
    c.gone(a);
    c.gone(root);
    assert_eq!(c.records(), 0);
    assert_eq!(c.limits_used(), 0);
}

/// Random opens, reductions, limits and goings, checked against a model that works out every
/// limit's count from which record each record was made through.
#[test]
fn limits_against_a_model() {
    let mut c = Core::with_limit(MemFs::new("Test"), 40);
    let root = c.root().unwrap();
    c.create(root, "f", Kind::File, ALL).map(|(b, _)| c.gone(b)).unwrap();
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    // The model: for each live record, its badge and the chain of limits above it (as ids
    // into `maxes`), nearest first.
    let mut maxes: Vec<u32> = Vec::new();
    let mut held: Vec<(u64, Vec<usize>)> = alloc::vec![(root, Vec::new())];
    let count =
        |held: &[(u64, Vec<usize>)], l: usize| held.iter().filter(|h| h.1.contains(&l)).count();
    for _ in 0..20_000 {
        let i = (rng.next() % held.len() as u64) as usize;
        let (b, chain) = held[i].clone();
        let room = held.len() < 40 && chain.iter().all(|&l| count(&held, l) < maxes[l] as usize);
        match rng.next() % 5 {
            0 | 1 => {
                let r = if c.info(b).unwrap().stat.kind == Kind::Folder {
                    c.open(b, "f", READ).map(|x| x.0)
                } else {
                    c.reduce(b, READ)
                };
                assert_eq!(r.is_ok(), room, "open or reduce disagreed with the model");
                if let Ok(n) = r {
                    held.push((n, chain));
                }
            }
            2 => {
                let records = (rng.next() % 6) as u32 + 1;
                let nearest = chain.first().map(|&l| maxes[l]);
                let r = c.limit(b, READ, records);
                if nearest.is_some_and(|m| records > m) {
                    assert_eq!(r, Err(Error::BadArgument));
                } else {
                    assert_eq!(r.is_ok(), room, "limit disagreed with the model");
                }
                if let Ok(n) = r {
                    maxes.push(records);
                    let mut chain = chain;
                    chain.insert(0, maxes.len() - 1);
                    held.push((n, chain));
                }
            }
            _ => {
                if i != 0 {
                    let (gone, _) = held.swap_remove(i);
                    c.gone(gone);
                }
            }
        }
        assert_eq!(c.records(), held.len());
        // A limit is kept exactly while something is under it.
        let live = (0..maxes.len()).filter(|&l| count(&held, l) > 0).count();
        assert_eq!(c.limits_used(), live);
    }
    for (b, _) in held.drain(..) {
        c.gone(b);
    }
    assert_eq!(c.records(), 0);
    assert_eq!(c.limits_used(), 0);
}
