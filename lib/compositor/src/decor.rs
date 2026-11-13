// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Where the parts of a window's decorations are: its frame, title bar, buttons and the edges
//! that resize it (docs/design/0022-display-server.md, section 6), in 1.3's sizes
//! (`classic/kernel/wm.c`).
//!
//! The display server draws every `Normal`, `Dialog` and `System` window's decorations
//! itself, outside the app's picture, so that no app can draw a title bar that lies about whose
//! window it is. Drawing them (`draw.rs`) and working out what a click on them means (`input.rs`)
//! both use the rectangles here, so the two can't disagree.
//!
//! Everything is in logical pixels, from the window's *content*: the rectangle the app draws
//! in, whose top left corner is the window's place in the space.

use pbr_draw::Rect;

/// The frame's border, all round.
pub const BORDER: i32 = 1;
/// The title bar's height, between the top border and the content.
pub const TITLE: i32 = 26;
/// The title bar's round buttons and the icon: this wide and high.
pub const BUTTON: i32 = 16;
/// From the top of the frame to the top of the buttons.
pub const BUTTON_TOP: i32 = 5;
/// From one button's left edge to the next one's.
pub const BUTTON_STEP: i32 = 22;
/// How far outside the frame a resizable window's edges can be caught.
pub const GRIP: i32 = 4;
/// How far a window's shadow reaches beyond its frame: left, top, right, bottom. (The shadow
/// is the frame moved 2 right and 4 down, blurred over 6.)
pub const SHADOW: [i32; 4] = [4, 2, 8, 10];
/// The title bar's text starts this far from the frame's left edge (after the icon).
pub const TEXT_LEFT: i32 = 28;
/// The narrowest a decorated window's content may be, so the title bar's parts fit: the icon,
/// a few letters of the title, the "trusted" mark and the three buttons.
pub const MIN_WIDTH: i32 = 160;

/// A title bar button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    /// Asks the app to close the window.
    Close,
    /// Maximises or restores it.
    Maximise,
    /// Hides it until it is chosen in the taskbar.
    Minimise,
}

/// What a point on or near a window is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The app's own picture.
    Content,
    /// The title bar, where nothing else is: drag to move.
    Title,
    /// The icon at the left of the title bar: the window menu.
    Icon,
    /// One of the title bar's buttons.
    Button(Button),
    /// An edge or corner of a resizable window (`pbr.window`'s `Edge` numbers: 1 top, 2
    /// bottom, 3 left, 4 right, 5 top left, 6 top right, 7 bottom left, 8 bottom right).
    Edge(u32),
    /// The border of a window that can't be resized.
    Border,
}

/// The whole frame: content, title bar and border.
pub fn frame(content: Rect) -> Rect {
    Rect::new(
        content.x.saturating_sub(BORDER),
        content.y.saturating_sub(BORDER + TITLE),
        content.width.saturating_add(2 * BORDER),
        content.height.saturating_add(2 * BORDER + TITLE),
    )
}

/// The title bar, inside the border.
pub fn title_bar(content: Rect) -> Rect {
    Rect::new(content.x, content.y - TITLE, content.width, TITLE)
}

/// A title bar button: close at the right, then maximise, then minimise.
pub fn button(content: Rect, which: Button) -> Rect {
    let f = frame(content);
    let n = match which {
        Button::Close => 1,
        Button::Maximise => 2,
        Button::Minimise => 3,
    };
    let right = f.right().min(i64::from(i32::MAX)) as i32;
    Rect::new(right - n * BUTTON_STEP, f.y + BUTTON_TOP, BUTTON, BUTTON)
}

/// The icon at the left of the title bar.
pub fn icon(content: Rect) -> Rect {
    let f = frame(content);
    Rect::new(f.x + 6, f.y + BUTTON_TOP, BUTTON, BUTTON)
}

/// Where the title's text may go: after the icon, before the buttons.
pub fn text_area(content: Rect) -> Rect {
    let f = frame(content);
    let left = f.x + TEXT_LEFT;
    let right = button(content, Button::Minimise).x - 6;
    Rect::new(left, f.y, (right - left).max(0), TITLE + BORDER)
}

/// Everything a window can change on the screen: its frame (or, undecorated, its content)
/// and its shadow.
pub fn bounds(content: Rect, decorated: bool) -> Rect {
    if !decorated {
        return content;
    }
    let f = frame(content);
    Rect::from_edges(
        i64::from(f.x) - i64::from(SHADOW[0]),
        i64::from(f.y) - i64::from(SHADOW[1]),
        f.right() + i64::from(SHADOW[2]),
        f.bottom() + i64::from(SHADOW[3]),
    )
}

/// What the point (`x`, `y`) is on, for a window with this content: `None` if it is outside
/// the window altogether. A resizable window can also be caught a few pixels outside its
/// frame.
pub fn hit(content: Rect, decorated: bool, resizable: bool, x: i32, y: i32) -> Option<Part> {
    if content.contains(x, y) {
        return Some(Part::Content);
    }
    if !decorated {
        return None;
    }
    let f = frame(content);
    let reach = if resizable { f.inset(-GRIP) } else { f };
    if !reach.contains(x, y) {
        return None;
    }
    let inside = f.contains(x, y);
    if resizable {
        // Within the grip of an edge (or two, at a corner), counting the border itself.
        let (x, y) = (i64::from(x), i64::from(y));
        let near = |d: i64| d < i64::from(BORDER + GRIP);
        let left = near(x - i64::from(f.x) + i64::from(GRIP));
        let right = near(f.right() + i64::from(GRIP) - 1 - x);
        let top = near(y - i64::from(f.y) + i64::from(GRIP));
        let bottom = near(f.bottom() + i64::from(GRIP) - 1 - y);
        let edge = match (top, bottom, left, right) {
            (true, _, true, _) => Some(5),
            (true, _, _, true) => Some(6),
            (_, true, true, _) => Some(7),
            (_, true, _, true) => Some(8),
            (true, ..) => Some(1),
            (_, true, ..) => Some(2),
            (_, _, true, _) => Some(3),
            (_, _, _, true) => Some(4),
            _ => None,
        };
        // The top edge is shared with the title bar's border; the buttons and icon are well
        // inside, so they win only where the point is on them.
        if let Some(e) = edge
            && !(inside && on_title_parts(content, x as i32, y as i32))
        {
            return Some(Part::Edge(e));
        }
    }
    if !inside {
        return None;
    }
    if on_title_parts(content, x, y) || title_bar(content).contains(x, y) {
        for b in [Button::Close, Button::Maximise, Button::Minimise] {
            if button(content, b).contains(x, y) {
                return Some(Part::Button(b));
            }
        }
        if icon(content).contains(x, y) {
            return Some(Part::Icon);
        }
        return Some(Part::Title);
    }
    Some(Part::Border)
}

/// Whether the point is on one of the title bar's buttons or its icon.
fn on_title_parts(content: Rect, x: i32, y: i32) -> bool {
    [Button::Close, Button::Maximise, Button::Minimise]
        .into_iter()
        .any(|b| button(content, b).contains(x, y))
        || icon(content).contains(x, y)
}

/// The window menu's rows, at the server's own size.
pub const MENU_ROW: i32 = 22;
/// The window menu's width.
pub const MENU_WIDTH: i32 = 220;

/// Everything the window menu at `rect` can change on the screen: the menu and its shadow.
pub fn menu_bounds(rect: Rect) -> Rect {
    Rect::from_edges(
        i64::from(rect.x) - 8,
        i64::from(rect.y) - 8,
        rect.right() + 16,
        rect.bottom() + 16,
    )
}

/// The pointer's picture is at most this big, from its hot spot up and left by
/// [`POINTER_HOT`]: what moving it changes.
pub const POINTER_SIZE: i32 = 24;
/// How far the pointer's picture reaches up and left of the point it points at.
pub const POINTER_HOT: i32 = 12;

/// The rectangle the pointer's picture may cover when it points at (`x`, `y`).
pub fn pointer_bounds(x: i32, y: i32) -> Rect {
    Rect::new(
        x.saturating_sub(POINTER_HOT),
        y.saturating_sub(POINTER_HOT),
        POINTER_SIZE * 2,
        POINTER_SIZE * 2,
    )
}
