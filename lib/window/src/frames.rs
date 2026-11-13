// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Double buffering: which of a window's two frames the app may draw in, and what has to be
//! copied first. No system calls, so it is tested on the host.
//!
//! A window has two frames (note 0022, section 3). The app draws in one and *commits* it, which
//! gives it to the display server until a *released* event gives it back; meanwhile the app
//! can draw in the other. Because the two take turns, the frame the app is about to draw in is
//! one commit behind the other: it lacks whatever the last commit changed. An app that redraws
//! only what changed would leave that out of every second picture, so before drawing, the
//! rectangles of the last commit are copied from the other frame (the one with the newest
//! picture) into this one. Then both frames are whole again.

use alloc::vec::Vec;
use pbr_draw::{Damage, Rect};
use pbr_interfaces::pbr_window::Rect as WireRect;

/// What the last commit changed, in logical pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Changed {
    /// The whole window (the commit named no rectangles).
    All,
    /// These rectangles.
    Some(Vec<Rect>),
}

/// What to copy from the newest frame into the one about to be drawn in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyPlan<'a> {
    /// Nothing: no picture has been committed yet (both frames are blank).
    Nothing,
    /// All of the frame.
    All,
    /// These logical rectangles.
    Rects(&'a [Rect]),
}

/// Which frames the app holds, and what the newest commit changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frames {
    held: [bool; 2],
    /// The frame committed last, and what it changed. `None` before the first commit.
    last: Option<(usize, Changed)>,
}

impl Frames {
    /// New buffers: both frames blank and the app's.
    pub fn fresh() -> Frames {
        Frames { held: [true; 2], last: None }
    }

    /// The frame to draw in next: the one not committed last (frame 0 at first), or `None`
    /// if the display server still has it and the app must wait for a released event.
    pub fn next(&self) -> Option<usize> {
        let n = self.target();
        self.held[n].then_some(n)
    }

    fn target(&self) -> usize {
        self.last.as_ref().map_or(0, |(f, _)| 1 - f)
    }

    /// Whether the app holds frame `frame`.
    pub fn holds(&self, frame: usize) -> bool {
        self.held.get(frame).copied().unwrap_or(false)
    }

    /// What to copy into the frame [`Frames::next`] gave, from the other one.
    pub fn copy_before_drawing(&self) -> CopyPlan<'_> {
        match &self.last {
            None => CopyPlan::Nothing,
            Some((_, Changed::All)) => CopyPlan::All,
            Some((_, Changed::Some(r))) => CopyPlan::Rects(r),
        }
    }

    /// Records that `frame` was committed with `changed` (empty means all of it).
    pub fn committed(&mut self, frame: usize, changed: &[Rect]) {
        if frame > 1 {
            return;
        }
        self.held[frame] = false;
        let changed =
            if changed.is_empty() { Changed::All } else { Changed::Some(changed.to_vec()) };
        self.last = Some((frame, changed));
    }

    /// Records that the display server gave `frame` back. Anything but 0 or 1 is ignored.
    pub fn released(&mut self, frame: u32) {
        if let Some(h) = self.held.get_mut(frame as usize) {
            *h = true;
        }
    }
}

/// How many device pixels make a logical pixel at `scale` (hundredths), as a whole number
/// 1 to 8, the way `pbr_draw::Canvas` takes it.
pub fn pixel_scale(scale: u32) -> u32 {
    (scale.saturating_add(50) / 100).clamp(1, 8)
}

/// A logical rectangle as the part of a `width` by `height` frame (device pixels) it covers:
/// (left, top, right, bottom), or `None` if nothing is inside.
pub fn device_span(
    r: &Rect,
    factor: u32,
    width: u32,
    height: u32,
) -> Option<(usize, usize, usize, usize)> {
    let f = i64::from(factor);
    let left = (i64::from(r.x) * f).max(0);
    let top = (i64::from(r.y) * f).max(0);
    let right = ((i64::from(r.x) + i64::from(r.width.max(0))) * f).min(i64::from(width));
    let bottom = ((i64::from(r.y) + i64::from(r.height.max(0))) * f).min(i64::from(height));
    (right > left && bottom > top).then_some((
        left as usize,
        top as usize,
        right as usize,
        bottom as usize,
    ))
}

/// Copies the pixels of `span` (as from [`device_span`]) from `src` to `dst`, two frames of
/// `stride` pixels a row. Does nothing if either is too short for it.
pub fn copy_span(dst: &mut [u32], src: &[u32], stride: usize, span: (usize, usize, usize, usize)) {
    let (left, top, right, bottom) = span;
    for row in top..bottom {
        let (a, b) = (row * stride + left, row * stride + right);
        if let (Some(d), Some(s)) = (dst.get_mut(a..b), src.get(a..b)) {
            d.copy_from_slice(s);
        }
    }
}

/// Does what [`Frames::copy_before_drawing`] says, between two frames.
pub fn apply_copy(
    plan: CopyPlan<'_>,
    dst: &mut [u32],
    src: &[u32],
    (width, height, stride): (u32, u32, usize),
    factor: u32,
) {
    match plan {
        CopyPlan::Nothing => {}
        CopyPlan::All => copy_span(dst, src, stride, (0, 0, width as usize, height as usize)),
        CopyPlan::Rects(rects) => {
            for r in rects {
                if let Some(span) = device_span(r, factor, width, height) {
                    copy_span(dst, src, stride, span);
                }
            }
        }
    }
}

/// The rectangles to commit for `damage` (logical pixels) in a window of `size`: cut to the
/// window. An empty damage gives an empty list, which means all of it. Damage that is all
/// outside the window gives one tiny rectangle, so that it does not turn into "all".
pub fn wire_rects(damage: &Damage, size: (u32, u32)) -> Vec<WireRect> {
    let bounds =
        Rect::new(0, 0, size.0.min(i32::MAX as u32) as i32, size.1.min(i32::MAX as u32) as i32);
    let mut out: Vec<WireRect> = damage
        .rects()
        .iter()
        .map(|r| r.intersect(&bounds))
        .filter(|r| !r.is_empty())
        .map(|r| WireRect { x: r.x, y: r.y, width: r.width as u32, height: r.height as u32 })
        .collect();
    if out.is_empty() && !damage.is_empty() {
        out.push(WireRect { x: 0, y: 0, width: 1, height: 1 });
    }
    out
}

/// The same rectangles as the plain list `Frames::committed` wants: what was committed,
/// after cutting.
pub fn plain_rects(wire: &[WireRect]) -> Vec<Rect> {
    wire.iter().map(|r| Rect::new(r.x, r.y, r.width as i32, r.height as i32)).collect()
}

/// A title cut to at most 64 bytes (the most the interface carries), at a character boundary.
pub fn clip_title(title: &str) -> &str {
    let mut end = title.len().min(64);
    while !title.is_char_boundary(end) {
        end -= 1;
    }
    &title[..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn frames_take_turns() {
        let mut f = Frames::fresh();
        assert_eq!(f.next(), Some(0));
        assert_eq!(f.copy_before_drawing(), CopyPlan::Nothing);
        f.committed(0, &[Rect::new(1, 2, 3, 4)]);
        assert!(!f.holds(0) && f.holds(1));
        assert_eq!(f.next(), Some(1));
        assert_eq!(f.copy_before_drawing(), CopyPlan::Rects(&[Rect::new(1, 2, 3, 4)]));
        f.committed(1, &[]);
        assert_eq!(f.copy_before_drawing(), CopyPlan::All);
        // Both are the server's: nothing to draw in until one comes back.
        assert_eq!(f.next(), None);
        // The one it wants next is frame 0, and frame 1 coming back does not help.
        f.released(1);
        assert_eq!(f.next(), None);
        f.released(0);
        assert_eq!(f.next(), Some(0));
        f.released(7); // nonsense is ignored
        assert_eq!(f.next(), Some(0));
    }

    #[test]
    fn committing_twice_in_a_row_needs_the_other_back() {
        let mut f = Frames::fresh();
        f.committed(0, &[]);
        f.committed(1, &[Rect::new(0, 0, 1, 1)]);
        f.released(1); // the newer one back first: still not drawable (frame 0 is next)
        assert_eq!(f.next(), None);
        f.released(0);
        assert_eq!(f.next(), Some(0));
    }

    #[test]
    fn partial_redraws_keep_both_frames_whole() {
        // Simulate an app drawing one new column per picture into a 4x1 window, through the
        // copy rule, and check that the picture it commits is always complete.
        let (w, h, stride) = (4u32, 1u32, 4usize);
        let mut frames = [vec![0u32; 4], vec![0u32; 4]];
        let mut book = Frames::fresh();
        for step in 0..4u32 {
            let n = book.next().unwrap();
            let plan = book.copy_before_drawing();
            let (a, b) = frames.split_at_mut(1);
            let (dst, src) = if n == 0 { (&mut a[0], &b[0]) } else { (&mut b[0], &a[0]) };
            apply_copy(plan, dst, src, (w, h, stride), 1);
            dst[step as usize] = step + 1;
            book.committed(n, &[Rect::new(step as i32, 0, 1, 1)]);
            let so_far: Vec<u32> = (0..4).map(|i| if i <= step { i + 1 } else { 0 }).collect();
            assert_eq!(frames[n], so_far, "step {step}");
            book.released(0);
            book.released(1);
        }
    }

    #[test]
    fn spans_are_scaled_and_cut() {
        let r = Rect::new(2, 3, 4, 5);
        assert_eq!(device_span(&r, 1, 100, 100), Some((2, 3, 6, 8)));
        assert_eq!(device_span(&r, 2, 100, 100), Some((4, 6, 12, 16)));
        assert_eq!(device_span(&Rect::new(-5, -5, 7, 7), 1, 100, 100), Some((0, 0, 2, 2)));
        assert_eq!(device_span(&Rect::new(90, 90, 50, 50), 1, 100, 100), Some((90, 90, 100, 100)));
        assert_eq!(device_span(&Rect::new(200, 0, 5, 5), 1, 100, 100), None);
        assert_eq!(device_span(&Rect::new(0, 0, 0, 5), 1, 100, 100), None);
        assert_eq!(device_span(&Rect::new(0, 0, -3, 5), 1, 100, 100), None);
        assert_eq!(device_span(&Rect::new(i32::MAX, i32::MAX, i32::MAX, 9), 8, 100, 100), None);
        assert!(
            device_span(&Rect::new(i32::MIN, i32::MIN, i32::MAX, i32::MAX), 8, 10, 10).is_none()
        );
    }

    #[test]
    fn copy_uses_the_stride_and_leaves_the_rest() {
        let mut dst = vec![0u32; 12];
        let src: Vec<u32> = (1..=12).collect();
        // 3 wide, 3 high, stride 4: copy the middle column's rows 1 and 2.
        apply_copy(CopyPlan::Rects(&[Rect::new(1, 1, 1, 2)]), &mut dst, &src, (3, 3, 4), 1);
        assert_eq!(dst, [0, 0, 0, 0, 0, 6, 0, 0, 0, 10, 0, 0]);
        // A copy into frames that are too short does nothing and does not panic.
        let mut small = vec![0u32; 2];
        apply_copy(CopyPlan::All, &mut small, &src, (3, 3, 4), 1);
    }

    #[test]
    fn scales() {
        assert_eq!(
            [pixel_scale(0), pixel_scale(100), pixel_scale(200), pixel_scale(150)],
            [1, 1, 2, 2]
        );
        assert_eq!(pixel_scale(100_000), 8);
        assert_eq!(pixel_scale(u32::MAX), 8);
    }

    #[test]
    fn wire_rectangles() {
        let mut d = Damage::new();
        assert!(wire_rects(&d, (100, 50)).is_empty());
        d.add(Rect::new(-10, -10, 30, 30));
        d.add(Rect::new(90, 40, 100, 100));
        let w = wire_rects(&d, (100, 50));
        assert_eq!(w.len(), 2);
        assert_eq!(w[0], WireRect { x: 0, y: 0, width: 20, height: 20 });
        assert_eq!(w[1], WireRect { x: 90, y: 40, width: 10, height: 10 });
        let mut out = Damage::new();
        out.add(Rect::new(500, 500, 5, 5));
        assert_eq!(wire_rects(&out, (100, 50)).len(), 1);
        assert_eq!(plain_rects(&w), [Rect::new(0, 0, 20, 20), Rect::new(90, 40, 10, 10)]);
    }

    #[test]
    fn titles() {
        assert_eq!(clip_title("hello"), "hello");
        let long = "é".repeat(40); // 80 bytes
        let t = clip_title(&long);
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c == 'é'));
        assert_eq!(clip_title(&"a".repeat(100)).len(), 64);
        assert_eq!(clip_title(""), "");
    }
}
