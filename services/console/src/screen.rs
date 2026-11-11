// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The screen: a buffer from the display driver, mapped into this program, the drawing of
//! text rows in it, and showing it.
//!
//! The display driver owns the screen (note 0020). This program *takes* the display, which
//! makes it the one client whose pictures are shown (and makes the driver take the screen away
//! from the kernel's own text console, note 0010, section 7). It then asks for a *buffer*:
//! memory the size of the screen, 32 bits a pixel, which it maps and draws in. Nothing appears
//! until it *presents* the buffer, naming the rectangles that changed; the driver copies those
//! to the screen and signals a notification when the picture is there. Until that signal the
//! buffer must be left alone, so [`Screen::show`] waits for it before returning.
//!
//! Which pixels to draw is worked out by [`pbr_terminal::render::Layout`], which is tested on
//! the host; this file only copies them into the buffer, which is the one place that needs
//! `unsafe`.

use alloc::vec;
use alloc::vec::Vec;
use granite::memory::{Mapping, Memory};
use granite::notification::Notification;
use granite::time;
use pbr_interfaces::pbr_display::{self as display, Rect};
use pbr_terminal::render::{BACKGROUND, Layout};

/// How long to wait for the driver to say a picture is on the screen. The drivers take a few
/// milliseconds; one that takes this long has something wrong with it, and the console must
/// not stop for it.
const DONE_MS: u64 = 2_000;
/// The most rectangles a present can name (`pbr.display`). With more than that changed, the
/// whole buffer is presented.
const MAX_RECTS: usize = 64;

/// What the display driver gave this program to draw on one screen with.
pub struct Taken {
    /// The capability that presents: only requests through it are obeyed.
    pub taker: display::Client,
    /// Signalled by the driver: bit *n* when screen *n*'s picture is on the screen.
    pub done: Notification,
    /// Which of the device's screens.
    pub screen: u32,
    /// The buffer's number.
    pub buffer: u32,
    /// The buffer's memory. The mapping does not need it, but it is kept beside it.
    pub _memory: Memory,
}

/// A screen that can be drawn on.
pub struct Screen {
    layout: Layout,
    /// The buffer, mapped.
    map: Mapping,
    taken: Taken,
    /// One line of pixels of a row of text, made and then copied into the buffer.
    line: Vec<u32>,
    /// The parts of the buffer drawn in since it was last shown.
    changed: Vec<Rect>,
    /// More changed than `changed` can list: show everything.
    everything: bool,
}

impl Screen {
    /// Takes charge of a mapped buffer, which must hold at least `layout.bytes()` bytes,
    /// paints it the background colour and shows it. `None` if the mapping is too small (the
    /// display driver's description of the buffer is not to be trusted with writes).
    pub fn new(layout: Layout, map: Mapping, taken: Taken) -> Option<Screen> {
        if map.len() < layout.bytes() {
            return None;
        }
        let mut screen = Screen {
            layout,
            map,
            taken,
            line: vec![0; layout.text_width()],
            changed: Vec::new(),
            everything: true,
        };
        screen.paint_background();
        screen.show();
        Some(screen)
    }

    /// How text is laid out on this screen.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn paint_background(&mut self) {
        let row = vec![self.layout.pixel(BACKGROUND); self.layout.width];
        for y in 0..self.layout.height {
            self.put(0, y, &row);
        }
        self.everything = true;
    }

    /// Draws row `row` of the grid, whose characters are `chars`, with the cursor in column
    /// `caret` if there is one. A row off the screen is not drawn.
    pub fn draw_row(&mut self, row: usize, chars: &[char], caret: Option<usize>) {
        if row >= self.layout.rows {
            return;
        }
        let (x, y) = self.layout.origin(row);
        for line in 0..self.layout.cell_height {
            self.layout.render_line(chars, caret, line, &mut self.line);
            self.put(x, y + line, &self.line);
        }
        self.mark(y, self.layout.cell_height);
    }

    /// Notes that the `height` lines of pixels from line `y` down were drawn in.
    fn mark(&mut self, y: usize, height: usize) {
        if self.everything {
            return;
        }
        let (y, height) = (y as u32, height as u32);
        // Rows of text are mostly drawn one after the other: a row straight below the last
        // rectangle just makes that one taller.
        if let Some(last) = self.changed.last_mut()
            && last.y + last.height == y
        {
            last.height += height;
        } else if self.changed.len() < MAX_RECTS {
            self.changed.push(Rect { x: 0, y, width: self.layout.width as u32, height });
        } else {
            self.everything = true;
        }
    }

    /// Shows what was drawn since the last time, and waits until it is on the screen (or for
    /// two seconds). If the driver has gone, or another program has taken the display, nothing
    /// is shown; the watcher (`devices.rs`) notices a driver that has gone and finds the
    /// screen again, and everything is then drawn afresh.
    pub fn show(&mut self) {
        if !self.everything && self.changed.is_empty() {
            return;
        }
        // An empty list means the whole buffer.
        let changed = if self.everything { &[][..] } else { &self.changed[..] };
        if self.taken.taker.present(self.taken.buffer, changed).is_ok() {
            // (`devices.rs` only accepts screens 0 to 15.)
            let bit = 1u64 << (self.taken.screen & 15);
            let end = time::after_ms(DONE_MS);
            // The notification carries other bits too (bit 63: the screens changed), so wait
            // until this screen's own bit has come.
            while let Ok(bits) = self.taken.done.wait(end) {
                if bits & bit != 0 {
                    break;
                }
            }
            self.changed.clear();
            self.everything = false;
        } else {
            // Nothing was shown, and next time the list may not be all of it.
            self.everything = true;
        }
    }

    /// Copies `pixels` into the buffer at (`x`, `y`). Pixels that would fall off the
    /// right or the bottom are left out, so a wrong position can only draw in the wrong place
    /// on the screen, never outside its memory.
    fn put(&self, x: usize, y: usize, pixels: &[u32]) {
        if y >= self.layout.height || x >= self.layout.width {
            return;
        }
        let count = pixels.len().min(self.layout.width - x);
        // In bytes: a row's start (rows are `stride` apart), then the pixel's place in the row.
        let offset = y * self.layout.stride + x * 4;
        // The whole of the last row `put` can reach is inside `layout.bytes()`, which `new`
        // checked fits in the mapping. (This check is only there so the arithmetic above can
        // never be wrong silently.)
        if (offset + count * 4) as u64 > self.layout.bytes() {
            return;
        }
        // SAFETY: `offset + count * 4` is within the mapping (checked just above, and `new`
        // checked that the layout's bytes fit in it), and the mapping lives as long as `self`.
        // Both the mapping's start (a page) and `offset` (a multiple of 4: `Layout::new` only
        // accepts a stride that is) are aligned for 32-bit writes. Volatile writes are used
        // because the display driver reads this memory from another program: the compiler
        // must not drop the writes as if nobody looked at them.
        unsafe {
            let at = self.map.as_mut_ptr().add(offset).cast::<u32>();
            for (i, pixel) in pixels[..count].iter().enumerate() {
                at.add(i).write_volatile(*pixel);
            }
        }
    }
}
