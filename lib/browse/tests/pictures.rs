// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! The reference pictures of the icons (`tests/pictures/browse/`): after a change to an icon
//! that is meant, `PBR_UPDATE_PICTURES=1 cargo test -p pbr-browse`, then look at both pictures.

use pbr_browse::PlaceKind;
use pbr_browse::kind::ALL;
use pbr_testpng::Picture;
use std::path::PathBuf;

/// The light grey the sheets are painted.
const GREY: u32 = 0xFFF0_F0F0;
/// The space around and between the icons.
const GAP: u32 = 4;

/// Every icon on a sheet: the kinds in the first row, the places in the second, `scale` pixels
/// to a dot.
fn sheet(scale: u32) -> Picture {
    let kinds: Vec<&[u32; 256]> = ALL.iter().map(|k| k.icon()).collect();
    let places: Vec<&[u32; 256]> = [
        PlaceKind::Home,
        PlaceKind::Documents,
        PlaceKind::Pictures,
        PlaceKind::Music,
        PlaceKind::Videos,
        PlaceKind::Downloads,
        PlaceKind::Volume,
    ]
    .iter()
    .map(|k| k.icon())
    .collect();
    let rows = [kinds, places];
    let side = 16 * scale;
    let width = GAP + 8 * (side + GAP);
    let height = GAP + 2 * (side + GAP);
    let mut pixels = vec![GREY; (width * height) as usize];
    for (row, icons) in rows.iter().enumerate() {
        for (column, icon) in icons.iter().enumerate() {
            let left = GAP + column as u32 * (side + GAP);
            let top = GAP + row as u32 * (side + GAP);
            for (dot, &colour) in icon.iter().enumerate() {
                if colour >> 24 != 0xFF {
                    continue;
                }
                let (dx, dy) = ((dot % 16) as u32 * scale, (dot / 16) as u32 * scale);
                for y in 0..scale {
                    for x in 0..scale {
                        pixels[((top + dy + y) * width + left + dx + x) as usize] = colour;
                    }
                }
            }
        }
    }
    Picture { width, height, pixels }
}

fn check(name: &str, scale: u32) {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/pictures/browse")
        .join(format!("{name}.png"));
    if let Err(why) = pbr_testpng::check(&file, &sheet(scale)) {
        panic!("{name}: {why}");
    }
}

#[test]
fn every_icon_at_1x() {
    check("icons-1x", 1);
}

#[test]
fn every_icon_at_2x() {
    check("icons-2x", 2);
}
