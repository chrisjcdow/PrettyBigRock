// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The desktop's app icons (1.3's `draw_icons` and `icon_cell`): a column down the left of the
//! wallpaper, one for each app in the start menu, that a double click launches.
//!
//! 1.3 had seven and stacked them in one column. Here there may be more (any app with a window
//! gets one), so when a column is full the next shortcut starts a new column to its right.

use crate::icons::{self, Icon};
use crate::look;
use pbr_draw::{Canvas, Rect, TextStyle};

/// Where the first cell is, from the screen's top left corner.
const MARGIN: i32 = 16;
/// A cell's size (1.3's `ICON_CELL_W`, and `ICON_CELL_H` less the 6 pixels between rows).
const CELL_W: i32 = 84;
const CELL_H: i32 = 76;
/// How far apart cells are, down a column and from column to column.
const ROW_PITCH: i32 = 82;
const COLUMN_PITCH: i32 = CELL_W + 6;

/// One desktop icon: its picture and its name under it.
#[derive(Clone, Copy, Debug)]
pub struct Shortcut<'a> {
    /// The picture, drawn at three times its size.
    pub icon: Icon,
    /// The name under it.
    pub label: &'a str,
}

/// How many cells fit in a column on a screen whose usable height is `height`: at least one,
/// so that a very short screen still gets its icons (cut off at the bottom, but reachable).
fn rows(height: i32) -> i64 {
    let room = i64::from(height) - i64::from(MARGIN) - i64::from(CELL_H);
    if room < 0 { 1 } else { room / i64::from(ROW_PITCH) + 1 }
}

/// Cell `i`'s rectangle, in logical pixels, on a screen whose usable height (above the
/// taskbar) is `height`: 1.3's column (x 16, from y 16, 84 x 76, 82 apart), continuing in a new
/// column to the right when a column is full.
pub fn cell(i: usize, height: i32) -> Rect {
    let rows = rows(height);
    // `try_from` turns a `usize` into an `i64` if it fits; a number too big to is cut to the
    // biggest (so a silly `i` gives a cell far off the screen, not a panic).
    let i = i64::try_from(i).unwrap_or(i64::MAX);
    let (column, row) = (i / rows, i % rows);
    let x = i64::from(MARGIN).saturating_add(column.saturating_mul(i64::from(COLUMN_PITCH)));
    let y = i64::from(MARGIN) + row * i64::from(ROW_PITCH);
    Rect::new(x.min(i64::from(i32::MAX)) as i32, y as i32, CELL_W, CELL_H)
}

/// Which shortcut is at (`x`, `y`), if any: the gaps between cells are nothing.
pub fn at(count: usize, height: i32, x: i32, y: i32) -> Option<usize> {
    let (dx, dy) = (i64::from(x) - i64::from(MARGIN), i64::from(y) - i64::from(MARGIN));
    if dx < 0 || dy < 0 {
        return None;
    }
    // Which column and row the point is in, if the cells were laid out for ever; then whether
    // it is on the cell itself rather than the gap after it. No search through the shortcuts.
    let (column, in_x) = (dx / i64::from(COLUMN_PITCH), dx % i64::from(COLUMN_PITCH));
    let (row, in_y) = (dy / i64::from(ROW_PITCH), dy % i64::from(ROW_PITCH));
    if in_x >= i64::from(CELL_W) || in_y >= i64::from(CELL_H) || row >= rows(height) {
        return None;
    }
    let index = column.checked_mul(rows(height))?.checked_add(row)?;
    let index = usize::try_from(index).ok()?;
    (index < count).then_some(index)
}

/// Draws them all (1.3's look): the icon at size 3 (48 x 48), the label under it with a
/// one-pixel shadow; the selected one with a translucent white box and its outline.
pub fn draw(
    canvas: &mut Canvas<'_>,
    shortcuts: &[Shortcut<'_>],
    selected: Option<usize>,
    height: i32,
) {
    let visible = canvas.clip_bounds();
    for (i, shortcut) in shortcuts.iter().enumerate() {
        let c = cell(i, height);
        // Cells are laid out left to right, so the first one past the canvas's right edge ends
        // the job: a very long list costs no more than the part that shows.
        if i64::from(c.x) >= visible.right() {
            break;
        }
        if selected == Some(i) {
            canvas.fill_rect(c, look::C_SELECT_FILL);
            canvas.stroke_rect(c, 1, look::C_SELECT_EDGE);
        }
        icons::draw(canvas, shortcut.icon, c.x + (c.width - 48) / 2, c.y + 6, 3);

        // The label is centred under the icon; one too long for the cell starts at its left
        // edge and is cut off at its right, rather than running over the next icon.
        let style = TextStyle::PLAIN;
        let x = c.x + ((c.width - style.width(shortcut.label)) / 2).max(0);
        let mut inside = canvas.clipped(c);
        crate::text(&mut inside, x + 1, c.y + 61, shortcut.label, look::C_LABEL_SHADOW, style);
        crate::text(&mut inside, x, c.y + 60, shortcut.label, look::C_WHITE, style);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_column_is_1_3s() {
        assert_eq!(cell(0, 400), Rect::new(16, 16, 84, 76));
        assert_eq!(cell(1, 400), Rect::new(16, 98, 84, 76));
        assert_eq!(cell(2, 400), Rect::new(16, 180, 84, 76));
    }
}
