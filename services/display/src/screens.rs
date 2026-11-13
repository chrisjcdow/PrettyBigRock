// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The displays this program has taken, a buffer for each of their screens, and putting
//! pictures on them (docs/design/0022-display-server.md, section 7; note 0020 for the
//! drivers' side).
//!
//! For each screen the driver makes a *buffer*: memory the size of the screen, mapped here,
//! which `lib/compositor` draws in. Nothing shows until the buffer is *presented*, naming the
//! rectangles that changed; the driver copies those to the screen and signals the display's
//! done notification when the picture is there. Until then the buffer must be left alone, so a
//! screen whose present isn't done is *busy*, and what changes on it meanwhile waits.
//!
//! Each screen is drawn at most once per refresh (60 times a second), and only where
//! something changed.

use crate::devices::{Taken, lost};
use alloc::vec::Vec;
use core::sync::atomic::Ordering;
use granite::memory::{Mapping, Memory};
use granite::sys::perm;
use pbr_compositor::draw::{Frames, Target, compose};
use pbr_compositor::{Desk, Output};
use pbr_draw::{Damage, Rect};
use pbr_interfaces::pbr_display::{self as display, DisplayError};

/// The shortest time between two pictures on a screen: 60 a second.
pub const REFRESH: u64 = 16_666_667;
/// Screens a display may have that this program will use (done bits 0 to 62).
const MAX_SCREEN: u32 = 62;
/// After this long a present that hasn't said it is done is taken as done.
const STUCK: u64 = 2_000_000_000;
/// The done notification's bit for "the screens changed".
const SCREENS_CHANGED: u64 = 1 << 63;

/// What is waiting to be drawn on a screen.
#[derive(Default)]
struct Pending {
    all: bool,
    rects: Damage,
}

/// One screen, with the buffer it is drawn in.
struct ScreenBuf {
    index: u32,
    width: u32,
    height: u32,
    /// Pixels from the start of one row to the next.
    stride: u32,
    buffer: u32,
    _memory: Memory,
    map: Mapping,
    /// Its last present isn't done yet.
    busy: bool,
    pending: Pending,
    /// When it was last presented.
    last: u64,
    /// Memory kept between compositions of a turned or mirroring screen.
    scratch: Vec<u32>,
}

/// One display device, taken.
pub struct Display {
    taken: Taken,
    screens: Vec<ScreenBuf>,
}

impl Display {
    /// The device manager's number for it.
    pub fn device(&self) -> u32 {
        self.taken.device
    }
}

impl Drop for Display {
    fn drop(&mut self) {
        // Its done thread can end.
        self.taken.stop.store(true, Ordering::Release);
    }
}

/// Every display taken.
#[derive(Default)]
pub struct Displays {
    list: Vec<Display>,
}

impl Displays {
    /// A display was taken: sets its screens up.
    pub fn add(&mut self, taken: Taken) -> Result<(), DisplayError> {
        let mut d = Display { taken, screens: Vec::new() };
        refresh(&mut d)?;
        self.list.retain(|o| o.device() != d.device());
        self.list.push(d);
        Ok(())
    }

    /// Every screen of every display, as the compositor wants them.
    pub fn outputs(&self) -> Vec<Output> {
        let mut out = Vec::new();
        for d in &self.list {
            let infos = d.taken.client.screens().ok();
            for s in &d.screens {
                let info =
                    infos.as_ref().and_then(|i| i.screens.iter().find(|x| x.screen == s.index));
                out.push(Output {
                    device: d.device(),
                    index: s.index,
                    connector: info.map(|i| i.connector.clone()).unwrap_or_default(),
                    identity: info.map(|i| i.identity.clone()).unwrap_or_default(),
                    width: s.width,
                    height: s.height,
                    width_mm: info.map_or(0, |i| i.width_mm),
                    height_mm: info.map_or(0, |i| i.height_mm),
                });
            }
        }
        out
    }

    /// A display's driver signalled done: its screens with those bits are free again, and if
    /// its screens changed they are set up afresh. Returns whether the screens changed (or the
    /// display went), so the caller tells the compositor.
    pub fn done(&mut self, device: u32, bits: u64) -> bool {
        let Some(i) = self.list.iter().position(|d| d.device() == device) else { return false };
        for s in &mut self.list[i].screens {
            if bits & (1u64 << s.index) != 0 {
                s.busy = false;
            }
        }
        if bits & SCREENS_CHANGED == 0 {
            return false;
        }
        if refresh(&mut self.list[i]).is_err() {
            // It has gone, or can't be used: let go of it, and the watcher finds it again.
            self.list.remove(i);
        }
        true
    }

    /// Takes away a display whose driver has gone. Returns which, so the watcher looks again.
    pub fn drop_lost(&mut self) -> Vec<u32> {
        let mut gone = Vec::new();
        self.list.retain(|d| {
            let alive = !matches!(d.taken.client.screens(), Err(ref e) if lost(e));
            if !alive {
                gone.push(d.device());
            }
            alive
        });
        gone
    }

    /// Adds damage (rectangles in the space, or all of it) to every screen.
    pub fn damage(&mut self, rects: Option<&[Rect]>) {
        for s in self.list.iter_mut().flat_map(|d| d.screens.iter_mut()) {
            match rects {
                None => s.pending.all = true,
                Some(r) => {
                    for r in r {
                        s.pending.rects.add(*r);
                    }
                }
            }
        }
    }

    /// Whether some screen has something to draw or a picture on its way: the part of the
    /// space `r` on such a screen isn't up to date yet (frame events wait for it).
    pub fn busy_over(&self, desk: &Desk, r: Rect) -> bool {
        desk.space().screens().iter().any(|s| {
            let Some(buf) = self.find(s.output.device, s.output.index) else { return false };
            let busy = buf.busy || buf.pending.all || !buf.pending.rects.is_empty();
            busy && desk.space().shows(s.number).is_some_and(|m| !m.rect().intersect(&r).is_empty())
        })
    }

    fn find(&self, device: u32, index: u32) -> Option<&ScreenBuf> {
        self.list
            .iter()
            .find(|d| d.device() == device)
            .and_then(|d| d.screens.iter().find(|s| s.index == index))
    }

    /// When a screen will next be ready to draw what is waiting for it, if any is waiting.
    pub fn next_draw(&self) -> Option<u64> {
        self.list
            .iter()
            .flat_map(|d| d.screens.iter())
            .filter(|s| !s.busy && (s.pending.all || !s.pending.rects.is_empty()))
            .map(|s| s.last.saturating_add(REFRESH))
            .min()
    }

    /// Draws and presents every screen that has something waiting and is ready. Returns
    /// whether any display's driver turned out to have gone.
    pub fn draw(&mut self, desk: &Desk, frames: &dyn Frames, now: u64) -> bool {
        let scene = desk.scene();
        let mut trouble = false;
        for d in &mut self.list {
            for s in &mut d.screens {
                let waiting = s.pending.all || !s.pending.rects.is_empty();
                if s.busy || !waiting || now < s.last.saturating_add(REFRESH) {
                    continue;
                }
                let Some(number) = desk
                    .space()
                    .screens()
                    .iter()
                    .find(|x| (x.output.device, x.output.index) == (d.taken.device, s.index))
                    .map(|x| x.number)
                else {
                    continue;
                };
                let areas: Option<Vec<Rect>> =
                    if s.pending.all { None } else { Some(s.pending.rects.rects().to_vec()) };
                let pixels = (u64::from(s.stride) * u64::from(s.height)) as usize;
                if (s.map.len() as usize) < pixels * 4 {
                    continue;
                }
                // SAFETY: the mapping is `stride * height` 32-bit pixels at least (checked just
                // above), page aligned, writable, and this program's alone while the screen
                // isn't busy: the driver reads it only between a present and its done signal.
                // The slice ends with this block, before anything is presented.
                let target = unsafe {
                    core::slice::from_raw_parts_mut(s.map.as_mut_ptr().cast::<u32>(), pixels)
                };
                let changed = compose(
                    &scene,
                    desk.space(),
                    number,
                    Target { pixels: target, width: s.width, height: s.height, stride: s.stride },
                    areas.as_deref(),
                    frames,
                    &mut s.scratch,
                );
                s.pending = Pending::default();
                s.last = now;
                if changed.is_empty() {
                    continue;
                }
                let rects: Vec<display::Rect> = changed
                    .iter()
                    .filter(|r| !r.is_empty())
                    .take(64)
                    .map(|r| display::Rect {
                        x: r.x.max(0) as u32,
                        y: r.y.max(0) as u32,
                        width: r.width as u32,
                        height: r.height as u32,
                    })
                    .collect();
                match d.taken.taker.present(s.buffer, &rects) {
                    Ok(()) => s.busy = true,
                    Err(ref e) if lost(e) => trouble = true,
                    // Busy (should not happen) or a refusal: draw all of it next time.
                    Err(_) => s.pending.all = true,
                }
            }
        }
        trouble
    }

    /// Forgets that a screen is busy if its present has taken far too long (a driver that
    /// lost a signal mustn't freeze the screen for good).
    pub fn unstick(&mut self, now: u64) {
        for s in self.list.iter_mut().flat_map(|d| d.screens.iter_mut()) {
            if s.busy && now.saturating_sub(s.last) > STUCK {
                s.busy = false;
                s.pending.all = true;
            }
        }
    }
}

/// Asks a display for its screens and gives each connected one a buffer of its size: keeps
/// the ones it has, makes new ones for screens that came or changed size, and destroys the
/// ones of screens that went.
fn refresh(d: &mut Display) -> Result<(), DisplayError> {
    let listed = d.taken.client.screens().map_err(|_| DisplayError::NoScreen)?;
    let mut keep = Vec::new();
    for info in listed.screens.iter().filter(|s| s.connected && s.screen <= MAX_SCREEN) {
        // A screen plugged in but switched off shows the mode it likes best.
        let mut mode = info.current.clone();
        if mode.width == 0 || mode.height == 0 {
            let Some(first) = info.modes.first() else { continue };
            if d.taken.taker.set_mode(info.screen, first).is_err() {
                continue;
            }
            mode = first.clone();
        }
        if let Some(i) = d.screens.iter().position(|s| s.index == info.screen) {
            if (d.screens[i].width, d.screens[i].height) == (mode.width, mode.height) {
                keep.push(d.screens.remove(i));
                continue;
            }
            let old = d.screens.remove(i);
            let _ = d.taken.taker.destroy_buffer(old.buffer);
        }
        let Ok(made) = d.taken.taker.create_buffer(info.screen) else { continue };
        if made.stride % 4 != 0 || made.stride / 4 < mode.width {
            let _ = d.taken.taker.destroy_buffer(made.buffer);
            continue;
        }
        let Ok(map) = made.memory.map(perm::READ_WRITE) else {
            let _ = d.taken.taker.destroy_buffer(made.buffer);
            continue;
        };
        keep.push(ScreenBuf {
            index: info.screen,
            width: mode.width,
            height: mode.height,
            stride: made.stride / 4,
            buffer: made.buffer,
            _memory: made.memory,
            map,
            busy: false,
            pending: Pending { all: true, rects: Damage::new() },
            last: 0,
            scratch: Vec::new(),
        });
    }
    // Screens that went: their buffers too.
    for old in d.screens.drain(..) {
        let _ = d.taken.taker.destroy_buffer(old.buffer);
    }
    d.screens = keep;
    Ok(())
}
