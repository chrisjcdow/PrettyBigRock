// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Scroll bars, shared by everything that scrolls ([`Scroll`](super::Scroll), lists, the text
//! editor): where the thumb is, drawing it, and which offset a dragged thumb stands for.
//!
//! An *offset* is how far the content is scrolled, in logical pixels from its start: 0 shows
//! its beginning, `content - view` its end.

use crate::geometry::Rect;
use crate::theme::Theme;
use pbr_draw::Canvas;

/// How thick a scroll bar is.
pub const THICKNESS: i32 = 12;
/// How far one step of the wheel scrolls (three lines of text).
pub const WHEEL_STEP: i32 = 33;
/// The shortest a thumb is, so that it can always be grabbed.
const MIN_THUMB: i32 = 16;

/// A scroll bar's track and thumb.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bar {
    /// The whole bar.
    pub track: Rect,
    /// The part that shows what is in view, and can be dragged.
    pub thumb: Rect,
    /// Up and down (true) or across.
    pub vertical: bool,
}

/// The largest offset: how far `content` can scroll in `view`.
pub fn max_offset(content: i32, view: i32) -> i32 {
    content.saturating_sub(view).max(0)
}

/// `offset` brought within 0 and the largest offset.
pub fn clamp(offset: i32, content: i32, view: i32) -> i32 {
    offset.clamp(0, max_offset(content, view))
}

/// The bar in `track` for `content` long content seen `view` at a time, scrolled by `offset`,
/// or `None` if everything fits (no bar is shown then).
pub fn bar(track: Rect, vertical: bool, content: i32, view: i32, offset: i32) -> Option<Bar> {
    if content <= view || view <= 0 {
        return None;
    }
    let len = if vertical { track.height } else { track.width };
    let thumb_len = ((i64::from(len) * i64::from(view) / i64::from(content)) as i32)
        .clamp(MIN_THUMB.min(len), len);
    let travel = len - thumb_len;
    let most = max_offset(content, view);
    let pos = if most == 0 {
        0
    } else {
        (i64::from(travel) * i64::from(clamp(offset, content, view)) / i64::from(most)) as i32
    };
    let thumb = if vertical {
        Rect::new(track.x, track.y + pos, track.width, thumb_len)
    } else {
        Rect::new(track.x + pos, track.y, thumb_len, track.height)
    };
    Some(Bar { track, thumb, vertical })
}

impl Bar {
    /// Draws the bar: the track, and a rounded thumb inside it.
    pub fn draw(&self, canvas: &mut Canvas<'_>, theme: &Theme, pressed: bool) {
        canvas.fill_rect(self.track, theme.scroll_track);
        let colour = if pressed { theme.button_edge_pressed } else { theme.scroll_thumb };
        canvas.fill_round_rect(self.thumb.inset(2), 3, colour);
    }

    /// The offset that puts the thumb's start at `pos` along the track (where a drag moved it,
    /// in the window's coordinates), for `content` seen `view` at a time.
    pub fn offset_at(&self, pos: i32, content: i32, view: i32) -> i32 {
        let (start, len, thumb) = if self.vertical {
            (self.track.y, self.track.height, self.thumb.height)
        } else {
            (self.track.x, self.track.width, self.thumb.width)
        };
        let travel = (len - thumb).max(1);
        let most = max_offset(content, view);
        clamp((i64::from(pos - start) * i64::from(most) / i64::from(travel)) as i32, content, view)
    }

    /// The thumb's start along the track (to remember where a drag began).
    pub fn thumb_start(&self) -> i32 {
        if self.vertical { self.thumb.y } else { self.thumb.x }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_bar_when_it_fits() {
        assert_eq!(bar(Rect::new(0, 0, 12, 100), true, 100, 100, 0), None);
        assert_eq!(bar(Rect::new(0, 0, 12, 100), true, 50, 100, 0), None);
    }

    #[test]
    fn the_thumb_shows_what_is_in_view_and_drags_back_to_the_same_offset() {
        let b = bar(Rect::new(0, 0, 12, 100), true, 400, 100, 0).unwrap();
        assert_eq!(b.thumb, Rect::new(0, 0, 12, 25));
        let end = bar(Rect::new(0, 0, 12, 100), true, 400, 100, 300).unwrap();
        assert_eq!(end.thumb, Rect::new(0, 75, 12, 25));
        assert_eq!(b.offset_at(75, 400, 100), 300);
        assert_eq!(b.offset_at(500, 400, 100), 300);
        assert_eq!(b.offset_at(-50, 400, 100), 0);
        let mid = bar(Rect::new(0, 0, 100, 12), false, 10_000, 100, 5_000).unwrap();
        assert!(mid.thumb.width >= MIN_THUMB);
    }
}
