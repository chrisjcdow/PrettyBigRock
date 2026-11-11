// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The rules every display driver keeps for `pbr.display`
//! (docs/design/0020-screens-and-virtio-gpu.md, sections 1 and 2), and the pixel conversions
//! the framebuffer driver needs (section 3). Kept here, apart from the drivers, so that they
//! are tested on the host: who may present, which buffers exist, whether a screen is still
//! busy with its last picture, and whether the rectangles a client names are really inside its
//! buffer. A driver trusts none of what a client sends until [`State`] has passed it.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// The most screens one device can have (bit *n* of the done notification is screen *n*).
pub const MAX_SCREENS: u32 = 16;
/// The most rectangles one present can name.
pub const MAX_RECTS: usize = 64;
/// The most buffers a taker can have at once, whatever the driver's memory allows.
pub const MAX_BUFFERS: usize = 64;
/// The badge every client's ordinary capability carries (`pbr_driver::CLIENT_BADGE`). Takers
/// get badges above it.
pub const CLIENT_BADGE: u64 = 1;

/// A rectangle of pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    /// Left edge.
    pub x: u32,
    /// Top edge.
    pub y: u32,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

/// Why a request is refused. Each is one of `pbr.display`'s errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// There is no such screen.
    NoScreen,
    /// No room for another buffer.
    NoMemory,
    /// There is no such buffer.
    NoSuchBuffer,
    /// Not asked through the current taker's capability.
    NotTaken,
    /// The screen's previous present isn't done.
    Busy,
    /// A rectangle that is empty or not inside the buffer, or too many of them.
    BadArgument,
}

/// A buffer the taker has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Buffer {
    /// The screen it was made for.
    pub screen: u32,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

/// What a display driver remembers about its client.
#[derive(Debug)]
pub struct State {
    /// The badge of the current taker's capability, or 0 if nobody has taken the screens.
    taker: u64,
    /// The badge the next taker gets. They are never used twice, so a request through an old
    /// taker's capability can't be mistaken for the new one's.
    next_badge: u64,
    buffers: BTreeMap<u32, Buffer>,
    /// The number the next buffer gets. Never used twice either, for the same reason.
    next_buffer: u32,
    /// Bit *n*: screen *n* has a present that isn't done.
    busy: u32,
    /// For each busy screen, the buffer being presented on it.
    presenting: [u32; MAX_SCREENS as usize],
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    /// Nobody has taken the screens.
    pub fn new() -> State {
        State {
            taker: 0,
            next_badge: CLIENT_BADGE + 1,
            buffers: BTreeMap::new(),
            next_buffer: 1,
            busy: 0,
            presenting: [0; MAX_SCREENS as usize],
        }
    }

    /// Someone takes the screens. Returns the badge for their capability, and the previous
    /// taker's buffers, which the driver must now destroy. No screen is busy for the new taker.
    pub fn take(&mut self) -> (u64, Vec<u32>) {
        let old = self.clear();
        self.taker = self.next_badge;
        self.next_badge += 1;
        (self.taker, old)
    }

    /// The capability with `badge` has gone. If it was the taker's, nobody has the screens
    /// now, and its buffers are returned for the driver to destroy.
    pub fn gone(&mut self, badge: u64) -> Vec<u32> {
        if badge != 0 && badge == self.taker {
            self.taker = 0;
            self.clear()
        } else {
            Vec::new()
        }
    }

    fn clear(&mut self) -> Vec<u32> {
        self.busy = 0;
        let old = self.buffers.keys().copied().collect();
        self.buffers.clear();
        old
    }

    /// Whether a request that came through a capability with `badge` is the taker's.
    pub fn check(&self, badge: u64) -> Result<(), Refusal> {
        if self.taker != 0 && badge == self.taker { Ok(()) } else { Err(Refusal::NotTaken) }
    }

    /// Records a new buffer for `screen`, `width` by `height` pixels, and returns its number.
    pub fn add(&mut self, screen: u32, width: u32, height: u32) -> Result<u32, Refusal> {
        if screen >= MAX_SCREENS {
            return Err(Refusal::NoScreen);
        }
        if self.buffers.len() >= MAX_BUFFERS || self.next_buffer == u32::MAX {
            return Err(Refusal::NoMemory);
        }
        let id = self.next_buffer;
        self.next_buffer += 1;
        self.buffers.insert(id, Buffer { screen, width, height });
        Ok(id)
    }

    /// Forgets buffer `id`. Refused with `Busy` while the buffer is being presented: the
    /// device is still reading it, and the screen's present couldn't be finished without it.
    pub fn remove(&mut self, id: u32) -> Result<Buffer, Refusal> {
        let b = *self.buffers.get(&id).ok_or(Refusal::NoSuchBuffer)?;
        if self.is_busy(b.screen) && self.presenting[b.screen as usize] == id {
            return Err(Refusal::Busy);
        }
        self.buffers.remove(&id);
        Ok(b)
    }

    /// Forgets every buffer of `screen` (its mode changed, so they are the wrong size) and
    /// returns their numbers for the driver to destroy.
    pub fn remove_screen(&mut self, screen: u32) -> Vec<u32> {
        let gone: Vec<u32> =
            self.buffers.iter().filter(|(_, b)| b.screen == screen).map(|(id, _)| *id).collect();
        for id in &gone {
            self.buffers.remove(id);
        }
        gone
    }

    /// Checks a present of buffer `id` with the rectangles `changed`, and marks its screen
    /// busy. Returns the buffer and the rectangles to put on the screen: the client's, or the
    /// whole buffer if it named none.
    pub fn present(&mut self, id: u32, changed: &[Rect]) -> Result<(Buffer, Vec<Rect>), Refusal> {
        let b = *self.buffers.get(&id).ok_or(Refusal::NoSuchBuffer)?;
        if self.busy & (1 << b.screen) != 0 {
            return Err(Refusal::Busy);
        }
        if changed.len() > MAX_RECTS {
            return Err(Refusal::BadArgument);
        }
        // Every rectangle must be inside the buffer and not empty. `checked_add` gives `None`
        // instead of wrapping round if the numbers are too big to add.
        let inside = |r: &Rect| {
            r.width != 0
                && r.height != 0
                && r.x.checked_add(r.width).is_some_and(|right| right <= b.width)
                && r.y.checked_add(r.height).is_some_and(|bottom| bottom <= b.height)
        };
        if !changed.iter().all(inside) {
            return Err(Refusal::BadArgument);
        }
        let rects = if changed.is_empty() {
            alloc::vec![Rect { x: 0, y: 0, width: b.width, height: b.height }]
        } else {
            changed.to_vec()
        };
        self.busy |= 1 << b.screen;
        self.presenting[b.screen as usize] = id;
        Ok((b, rects))
    }

    /// Whether `screen` has a present that isn't done.
    pub fn is_busy(&self, screen: u32) -> bool {
        screen < MAX_SCREENS && self.busy & (1 << screen) != 0
    }

    /// The present on `screen` is done. Returns whether one was outstanding (if not, there is
    /// nobody to tell: the taker changed meanwhile).
    pub fn done(&mut self, screen: u32) -> bool {
        if screen >= MAX_SCREENS || self.busy & (1 << screen) == 0 {
            return false;
        }
        self.busy &= !(1 << screen);
        true
    }
}

/// How a screen's own memory holds a pixel, lowest byte first. The numbers are the device
/// record's (`record::Screen::format`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Blue, green, red, one unused byte: what buffers hold too.
    Bgrx = 1,
    /// Red, green, blue, one unused byte.
    Rgbx = 2,
    /// Blue, green, red: three bytes a pixel.
    Bgr = 3,
    /// Red, green, blue: three bytes a pixel.
    Rgb = 4,
}

impl Format {
    /// From the device record's number.
    pub fn from_record(n: u32) -> Option<Format> {
        match n {
            1 => Some(Format::Bgrx),
            2 => Some(Format::Rgbx),
            3 => Some(Format::Bgr),
            4 => Some(Format::Rgb),
            _ => None,
        }
    }

    /// Bytes a pixel takes on the screen.
    pub fn bytes(self) -> usize {
        match self {
            Format::Bgrx | Format::Rgbx => 4,
            Format::Bgr | Format::Rgb => 3,
        }
    }
}

/// Converts a row of buffer pixels (`src`: four bytes each, blue first) into the screen's
/// format in `out`, and returns how many bytes of `out` were written. Converts as many whole
/// pixels as fit in both.
pub fn convert_row(src: &[u8], format: Format, out: &mut [u8]) -> usize {
    let size = format.bytes();
    let pixels = (src.len() / 4).min(out.len() / size);
    // `chunks_exact` walks a slice in pieces of exactly that many bytes; `zip` pairs each
    // source pixel with its place in the output.
    for (s, o) in src.chunks_exact(4).zip(out.chunks_exact_mut(size)).take(pixels) {
        let (b, g, r) = (s[0], s[1], s[2]);
        match format {
            Format::Bgrx => o.copy_from_slice(&[b, g, r, 0]),
            Format::Rgbx => o.copy_from_slice(&[r, g, b, 0]),
            Format::Bgr => o.copy_from_slice(&[b, g, r]),
            Format::Rgb => o.copy_from_slice(&[r, g, b]),
        }
    }
    pixels * size
}

/// Where a screen's pixels are in its memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
    /// Bytes from the start of one row to the start of the next.
    pub stride: u32,
    /// How each pixel is held.
    pub format: Format,
}

impl Geometry {
    /// The bytes the picture takes, from the first pixel to the last, or `None` if the numbers
    /// make no sense: no pixels, or rows that overlap. (The last row counts only its pixels,
    /// not a whole stride: firmware is allowed to give exactly that much memory.)
    pub fn bytes(&self) -> Option<u64> {
        let row = u64::from(self.width) * self.format.bytes() as u64;
        if self.width == 0 || self.height == 0 || u64::from(self.stride) < row {
            return None;
        }
        Some(u64::from(self.height - 1) * u64::from(self.stride) + row)
    }
}

/// One row of a rectangle to copy: `pixels` pixels, from byte `from` of the buffer to byte
/// `to` of the screen's memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// Where the row's first pixel is in the buffer.
    pub from: usize,
    /// Where it goes in the screen's memory.
    pub to: usize,
    /// How many pixels.
    pub pixels: usize,
}

/// The rows to copy to put rectangle `r` of a buffer on a screen of the same size. The buffer
/// is `Bgrx` with rows `buffer_stride` bytes apart in `buffer_len` bytes of memory; the screen
/// is `screen`, in `screen_len` bytes of memory. `None` unless every byte that would be read
/// is inside the buffer's memory and every byte written inside the screen's: this is the check
/// that the framebuffer driver's copying relies on.
pub fn spans(
    buffer_stride: u32,
    buffer_len: u64,
    screen: Geometry,
    screen_len: u64,
    r: Rect,
) -> Option<impl Iterator<Item = Span>> {
    let buffer = Geometry { stride: buffer_stride, format: Format::Bgrx, ..screen };
    if buffer.bytes()? > buffer_len || screen.bytes()? > screen_len {
        return None;
    }
    // Both pictures fit in memory that really exists, so they fit in a `usize` too, and so
    // does every offset inside them.
    usize::try_from(buffer_len.max(screen_len)).ok()?;
    if r.width == 0
        || r.height == 0
        || r.x.checked_add(r.width)? > screen.width
        || r.y.checked_add(r.height)? > screen.height
    {
        return None;
    }
    let size = screen.format.bytes();
    let (x, width) = (r.x as usize, r.width as usize);
    let (from_stride, to_stride) = (buffer_stride as usize, screen.stride as usize);
    Some((r.y as usize..(r.y + r.height) as usize).map(move |y| Span {
        from: y * from_stride + x * 4,
        to: y * to_stride + x * size,
        pixels: width,
    }))
}

/// The pictures the device self-test shows, which the test runner then looks for in a
/// screenshot of each screen (note 0020, section 7). Kept here so that the program that draws
/// them and the runner that checks them use the same code.
pub mod test {
    use super::Rect;

    /// A pixel of pattern `which` on a screen of `width` by `height` pixels, as a buffer
    /// holds it (red in bits 16 to 23, green in 8 to 15, blue in 0 to 7): bands of eight
    /// colours that step sideways as they go down, so that a picture shown with the wrong
    /// row length, upside down or with its colours swapped doesn't match, and a square of
    /// its own colour in each corner, so that one cut short or shifted doesn't either. Two
    /// patterns with different `which` differ at every pixel.
    pub fn pattern(which: u32, width: u32, height: u32, x: u32, y: u32) -> u32 {
        const BANDS: [u32; 8] = [
            0x20_2020, 0xC0_0000, 0x00_C000, 0x00_00C0, 0xC0_C000, 0x00_C0C0, 0xC0_00C0, 0xE0_E0E0,
        ];
        const CORNERS: [u32; 4] = [0xFF_0000, 0x00_FF00, 0x00_00FF, 0xFF_FFFF];
        let which = which % 4;
        let side = 32.min(width / 4).min(height / 4).max(1);
        let (left, right) = (x < side, x >= width.saturating_sub(side));
        let (top, bottom) = (y < side, y >= height.saturating_sub(side));
        let corner = match (top, bottom, left, right) {
            (true, _, true, _) => Some(0),
            (true, _, _, true) => Some(1),
            (_, true, true, _) => Some(2),
            (_, true, _, true) => Some(3),
            _ => None,
        };
        match corner {
            Some(c) => CORNERS[((c + which) % 4) as usize],
            None => BANDS[((x / 16 + y / 48 + which * 3) % 8) as usize],
        }
    }

    /// The rectangle that picture 2 changes: about the middle of the screen, away from every
    /// edge.
    pub fn changed(width: u32, height: u32) -> Rect {
        Rect { x: width / 4, y: height / 4, width: (width / 2).max(1), height: (height / 3).max(1) }
    }

    /// A pixel of picture `number`, as the screen should show it:
    /// - picture 1 is pattern 1;
    /// - picture 2 is what is on the screen after a buffer redrawn all over with pattern 2
    ///   was presented with only [`changed`] named: pattern 2 inside it, pattern 1 outside;
    /// - any other picture is pattern 3.
    pub fn picture(number: u32, width: u32, height: u32, x: u32, y: u32) -> u32 {
        let r = changed(width, height);
        let inside = x >= r.x && x - r.x < r.width && y >= r.y && y - r.y < r.height;
        match number {
            1 => pattern(1, width, height, x, y),
            2 if inside => pattern(2, width, height, x, y),
            2 => pattern(1, width, height, x, y),
            _ => pattern(3, width, height, x, y),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Rng;

    const WHOLE: Rect = Rect { x: 0, y: 0, width: 640, height: 480 };

    #[test]
    fn only_the_taker_is_obeyed() {
        let mut s = State::new();
        // Before anyone takes: nobody is the taker, whatever badge they show.
        for badge in [0, CLIENT_BADGE, 2, u64::MAX] {
            assert_eq!(s.check(badge), Err(Refusal::NotTaken));
        }
        let (first, old) = s.take();
        assert!(first > CLIENT_BADGE && old.is_empty());
        assert_eq!(s.check(first), Ok(()));
        assert_eq!(s.check(CLIENT_BADGE), Err(Refusal::NotTaken));
        assert_eq!(s.check(0), Err(Refusal::NotTaken));
        // A second taker displaces the first, whose buffers are handed back for destroying.
        let a = s.add(0, 640, 480).unwrap();
        let b = s.add(1, 800, 600).unwrap();
        let (second, old) = s.take();
        assert_ne!(second, first);
        assert_eq!(old, [a, b]);
        assert_eq!(s.check(first), Err(Refusal::NotTaken));
        assert_eq!(s.check(second), Ok(()));
        assert_eq!(s.remove(a), Err(Refusal::NoSuchBuffer));
    }

    #[test]
    fn a_taker_that_goes_leaves_nobody() {
        let mut s = State::new();
        let (first, _) = s.take();
        let (second, _) = s.take();
        let a = s.add(0, 640, 480).unwrap();
        // The old taker's capability going changes nothing; nor does badge 0.
        assert!(s.gone(first).is_empty());
        assert!(s.gone(0).is_empty());
        assert_eq!(s.check(second), Ok(()));
        // The current one going frees its buffers and leaves no taker.
        assert_eq!(s.gone(second), [a]);
        assert_eq!(s.check(second), Err(Refusal::NotTaken));
        assert_eq!(s.present(a, &[]), Err(Refusal::NoSuchBuffer));
        // Badges and buffer numbers are never used again.
        let (third, _) = s.take();
        assert!(third > second);
        assert!(s.add(0, 640, 480).unwrap() > a);
    }

    #[test]
    fn buffers_are_counted_and_belong_to_screens() {
        let mut s = State::new();
        s.take();
        assert_eq!(s.add(MAX_SCREENS, 1, 1), Err(Refusal::NoScreen));
        let a = s.add(0, 640, 480).unwrap();
        let b = s.add(1, 800, 600).unwrap();
        let c = s.add(0, 640, 480).unwrap();
        assert_eq!(s.remove(b), Ok(Buffer { screen: 1, width: 800, height: 600 }));
        assert_eq!(s.remove(b), Err(Refusal::NoSuchBuffer));
        // A change of mode on screen 0 takes its buffers and no others.
        let d = s.add(1, 800, 600).unwrap();
        assert_eq!(s.remove_screen(0), [a, c]);
        assert_eq!(s.remove_screen(0), []);
        assert!(s.present(d, &[]).is_ok());
        // No more than the limit at once.
        let mut s = State::new();
        s.take();
        for _ in 0..MAX_BUFFERS {
            s.add(0, 1, 1).unwrap();
        }
        assert_eq!(s.add(0, 1, 1), Err(Refusal::NoMemory));
    }

    #[test]
    fn one_picture_at_a_time_for_each_screen() {
        let mut s = State::new();
        s.take();
        let a = s.add(0, 640, 480).unwrap();
        let a2 = s.add(0, 640, 480).unwrap();
        let b = s.add(1, 640, 480).unwrap();
        // No rectangles means the whole buffer.
        assert_eq!(
            s.present(a, &[]),
            Ok((Buffer { screen: 0, width: 640, height: 480 }, [WHOLE].to_vec()))
        );
        // Screen 0 is busy for both its buffers; screen 1 isn't.
        assert_eq!(s.present(a, &[]), Err(Refusal::Busy));
        assert_eq!(s.present(a2, &[]), Err(Refusal::Busy));
        assert!(s.present(b, &[]).is_ok());
        assert!(s.is_busy(0) && s.is_busy(1) && !s.is_busy(2) && !s.is_busy(MAX_SCREENS));
        // A buffer being presented can't be destroyed; another of the same screen can.
        assert_eq!(s.remove(a), Err(Refusal::Busy));
        assert_eq!(s.remove(b), Err(Refusal::Busy));
        let spare = s.add(0, 640, 480).unwrap();
        assert!(s.remove(spare).is_ok());
        assert!(s.done(0));
        assert!(!s.is_busy(0));
        assert!(s.remove(a).is_ok());
        let a = s.add(0, 640, 480).unwrap();
        assert!(s.present(a, &[]).is_ok());
        assert!(s.done(0));
        assert!(!s.done(0), "done twice");
        assert!(!s.done(MAX_SCREENS));
        assert!(s.present(a2, &[]).is_ok());
        // A new taker doesn't inherit a busy screen, and a late done is for nobody.
        s.take();
        assert!(!s.done(0));
        let c = s.add(0, 640, 480).unwrap();
        assert!(s.present(c, &[]).is_ok());
    }

    #[test]
    fn rectangles_must_be_inside_the_buffer() {
        let mut s = State::new();
        s.take();
        let a = s.add(0, 640, 480).unwrap();
        let r = |x, y, width, height| Rect { x, y, width, height };
        let good = [r(0, 0, 1, 1), r(639, 479, 1, 1), r(100, 100, 540, 380), WHOLE];
        let (_, rects) = s.present(a, &good).unwrap();
        assert_eq!(rects, good);
        s.done(0);
        for bad in [
            r(0, 0, 0, 10),
            r(0, 0, 10, 0),
            r(640, 0, 1, 1),
            r(0, 480, 1, 1),
            r(1, 0, 640, 1),
            r(0, 1, 1, 480),
            r(u32::MAX, 0, 2, 1),
            r(0, u32::MAX, 1, 2),
            r(1, 1, u32::MAX, u32::MAX),
        ] {
            assert_eq!(s.present(a, &[good[0], bad]), Err(Refusal::BadArgument), "{bad:?}");
        }
        // A refused present doesn't leave the screen busy.
        assert!(s.present(a, &[good[0]]).is_ok());
        s.done(0);
        assert_eq!(s.present(a, &[good[0]; MAX_RECTS + 1]), Err(Refusal::BadArgument));
        assert!(s.present(a, &[good[0]; MAX_RECTS]).is_ok());
    }

    #[test]
    fn random_requests_keep_the_rules() {
        // Whatever a client asks, in any order: a present that is accepted names a buffer that
        // exists, only rectangles inside it, and a screen that wasn't busy.
        let mut rng = Rng(20);
        let mut s = State::new();
        let mut taker = s.take().0;
        let mut busy = [false; MAX_SCREENS as usize];
        for _ in 0..20_000 {
            match rng.below(8) {
                0 => {
                    taker = s.take().0;
                    busy = [false; MAX_SCREENS as usize];
                }
                1 => {
                    let _ = s.add(
                        rng.below(18) as u32,
                        1 + rng.below(64) as u32,
                        1 + rng.below(64) as u32,
                    );
                }
                2 => {
                    let _ = s.remove(rng.below(200) as u32);
                }
                3 => {
                    let screen = rng.below(18) as u32;
                    if s.done(screen) {
                        assert!(busy[screen as usize]);
                        busy[screen as usize] = false;
                    }
                }
                _ => {
                    let id = rng.below(200) as u32;
                    let rect = Rect {
                        x: rng.below(70) as u32,
                        y: rng.below(70) as u32,
                        width: rng.below(70) as u32,
                        height: rng.below(70) as u32,
                    };
                    if let Ok((b, rects)) = s.present(id, &[rect]) {
                        assert!(!busy[b.screen as usize]);
                        busy[b.screen as usize] = true;
                        for r in rects {
                            assert!(r.width > 0 && r.x + r.width <= b.width);
                            assert!(r.height > 0 && r.y + r.height <= b.height);
                        }
                    }
                }
            }
            assert_eq!(s.check(taker), Ok(()));
        }
    }

    #[test]
    fn spans_stay_inside_both_memories() {
        let r = |x, y, width, height| Rect { x, y, width, height };
        let screen = Geometry { width: 10, height: 4, stride: 32, format: Format::Bgr };
        assert_eq!(screen.bytes(), Some(3 * 32 + 30));
        // A rectangle in the middle: each row's place in the buffer (40 bytes a row, 4 a
        // pixel) and on the screen (32 bytes a row, 3 a pixel).
        let rows: Vec<Span> = spans(40, 160, screen, 126, r(2, 1, 5, 2)).unwrap().collect();
        assert_eq!(
            rows,
            [Span { from: 48, to: 38, pixels: 5 }, Span { from: 88, to: 70, pixels: 5 }]
        );
        // The whole picture, with exactly enough memory for each and not a byte more.
        assert_eq!(spans(40, 160, screen, 126, r(0, 0, 10, 4)).unwrap().count(), 4);
        assert!(spans(40, 159, screen, 126, r(0, 0, 1, 1)).is_none(), "buffer memory too small");
        assert!(spans(40, 160, screen, 125, r(0, 0, 1, 1)).is_none(), "screen memory too small");
        assert!(spans(39, 160, screen, 126, r(0, 0, 1, 1)).is_none(), "buffer rows overlap");
        // Rectangles that are empty, outside, or wrap round.
        for bad in [
            r(0, 0, 0, 1),
            r(0, 0, 1, 0),
            r(10, 0, 1, 1),
            r(0, 4, 1, 1),
            r(9, 0, 2, 1),
            r(0, 3, 1, 2),
            r(u32::MAX, 0, 2, 1),
            r(0, u32::MAX, 1, 2),
        ] {
            assert!(spans(40, 160, screen, 126, bad).is_none(), "{bad:?}");
        }
        // Screens that make no sense.
        for (width, height, stride) in
            [(0, 4, 32), (10, 0, 32), (10, 4, 29), (u32::MAX, 2, u32::MAX)]
        {
            let g = Geometry { width, height, stride, format: Format::Bgr };
            assert!(spans(40, u64::MAX, g, 126, r(0, 0, 1, 1)).is_none(), "{g:?}");
        }
    }

    #[test]
    fn random_spans_never_leave_the_memory() {
        let mut rng = Rng(21);
        let mut accepted = 0;
        for _ in 0..20_000 {
            let format = Format::from_record(1 + rng.below(4) as u32).unwrap();
            let screen = Geometry {
                width: rng.below(20) as u32,
                height: rng.below(20) as u32,
                stride: rng.below(100) as u32,
                format,
            };
            let (buffer_stride, buffer_len, screen_len) =
                (rng.below(100) as u32, rng.below(3000), rng.below(3000));
            let rect = Rect {
                x: rng.below(14) as u32,
                y: rng.below(14) as u32,
                width: rng.below(14) as u32,
                height: rng.below(14) as u32,
            };
            let Some(rows) = spans(buffer_stride, buffer_len, screen, screen_len, rect) else {
                continue;
            };
            accepted += 1;
            for s in rows {
                assert!(s.pixels > 0);
                assert!((s.from + s.pixels * 4) as u64 <= buffer_len);
                assert!((s.to + s.pixels * format.bytes()) as u64 <= screen_len);
            }
        }
        assert!(accepted > 100, "only {accepted} accepted: the test tests nothing");
    }

    #[test]
    fn test_pictures_differ_everywhere_they_should() {
        for (w, h) in [(1280, 800), (640, 480), (17, 5), (1, 1)] {
            let r = test::changed(w, h);
            assert!(r.width > 0 && r.x + r.width <= w && r.height > 0 && r.y + r.height <= h);
            for y in 0..h {
                for x in 0..w {
                    let p = |n| test::pattern(n, w, h, x, y);
                    assert!(p(1) != p(2) && p(2) != p(3) && p(1) != p(3), "{w}x{h} at ({x}, {y})");
                    assert_eq!(p(1) >> 24, 0);
                    let inside = x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
                    assert_eq!(test::picture(1, w, h, x, y), p(1));
                    assert_eq!(test::picture(2, w, h, x, y), if inside { p(2) } else { p(1) });
                    assert_eq!(test::picture(3, w, h, x, y), p(3));
                }
            }
        }
        // The four corners have four colours, and the bands step along and down.
        let p = |x, y| test::pattern(1, 640, 480, x, y);
        let corners = [p(0, 0), p(639, 0), p(0, 479), p(639, 479)];
        for (i, a) in corners.iter().enumerate() {
            assert!(corners[i + 1..].iter().all(|b| a != b));
        }
        assert_ne!(p(100, 100), p(116, 100));
        assert_ne!(p(100, 100), p(100, 148));
    }

    #[test]
    fn converts_rows_to_every_format() {
        // Two pixels: blue 1, green 2, red 3; then 4, 5, 6. The fourth byte is never copied.
        let src = [1, 2, 3, 0xEE, 4, 5, 6, 0xEE];
        let mut out = [0xAA; 8];
        assert_eq!(convert_row(&src, Format::Bgrx, &mut out), 8);
        assert_eq!(out, [1, 2, 3, 0, 4, 5, 6, 0]);
        assert_eq!(convert_row(&src, Format::Rgbx, &mut out), 8);
        assert_eq!(out, [3, 2, 1, 0, 6, 5, 4, 0]);
        let mut out = [0xAA; 8];
        assert_eq!(convert_row(&src, Format::Bgr, &mut out), 6);
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 0xAA, 0xAA]);
        assert_eq!(convert_row(&src, Format::Rgb, &mut out), 6);
        assert_eq!(out, [3, 2, 1, 6, 5, 4, 0xAA, 0xAA]);
        // Only whole pixels, and only as many as fit in both.
        let mut small = [0xAA; 5];
        assert_eq!(convert_row(&src, Format::Bgr, &mut small), 3);
        assert_eq!(small, [1, 2, 3, 0xAA, 0xAA]);
        assert_eq!(convert_row(&src[..7], Format::Bgrx, &mut out), 4);
        assert_eq!(convert_row(&[], Format::Rgb, &mut out), 0);
        assert_eq!(convert_row(&src, Format::Rgb, &mut []), 0);
        for (n, f) in [(1, Format::Bgrx), (2, Format::Rgbx), (3, Format::Bgr), (4, Format::Rgb)] {
            assert_eq!(Format::from_record(n), Some(f));
            assert_eq!(f as u32, n);
        }
        assert_eq!(Format::from_record(0), None);
        assert_eq!(Format::from_record(5), None);
    }
}
