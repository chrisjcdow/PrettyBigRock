// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The rules a GPU display driver keeps about its screens
//! (docs/design/0020-screens-and-virtio-gpu.md, section 4), kept here so that they are tested
//! on the host: which modes a screen lists, which a client may ask for, how many rectangles a
//! present sends, and which picture a screen is showing.
//!
//! A *scanout* is a GPU's word for one screen's output. A GPU keeps its pictures in *resources*
//! (a numbered block of pixels in the GPU's own memory); a scanout shows one resource at a time.
//! The driver tells the GPU which resource each scanout shows, and sends it the parts of a
//! resource that changed.

use crate::display::Rect;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// The largest width or height the driver will use. A GPU resource this size is already
/// 1 GiB, so a bigger one is a lie or a mistake.
pub const MAX_SIDE: u32 = 16384;
/// The smallest width or height a client may ask a screen for.
pub const MIN_SIDE: u32 = 64;
/// The most modes `pbr.display` lists for a screen.
pub const MAX_MODES: usize = 16;
/// The modes a virtual screen is offered besides the one it prefers, best first. A virtual
/// screen has no fixed set of sizes of its own: it can show a picture of any size.
pub const STANDARD_MODES: [(u32, u32); 11] = [
    (1920, 1080),
    (1680, 1050),
    (1600, 900),
    (1440, 900),
    (1366, 768),
    (1280, 1024),
    (1280, 800),
    (1280, 720),
    (1024, 768),
    (800, 600),
    (640, 480),
];

/// The name of connector `index` (counting from 0, as the device does): `Virtual-1` for the
/// first. People count from 1.
pub fn connector_name(index: u32) -> String {
    format!("Virtual-{}", u64::from(index) + 1)
}

/// Whether a screen of this size could be real: not empty, and not absurdly big.
pub fn plausible(width: u32, height: u32) -> bool {
    (1..=MAX_SIDE).contains(&width) && (1..=MAX_SIDE).contains(&height)
}

/// The bytes a picture of `width` by `height` pixels takes, at four bytes a pixel, or `None` if
/// that doesn't fit in 64 bits. (`checked_mul` multiplies and gives `None` instead of wrapping
/// round when the answer is too big; `?` returns that `None` from the function at once.)
pub fn picture_bytes(width: u32, height: u32) -> Option<u64> {
    u64::from(width).checked_mul(u64::from(height))?.checked_mul(4)
}

/// The modes to list for a connected screen whose preferred size is `preferred`: that first,
/// then each of [`STANDARD_MODES`] that isn't the same size, at most [`MAX_MODES`] in all. A
/// preferred size that is not [plausible] is left out.
pub fn mode_list(preferred: (u32, u32)) -> Vec<(u32, u32)> {
    let mut modes = Vec::new();
    if plausible(preferred.0, preferred.1) {
        modes.push(preferred);
    }
    for mode in STANDARD_MODES {
        if modes.len() < MAX_MODES && mode != preferred {
            modes.push(mode);
        }
    }
    modes
}

/// What `set_mode` should do for a requested mode. (`derive` asks the compiler to write the
/// obvious code to copy, print and compare a type.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Choice {
    /// Turn the screen off (the width was 0).
    Off,
    /// Show pictures of this size from now on.
    Show(u32, u32),
    /// Refuse: too small, too big, or more than one buffer can hold.
    Bad,
}

/// Decides a requested mode. Any size from [`MIN_SIDE`] by [`MIN_SIDE`] up to what fits in one
/// buffer of `buffer_bytes` is accepted, since a virtual screen can show any size.
pub fn choose_mode(width: u32, height: u32, buffer_bytes: u64) -> Choice {
    if width == 0 {
        return Choice::Off;
    }
    let sides = (MIN_SIDE..=MAX_SIDE).contains(&width) && (MIN_SIDE..=MAX_SIDE).contains(&height);
    match picture_bytes(width, height) {
        Some(bytes) if sides && bytes <= buffer_bytes => Choice::Show(width, height),
        _ => Choice::Bad,
    }
}

fn right(r: &Rect) -> u64 {
    u64::from(r.x) + u64::from(r.width)
}

fn bottom(r: &Rect) -> u64 {
    u64::from(r.y) + u64::from(r.height)
}

fn area(r: &Rect) -> u128 {
    u128::from(r.width) * u128::from(r.height)
}

/// The smallest rectangle that holds both. Sizes that wouldn't fit in 32 bits are cut to fit,
/// which can only happen for rectangles no real buffer has.
fn union(a: &Rect, b: &Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    let (r, d) = (right(a).max(right(b)), bottom(a).max(bottom(b)));
    Rect {
        x,
        y,
        width: u32::try_from(r - u64::from(x)).unwrap_or(u32::MAX),
        height: u32::try_from(d - u64::from(y)).unwrap_or(u32::MAX),
    }
}

/// The smallest rectangle that holds all of `rects` (those with an area), or `None` if there
/// are none.
pub fn bounding_box(rects: &[Rect]) -> Option<Rect> {
    rects.iter().filter(|r| area(r) != 0).fold(None, |acc, r| match acc {
        None => Some(*r),
        Some(a) => Some(union(&a, r)),
    })
}

/// Reduces `rects` to at most `max` rectangles (at least 1) that together cover every pixel of
/// every one of them. Sending more than a client named is allowed (the pixels are the same in
/// the buffer), sending less is not, so rectangles are merged into their bounding boxes, the
/// pair that wastes the least area first. Empty rectangles are dropped.
pub fn merge_rects(rects: &[Rect], max: usize) -> Vec<Rect> {
    let max = max.max(1);
    let mut all: Vec<Rect> = rects.iter().copied().filter(|r| area(r) != 0).collect();
    while all.len() > max {
        // The cost of merging two is the area the merged rectangle adds beyond the two (they
        // may overlap, so it can be negative; `i128` holds it).
        let mut best: Option<(i128, usize, usize)> = None;
        for i in 0..all.len() {
            for j in i + 1..all.len() {
                let cost = area(&union(&all[i], &all[j])) as i128
                    - area(&all[i]) as i128
                    - area(&all[j]) as i128;
                if best.is_none_or(|(c, _, _)| cost < c) {
                    best = Some((cost, i, j));
                }
            }
        }
        let Some((_, i, j)) = best else { break };
        let merged = union(&all[i], &all[j]);
        all.swap_remove(j); // j > i, so this leaves `i` where it is
        all[i] = merged;
    }
    all
}

/// What one scanout is showing, and the bookkeeping that keeps its picture while buffers come
/// and go.
///
/// A client may destroy the buffer a screen is showing, and `pbr.display` says the screen then
/// keeps its last picture. The driver does that by taking the GPU resource's memory away (so
/// the buffer's memory can be used again) but keeping the resource itself until the scanout is
/// given another: that resource is a *leftover* here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// The resource the scanout was last told to show.
    shown: Option<u32>,
    /// A destroyed buffer's resource that is still on the scanout, to be freed once something
    /// else is.
    leftover: Option<u32>,
    /// How many presents have been sent to this scanout.
    presents: u64,
}

/// What to send for a present, in this order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PresentPlan {
    /// The scanout has to be told to show this resource first.
    pub set_scanout: bool,
    /// Then this leftover resource can be freed.
    pub free: Option<u32>,
    /// This present's number, to give back to [`Shown::is_latest`] when it is done.
    pub number: u64,
}

/// What to do with a buffer's resource when the buffer is destroyed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dispose {
    /// Free the resource.
    Free,
    /// Take its memory away but keep the resource, because the screen is showing it.
    Detach,
}

impl Shown {
    /// A scanout that has been told nothing.
    pub fn new() -> Shown {
        Shown::default()
    }

    /// The resource the scanout was last told to show.
    pub fn shown(&self) -> Option<u32> {
        self.shown
    }

    /// The leftover resource, if there is one.
    pub fn leftover(&self) -> Option<u32> {
        self.leftover
    }

    /// A present of `resource` is about to be sent.
    pub fn present(&mut self, resource: u32) -> PresentPlan {
        self.presents = self.presents.wrapping_add(1);
        let set_scanout = self.shown != Some(resource);
        let free = if set_scanout { self.leftover.take() } else { None };
        self.shown = Some(resource);
        PresentPlan { set_scanout, free, number: self.presents }
    }

    /// Buffer `resource` is destroyed.
    pub fn destroy(&mut self, resource: u32) -> Dispose {
        if self.shown == Some(resource) {
            self.leftover = Some(resource);
            Dispose::Detach
        } else {
            Dispose::Free
        }
    }

    /// The scanout is turned off, or its screen unplugged: it shows nothing now. Returns the
    /// leftover resource to free, if there is one.
    pub fn clear(&mut self) -> Option<u32> {
        self.shown = None;
        self.leftover.take()
    }

    /// Whether the present with this number is the newest sent to the scanout. A finished
    /// present is only reported if it is: the screen is busy for the newest one, and an older
    /// one finishing late (its buffer was destroyed, or a new client has taken over and
    /// already presented) must not end the newer one's wait.
    pub fn is_latest(&self, number: u64) -> bool {
        self.presents == number
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    fn r(x: u32, y: u32, width: u32, height: u32) -> Rect {
        Rect { x, y, width, height }
    }

    #[test]
    fn connectors_are_numbered_from_one() {
        assert_eq!(connector_name(0), "Virtual-1");
        assert_eq!(connector_name(15), "Virtual-16");
        assert_eq!(connector_name(u32::MAX), "Virtual-4294967296");
    }

    #[test]
    fn the_preferred_mode_comes_first_and_is_not_listed_twice() {
        let m = mode_list((1024, 768));
        assert_eq!(m[0], (1024, 768));
        assert_eq!(m.len(), 11, "ten standard modes besides it");
        assert_eq!(m.iter().filter(|x| **x == (1024, 768)).count(), 1);
        assert_eq!(m[1], (1920, 1080));
        // A preferred mode that is a standard one moves to the front.
        let m = mode_list((1920, 1080));
        assert_eq!(&m[..3], &[(1920, 1080), (1680, 1050), (1600, 900)]);
        assert_eq!(m.len(), 11);
        // An unusual one adds a twelfth.
        let m = mode_list((2560, 1440));
        assert_eq!((m[0], m[1], m.len()), ((2560, 1440), (1920, 1080), 12));
        // No preferred size (an implausible one): just the standard modes.
        for odd in [(0, 0), (0, 600), (800, 0), (MAX_SIDE + 1, 600)] {
            let m = mode_list(odd);
            assert_eq!(m, STANDARD_MODES.to_vec(), "{odd:?}");
        }
        assert!(mode_list((1, 1)).len() <= MAX_MODES);
    }

    #[test]
    fn sizes_are_chosen_by_what_fits_a_buffer() {
        let ten_mib = 10 << 20;
        assert_eq!(choose_mode(0, 0, ten_mib), Choice::Off);
        assert_eq!(
            choose_mode(0, 999, ten_mib),
            Choice::Off,
            "width 0 means off whatever the height"
        );
        assert_eq!(choose_mode(64, 64, ten_mib), Choice::Show(64, 64));
        assert_eq!(choose_mode(63, 64, ten_mib), Choice::Bad);
        assert_eq!(choose_mode(64, 63, ten_mib), Choice::Bad);
        assert_eq!(choose_mode(64, 0, ten_mib), Choice::Bad);
        assert_eq!(choose_mode(1920, 1080, ten_mib), Choice::Show(1920, 1080));
        // 1920 x 1366 x 4 is 10 489 344 bytes: just over 10 MiB (10 485 760).
        assert_eq!(choose_mode(1920, 1365, ten_mib), Choice::Show(1920, 1365));
        assert_eq!(choose_mode(1920, 1366, ten_mib), Choice::Bad);
        assert_eq!(choose_mode(3840, 2160, ten_mib), Choice::Bad);
        // Exactly full is fine.
        assert_eq!(choose_mode(1024, 2560, ten_mib), Choice::Show(1024, 2560));
        assert_eq!(choose_mode(1024, 2561, ten_mib), Choice::Bad);
        // Hostile numbers.
        assert_eq!(choose_mode(u32::MAX, u32::MAX, u64::MAX), Choice::Bad);
        assert_eq!(choose_mode(MAX_SIDE + 1, 64, u64::MAX), Choice::Bad);
        assert_eq!(choose_mode(64, MAX_SIDE + 1, u64::MAX), Choice::Bad);
        assert_eq!(choose_mode(64, 64, 0), Choice::Bad);
        assert_eq!(picture_bytes(u32::MAX, u32::MAX), None);
        assert_eq!(picture_bytes(2, 3), Some(24));
    }

    #[test]
    fn few_rectangles_are_left_alone_and_many_are_merged_to_cover_them() {
        let two = [r(0, 0, 10, 10), r(50, 50, 10, 10)];
        assert_eq!(merge_rects(&two, 4), two.to_vec());
        assert_eq!(merge_rects(&two, 2), two.to_vec());
        assert_eq!(merge_rects(&two, 1), vec![r(0, 0, 60, 60)]);
        // Empty ones go, even when there is room.
        assert_eq!(merge_rects(&[r(0, 0, 0, 5), r(1, 1, 2, 2)], 4), vec![r(1, 1, 2, 2)]);
        assert!(merge_rects(&[], 4).is_empty());
        assert_eq!(merge_rects(&two, 0).len(), 1, "never fewer than one");
        // The pair that wastes least is merged first: two neighbours, and one far away.
        let three = [r(0, 0, 10, 10), r(10, 0, 10, 10), r(500, 500, 10, 10)];
        let merged = merge_rects(&three, 2);
        assert_eq!(merged.len(), 2);
        assert!(merged.contains(&r(0, 0, 20, 10)) && merged.contains(&r(500, 500, 10, 10)));
        assert_eq!(bounding_box(&three), Some(r(0, 0, 510, 510)));
        assert_eq!(bounding_box(&[]), None);
        assert_eq!(bounding_box(&[r(3, 3, 0, 0)]), None);
    }

    #[test]
    fn merged_rectangles_always_cover_what_was_named() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        for _ in 0..300 {
            let n = 1 + rng.below(64) as usize;
            let rects: Vec<Rect> = (0..n)
                .map(|_| {
                    r(
                        rng.below(1900) as u32,
                        rng.below(1000) as u32,
                        rng.below(300) as u32,
                        rng.below(300) as u32,
                    )
                })
                .collect();
            let max = 1 + rng.below(6) as usize;
            let merged = merge_rects(&rects, max);
            let named: Vec<&Rect> = rects.iter().filter(|r| area(r) != 0).collect();
            assert!(merged.len() <= max);
            assert_eq!(merged.is_empty(), named.is_empty());
            for want in named {
                assert!(
                    merged.iter().any(|m| m.x <= want.x
                        && m.y <= want.y
                        && right(m) >= right(want)
                        && bottom(m) >= bottom(want)),
                    "{want:?} is not inside any of {merged:?}"
                );
            }
            // Nothing outside the bounding box of what was named.
            if let Some(b) = bounding_box(&rects) {
                for m in &merged {
                    assert!(
                        m.x >= b.x
                            && m.y >= b.y
                            && right(m) <= right(&b)
                            && bottom(m) <= bottom(&b)
                    );
                }
            }
        }
        // Rectangles too big to add up don't panic.
        let huge = [r(u32::MAX, u32::MAX, u32::MAX, u32::MAX), r(0, 0, u32::MAX, u32::MAX)];
        assert_eq!(merge_rects(&huge, 1).len(), 1);
    }

    #[test]
    fn a_screen_keeps_its_picture_when_its_buffer_is_destroyed() {
        let mut s = Shown::new();
        // The first present sets the scanout; a second of the same buffer does not.
        let p = s.present(1);
        assert_eq!((p.set_scanout, p.free), (true, None));
        let p2 = s.present(1);
        assert_eq!((p2.set_scanout, p2.free), (false, None));
        assert!(!s.is_latest(p.number) && s.is_latest(p2.number));
        // Another buffer that was never shown is freed at once; the shown one is kept.
        assert_eq!(s.destroy(2), Dispose::Free);
        assert_eq!(s.destroy(1), Dispose::Detach);
        assert_eq!((s.shown(), s.leftover()), (Some(1), Some(1)));
        // Presenting another buffer frees the leftover, after setting the scanout.
        let p3 = s.present(3);
        assert_eq!((p3.set_scanout, p3.free), (true, Some(1)));
        assert_eq!((s.shown(), s.leftover()), (Some(3), None));
        // Presenting the first buffer's resource again would need no leftover to free.
        let p4 = s.present(4);
        assert_eq!((p4.set_scanout, p4.free), (true, None));
    }

    #[test]
    fn turning_a_screen_off_frees_the_leftover() {
        let mut s = Shown::new();
        s.present(7);
        assert_eq!(s.destroy(7), Dispose::Detach);
        assert_eq!(s.clear(), Some(7));
        assert_eq!((s.shown(), s.leftover()), (None, None));
        // After that a buffer is freed whole, and the next present sets the scanout again.
        assert_eq!(s.destroy(8), Dispose::Free);
        assert!(s.present(9).set_scanout);
        // Clearing a scanout with a live buffer on it leaves the buffer alone.
        assert_eq!(s.clear(), None);
        assert_eq!(s.destroy(9), Dispose::Free);
        assert_eq!(s.clear(), None);
    }

    #[test]
    fn only_the_newest_present_is_reported() {
        let mut s = Shown::new();
        let old = s.present(1);
        // The buffer was destroyed and a new client presented on the same screen meanwhile.
        s.destroy(1);
        let new = s.present(2);
        assert!(!s.is_latest(old.number), "the old one finishing late says nothing");
        assert!(s.is_latest(new.number));
        // With no newer present, an old one is reported, so the screen does not stay busy.
        let mut t = Shown::new();
        let p = t.present(1);
        t.destroy(1);
        assert!(t.is_latest(p.number));
    }
}
