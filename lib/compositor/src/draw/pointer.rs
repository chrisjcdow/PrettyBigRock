// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The pointer's shapes (`pbr.window`'s `Cursor`) and drawing the pointer.
//!
//! Each shape is a small bitmap written as rows of text, so that a person can see the shape in
//! the source: `#` is a black pixel, `.` a white one and a space is see-through. The outline is
//! black and the inside white so that the pointer shows on any colour. A bitmap pixel is one
//! *logical* pixel: on a 2x screen every pixel is drawn as a block of 2 by 2, which keeps the
//! shapes sharp without a second set of bitmaps.
//!
//! Every shape fits in [`MAX`] by [`MAX`] pixels and has its hot spot (the pixel that is "the
//! point" the pointer points at) no further than [`decor::POINTER_HOT`] from the bitmap's
//! top left corner, so it stays inside `decor::pointer_bounds`, which is what the desk marks
//! as changed when the pointer moves. If a shape strayed outside it, moving the pointer would
//! leave bits of the old picture behind on the screen.

use super::Origin;
use crate::decor;
use pbr_draw::{Canvas, Picture, Rect};
use pbr_interfaces::pbr_window::Cursor;

/// The biggest a shape may be, each way.
const MAX: usize = 24;

/// One shape: where its hot spot is, and its rows of pixels.
struct Shape {
    /// The hot spot, in bitmap pixels from the top left.
    hot: (i32, i32),
    /// The rows, all the same length.
    rows: &'static [&'static str],
}

const ARROW: Shape = Shape {
    hot: (0, 0),
    rows: &[
        "#           ",
        "##          ",
        "#.#         ",
        "#..#        ",
        "#...#       ",
        "#....#      ",
        "#.....#     ",
        "#......#    ",
        "#.......#   ",
        "#........#  ",
        "#.........# ",
        "#......#####",
        "#...#..#    ",
        "#..# #..#   ",
        "#.#  #..#   ",
        "##    #..#  ",
        "#     #..#  ",
        "       #..# ",
        "        ##  ",
    ],
};

const TEXT: Shape = Shape {
    hot: (3, 7),
    rows: &[
        ".#####.", "...#...", "...#...", "...#...", "...#...", "...#...", "...#...", "...#...",
        "...#...", "...#...", "...#...", "...#...", "...#...", "...#...", ".#####.",
    ],
};

const HAND: Shape = Shape {
    hot: (5, 0),
    rows: &[
        "    ####        ",
        "    #..#        ",
        "    #..#        ",
        "    #..#        ",
        "    #..#####    ",
        "    #......###  ",
        "#####........## ",
        "#..##.........# ",
        "##............# ",
        " ##...........# ",
        "  #..........## ",
        "  ##.........#  ",
        "   #........##  ",
        "   ##.......#   ",
        "    #########   ",
    ],
};

const MOVE: Shape = Shape {
    hot: (9, 9),
    rows: &[
        "        ###        ",
        "       ##.##       ",
        "      ##...##      ",
        "     ##.....##     ",
        "     #.......#     ",
        "   ######.######   ",
        "  ##.#  #.#  #.##  ",
        " ##..#  #.#  #..## ",
        "##...####.####...##",
        "#.................#",
        "##...####.####...##",
        " ##..#  #.#  #..## ",
        "  ##.#  #.#  #.##  ",
        "   ######.######   ",
        "     #.......#     ",
        "     ##.....##     ",
        "      ##...##      ",
        "       ##.##       ",
        "        ###        ",
    ],
};

const RESIZE_NS: Shape = Shape {
    hot: (5, 10),
    rows: &[
        "    ###    ",
        "   ##.##   ",
        "  ##...##  ",
        " ##.....## ",
        " #.......# ",
        " ####.#### ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        "    #.#    ",
        " ####.#### ",
        " #.......# ",
        " ##.....## ",
        "  ##...##  ",
        "   ##.##   ",
        "    ###    ",
    ],
};

const RESIZE_EW: Shape = Shape {
    hot: (10, 5),
    rows: &[
        "                     ",
        "   ###         ###   ",
        "  ##.#         #.##  ",
        " ##..#         #..## ",
        "##...###########...##",
        "#...................#",
        "##...###########...##",
        " ##..#         #..## ",
        "  ##.#         #.##  ",
        "   ###         ###   ",
        "                     ",
    ],
};

const RESIZE_NESW: Shape = Shape {
    hot: (8, 8),
    rows: &[
        "         ########",
        "         #......#",
        "         ##.....#",
        "          ##....#",
        "          ##....#",
        "         ##.##..#",
        "        ##.####.#",
        "       ##.##  ###",
        "      ##.##      ",
        "###  ##.##       ",
        "#.####.##        ",
        "#..##.##         ",
        "#....##          ",
        "#....##          ",
        "#.....##         ",
        "#......#         ",
        "########         ",
    ],
};

const RESIZE_NWSE: Shape = Shape {
    hot: (8, 8),
    rows: &[
        "########         ",
        "#......#         ",
        "#.....##         ",
        "#....##          ",
        "#....##          ",
        "#..##.##         ",
        "#.####.##        ",
        "###  ##.##       ",
        "      ##.##      ",
        "       ##.##  ###",
        "        ##.####.#",
        "         ##.##..#",
        "          ##....#",
        "          ##....#",
        "         ##.....#",
        "         #......#",
        "         ########",
    ],
};

const WAIT: Shape = Shape {
    hot: (6, 8),
    rows: &[
        "#############",
        " #.........# ",
        " ##.......## ",
        "  #.......#  ",
        "  ##.....##  ",
        "   #.....#   ",
        "   ##...##   ",
        "    ##.##    ",
        "    ##.##    ",
        "   ##...##   ",
        "   #.....#   ",
        "  ##.....##  ",
        "  #.......#  ",
        " ##.......## ",
        " #.........# ",
        "#############",
    ],
};

const CROSSHAIR: Shape = Shape {
    hot: (8, 8),
    rows: &[
        "       ###       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       ###       ",
        "#######   #######",
        "#.....#   #.....#",
        "#######   #######",
        "       ###       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       #.#       ",
        "       ###       ",
    ],
};

/// The shape for a cursor, or `None` for the one that draws nothing.
fn shape(cursor: Cursor) -> Option<&'static Shape> {
    Some(match cursor {
        Cursor::Arrow => &ARROW,
        Cursor::Text => &TEXT,
        Cursor::Hand => &HAND,
        Cursor::Move => &MOVE,
        Cursor::ResizeNs => &RESIZE_NS,
        Cursor::ResizeEw => &RESIZE_EW,
        Cursor::ResizeNesw => &RESIZE_NESW,
        Cursor::ResizeNwse => &RESIZE_NWSE,
        Cursor::Wait => &WAIT,
        Cursor::Crosshair => &CROSSHAIR,
        Cursor::None => return None,
    })
}

/// Draws the pointer, pointing at (`x`, `y`) in the space, if any of what it covers is inside
/// `seen` (the part of the space being drawn).
pub(super) fn draw(c: &mut Canvas, o: Origin, x: i32, y: i32, cursor: Cursor, seen: Rect) {
    let Some(shape) = shape(cursor) else { return };
    if decor::pointer_bounds(x, y).intersect(&seen).is_empty() {
        return;
    }
    // The bitmap as pixels. `0xAARRGGBB`: a picture drawn `with_alpha` mixes its pixels over
    // what is there, so the clear ones (all zero) leave the screen as it was.
    let mut pixels = [0u32; MAX * MAX];
    let mut width = 0;
    for (row, text) in shape.rows.iter().take(MAX).enumerate() {
        for (col, ch) in text.bytes().take(MAX).enumerate() {
            pixels[row * MAX + col] = match ch {
                b'#' => 0xFF00_0000,
                b'.' => 0xFFFF_FFFF,
                _ => 0,
            };
            width = width.max(col + 1);
        }
    }
    let height = shape.rows.len().min(MAX);
    let Some(picture) = Picture::new(&pixels, width as u32, height as u32, MAX as u32) else {
        return;
    };
    let (x, y) = o.point(x, y);
    c.picture(&picture.with_alpha(), x.saturating_sub(shape.hot.0), y.saturating_sub(shape.hot.1));
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Cursor; 10] = [
        Cursor::Arrow,
        Cursor::Text,
        Cursor::Hand,
        Cursor::Move,
        Cursor::ResizeNs,
        Cursor::ResizeEw,
        Cursor::ResizeNesw,
        Cursor::ResizeNwse,
        Cursor::Wait,
        Cursor::Crosshair,
    ];

    #[test]
    fn every_shape_is_tidy_and_stays_inside_the_pointers_bounds() {
        for cursor in ALL {
            let s = shape(cursor).expect("a shape");
            let width = s.rows[0].len();
            assert!(s.rows.iter().all(|r| r.len() == width), "{cursor:?}: ragged rows");
            assert!(width <= MAX && s.rows.len() <= MAX, "{cursor:?}: too big");
            assert!(s.rows.iter().all(|r| r.bytes().all(|b| b"#. ".contains(&b))));
            assert!(s.hot.0 >= 0 && s.hot.1 >= 0 && (s.hot.0 as usize) < width);
            assert!((s.hot.1 as usize) < s.rows.len(), "{cursor:?}: hot spot outside");
            // Drawn at the point (100, 100), the bitmap must lie inside the bounds.
            let bounds = decor::pointer_bounds(100, 100);
            let drawn = Rect::new(100 - s.hot.0, 100 - s.hot.1, width as i32, s.rows.len() as i32);
            assert!(bounds.contains_rect(&drawn), "{cursor:?}: outside pointer_bounds");
        }
        assert!(shape(Cursor::None).is_none());
    }
}
