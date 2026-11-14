// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Kinds of file and their icons.

use pbr_browse::Kind;
use pbr_browse::kind::{ALL, kind_of};

const TABLE: &[(Kind, &[&str])] = &[
    (
        Kind::Text,
        &[
            "txt", "md", "log", "ini", "cfg", "conf", "csv", "json", "toml", "rs", "c", "h", "py",
            "sh", "bat",
        ],
    ),
    (Kind::Picture, &["bmp", "png", "jpg", "jpeg", "gif"]),
    (Kind::Music, &["wav", "mp3", "ogg", "flac", "mid", "midi"]),
    (Kind::Video, &["avi", "mp4", "mkv", "webm", "mov"]),
    (Kind::Archive, &["zip", "tar", "gz", "tgz", "7z"]),
    (Kind::Program, &["app", "elf", "drv"]),
];

#[test]
fn every_extension_in_lower_and_upper_case() {
    for (kind, list) in TABLE {
        for ext in *list {
            assert_eq!(kind_of(&format!("file.{ext}"), false), *kind, "{ext}");
            assert_eq!(kind_of(&format!("FILE.{}", ext.to_uppercase()), false), *kind, "{ext}");
            assert_eq!(kind_of(&format!("a.b.{ext}"), false), *kind, "{ext}");
            // A folder is a folder, whatever its name says.
            assert_eq!(kind_of(&format!("file.{ext}"), true), Kind::Folder);
        }
    }
}

#[test]
fn everything_else_is_other() {
    for name in [
        "",
        "plain",
        "a.",
        ".",
        "..",
        "a.unknown",
        "txt",
        ".txt",
        ".profile",
        "a.txt ",
        "a.tx",
        "a.txtt",
        "a.ＴＸＴ",
        "a.ex\u{e9}",
    ] {
        assert_eq!(kind_of(name, false), Kind::Other, "{name:?}");
    }
    assert_eq!(kind_of("", true), Kind::Folder);
}

#[test]
fn dotfiles_use_their_last_extension() {
    assert_eq!(kind_of(".bashrc.txt", false), Kind::Text);
    assert_eq!(kind_of(".tar.gz", false), Kind::Archive);
}

#[test]
fn titles() {
    let titles: Vec<&str> = ALL.iter().map(|k| k.title()).collect();
    assert_eq!(
        titles,
        ["Folder", "Text document", "Picture", "Music", "Video", "Archive", "Program", "File"]
    );
    assert_eq!(ALL.len(), 8);
}

#[test]
fn icons_are_solid_or_clear_and_all_different() {
    for kind in ALL {
        let icon = kind.icon();
        assert!(icon.iter().any(|&d| d >> 24 == 0xFF), "{kind:?} draws something");
        assert!(icon.contains(&0), "{kind:?} leaves some clear");
        assert!(icon.iter().all(|&d| d == 0 || d >> 24 == 0xFF), "{kind:?}");
    }
    for (i, a) in ALL.iter().enumerate() {
        for b in &ALL[i + 1..] {
            assert_ne!(a.icon(), b.icon(), "{a:?} and {b:?}");
        }
    }
}

#[test]
fn the_folder_icon_is_1_3s_manila_folder() {
    let icon = Kind::Folder.icon();
    // Row 2 is ".nnnnnn.........": clear, six brown outline dots, clear.
    assert_eq!(icon[2 * 16], 0);
    assert_eq!(icon[2 * 16 + 1], 0xFF8A_5A35);
    assert_eq!(icon[2 * 16 + 6], 0xFF8A_5A35);
    assert_eq!(icon[2 * 16 + 7], 0);
    // The front of the folder is yellow (`y`).
    assert_eq!(icon[8 * 16 + 5], 0xFFF5_D04A);
}
