// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The screen: the display driver's framebuffer, mapped into this program, and the drawing of
//! text rows on it.
//!
//! A *framebuffer* is the screen's own memory: whatever is written to it appears on the screen.
//! The display driver owns the device; it hands this program a capability to the memory
//! (`pbr.display.framebuffer`, note 0011, section 8), and the first time that is asked for, it
//! also takes the screen away from the kernel's text console (note 0010, section 7), so from
//! then on this program owns the whole screen.
//!
//! Which pixels to draw is worked out by [`pbr_terminal::render::Layout`], which is tested on
//! the host; this file only copies them into the memory, which is the one place that needs
//! `unsafe`. The screen is *write-combining* memory: the processor gathers writes into big
//! bursts, which is much faster for drawing, but reading it back is very slow, so nothing here
//! ever reads the screen. That is also why a scroll redraws the rows instead of moving pixels.

use alloc::vec;
use alloc::vec::Vec;
use granite::device::DeviceMemory;
use granite::memory::Mapping;
use pbr_terminal::render::{BACKGROUND, Layout};

/// A screen that can be drawn on.
pub struct Screen {
    layout: Layout,
    map: Mapping,
    /// The framebuffer's capability. The mapping does not need it, but the screen is gone when
    /// this is closed, so it is kept for as long as the mapping is used.
    _memory: DeviceMemory,
    /// One line of pixels of a row of text, made and then copied into the screen.
    line: Vec<u32>,
}

impl Screen {
    /// Takes charge of a mapped framebuffer, which must hold at least `layout.bytes()` bytes,
    /// and paints the whole screen the background colour. `None` if the mapping is too small
    /// (the display driver's description of the screen is not to be trusted with writes).
    pub fn new(layout: Layout, map: Mapping, memory: DeviceMemory) -> Option<Screen> {
        if map.len() < layout.bytes() {
            return None;
        }
        let mut screen =
            Screen { layout, map, _memory: memory, line: vec![0; layout.text_width()] };
        screen.paint_background();
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
    }

    /// Copies `pixels` into the screen's memory at (`x`, `y`). Pixels that would fall off the
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
        // because the memory is a screen, not ordinary memory: the compiler must not drop,
        // merge or reorder them.
        unsafe {
            let at = self.map.as_mut_ptr().add(offset).cast::<u32>();
            for (i, pixel) in pixels[..count].iter().enumerate() {
                at.add(i).write_volatile(*pixel);
            }
        }
    }
}
