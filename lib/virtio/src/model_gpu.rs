// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A software virtio GPU for the host tests, the 2D part of QEMU's `virtio-gpu` as far as the
//! driver uses it. It rides on the block model's registers and queue handling (`model.rs`); this
//! file is what the device does with a command: scanouts that have sizes and may be enabled,
//! resources with pixels, backing memory the device reads when told to transfer, scanouts that
//! show a picture when told to flush, fences, EDIDs and the display event.
//!
//! Like the block model, it can lie. [`GpuLie`] is what is wrong with its responses; the lies
//! about the queue itself (a used length that is too big, a completion for a command that was
//! never sent) are the block model's [`crate::model::Lie`] and work the same way.

use crate::gpu::{FORMAT_B8G8R8X8_UNORM, error_code};
use pbr_devices::regs::{Heap, Registers};
use std::collections::BTreeMap;
use std::rc::Rc;

/// What the device does wrong in its responses.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GpuLie {
    #[default]
    None,
    /// Writes only 20 bytes, short of the header.
    ShortResponse,
    /// Answers with a success type that is for another command.
    WrongType,
    /// Answers with a type that is neither a success nor one of the errors.
    UnknownType,
    /// Leaves the fence out of a response to a fenced command.
    NoFence,
    /// Repeats the wrong fence.
    WrongFence,
    /// Says it answered without writing anything.
    Silent,
    /// Sends display information for fewer scanouts than it has.
    ShortInfo,
    /// Gives an EDID size bigger than the EDID can be.
    HugeEdid,
    /// Gives an EDID size bigger than what it wrote.
    EdidPastResponse,
}

/// A picture the device was told to show.
#[derive(Clone, Debug, Default)]
pub struct ModelScanout {
    /// The size the device reports (and prefers).
    pub width: u32,
    pub height: u32,
    /// Something is plugged in.
    pub enabled: bool,
    /// Its EDID (empty: the device sends an all-zero one).
    pub edid: Vec<u8>,
    /// The resource it shows (0: none).
    pub resource: u32,
    /// The part of the resource it shows.
    pub rect: (u32, u32, u32, u32),
    /// What the screen shows, `rect`'s width by height pixels of 4 bytes, or empty when it
    /// shows nothing.
    pub picture: Vec<u8>,
    /// How many flushes reached it.
    pub flushes: u32,
}

/// A resource: pixels the device holds.
#[derive(Clone, Debug)]
pub struct ModelResource {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, as the device holds them.
    pub pixels: Vec<u8>,
    /// Where its backing memory is: bus address and length.
    pub backing: Option<(u64, u32)>,
}

/// The state of a model GPU.
#[derive(Default)]
pub struct GpuModel {
    pub scanouts: Vec<ModelScanout>,
    pub resources: BTreeMap<u32, ModelResource>,
    /// Memory the device can reach other than the queue's: (bus address, memory). Backings
    /// have to be inside one.
    pub regions: Vec<(u64, Rc<Heap>)>,
    /// Events waiting to be read.
    pub events_read: u32,
    /// The type of every command processed, in order.
    pub log: Vec<u32>,
    /// The fence of every fenced command processed, in order.
    pub fences: Vec<u64>,
    pub lie: GpuLie,
    /// If set, the lie applies only to commands of this type.
    pub lie_only: Option<u32>,
    /// Commands of this type are refused with this error and do nothing.
    pub refuse: Vec<(u32, u32)>,
}

const OK_NODATA: u32 = 0x1100;
const OK_DISPLAY_INFO: u32 = 0x1101;
const OK_EDID: u32 = 0x1104;

fn le32(d: &[u8], at: usize) -> u32 {
    d.get(at..at + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
}

fn le64(d: &[u8], at: usize) -> u64 {
    d.get(at..at + 8).map_or(0, |b| u64::from_le_bytes(b.try_into().unwrap()))
}

impl GpuModel {
    /// A GPU with `n` scanouts, all enabled and 1024 by 768.
    pub fn new(n: u32) -> GpuModel {
        GpuModel {
            scanouts: (0..n)
                .map(|_| ModelScanout {
                    width: 1024,
                    height: 768,
                    enabled: true,
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The device configuration, as the driver reads it.
    pub fn config(&self) -> [u8; 16] {
        let mut c = [0u8; 16];
        c[0..4].copy_from_slice(&self.events_read.to_le_bytes());
        c[8..12].copy_from_slice(&(self.scanouts.len() as u32).to_le_bytes());
        c
    }

    /// Makes `bytes` bytes at `bus` reachable, starting zeroed, and returns the memory.
    pub fn add_region(&mut self, bus: u64, bytes: usize) -> Rc<Heap> {
        let heap = Rc::new(Heap::new(bytes));
        self.regions.push((bus, heap.clone()));
        heap
    }

    fn region(&self, addr: u64, len: u64) -> Option<(Rc<Heap>, usize)> {
        self.regions.iter().find_map(|(bus, heap)| {
            let off = addr.checked_sub(*bus)?;
            (off.checked_add(len)? <= heap.len() as u64).then(|| (heap.clone(), off as usize))
        })
    }

    /// Does what `request` says and returns the response bytes (before any lie about its
    /// length, which the caller cuts to the buffer).
    pub fn execute(&mut self, request: &[u8]) -> Vec<u8> {
        let kind = le32(request, 0);
        let (flags, fence) = (le32(request, 4), le64(request, 8));
        self.log.push(kind);
        let fenced = flags & 1 != 0;
        if fenced {
            self.fences.push(fence);
        }
        let refused = self.refuse.iter().find(|(k, _)| *k == kind).map(|(_, e)| *e);
        let (mut rkind, mut payload) = match refused {
            Some(e) => (e, vec![]),
            None => self.run(kind, request),
        };
        let mut fence_out = if fenced { Some(fence) } else { None };
        let mut out_len = None;
        if self.lie != GpuLie::None && self.lie_only.is_none_or(|k| k == kind) {
            match self.lie {
                GpuLie::ShortResponse => out_len = Some(20),
                GpuLie::WrongType => {
                    rkind = if kind == 0x0100 { OK_NODATA } else { OK_DISPLAY_INFO };
                }
                GpuLie::UnknownType => rkind = 0x1300,
                GpuLie::NoFence => fence_out = None,
                GpuLie::WrongFence => fence_out = fence_out.map(|f| f + 1),
                GpuLie::ShortInfo if kind == 0x0100 => out_len = Some(24 + 24),
                GpuLie::HugeEdid if kind == 0x010A => {
                    payload[0..4].copy_from_slice(&4096u32.to_le_bytes());
                }
                GpuLie::EdidPastResponse if kind == 0x010A => {
                    payload[0..4].copy_from_slice(&1024u32.to_le_bytes());
                    out_len = Some(24 + 8 + 100);
                }
                _ => {}
            }
        }
        let mut out = vec![0u8; 24];
        out[0..4].copy_from_slice(&rkind.to_le_bytes());
        if let Some(f) = fence_out {
            out[4..8].copy_from_slice(&1u32.to_le_bytes());
            out[8..16].copy_from_slice(&f.to_le_bytes());
        }
        out.extend_from_slice(&payload);
        if let Some(n) = out_len {
            out.truncate(n);
        }
        out
    }

    /// Runs a command: the response type and what follows the header.
    fn run(&mut self, kind: u32, r: &[u8]) -> (u32, Vec<u8>) {
        let err = |code: u32| (code, vec![]);
        let ok = (OK_NODATA, vec![]);
        match kind {
            0x0100 => {
                let mut p = vec![0u8; 16 * 24];
                for (i, s) in self.scanouts.iter().enumerate() {
                    let e = &mut p[i * 24..(i + 1) * 24];
                    e[8..12].copy_from_slice(&s.width.to_le_bytes());
                    e[12..16].copy_from_slice(&s.height.to_le_bytes());
                    e[16..20].copy_from_slice(&u32::from(s.enabled).to_le_bytes());
                }
                (OK_DISPLAY_INFO, p)
            }
            0x010A => {
                let Some(s) = self.scanouts.get(le32(r, 24) as usize) else {
                    return err(error_code::INVALID_SCANOUT_ID);
                };
                let mut p = vec![0u8; 8 + 1024];
                p[0..4].copy_from_slice(&(s.edid.len() as u32).to_le_bytes());
                p[8..8 + s.edid.len()].copy_from_slice(&s.edid);
                (OK_EDID, p)
            }
            0x0101 => {
                let (id, format, w, h) = (le32(r, 24), le32(r, 28), le32(r, 32), le32(r, 36));
                if id == 0 || self.resources.contains_key(&id) {
                    return err(error_code::INVALID_RESOURCE_ID);
                }
                if format != FORMAT_B8G8R8X8_UNORM || w == 0 || h == 0 {
                    return err(error_code::INVALID_PARAMETER);
                }
                let bytes = u64::from(w) * u64::from(h) * 4;
                if bytes > 64 << 20 {
                    return err(error_code::OUT_OF_MEMORY);
                }
                let pixels = vec![0u8; bytes as usize];
                self.resources
                    .insert(id, ModelResource { width: w, height: h, pixels, backing: None });
                ok
            }
            0x0102 => {
                let id = le32(r, 24);
                if self.resources.remove(&id).is_none() {
                    return err(error_code::INVALID_RESOURCE_ID);
                }
                for s in self.scanouts.iter_mut().filter(|s| s.resource == id) {
                    s.resource = 0;
                    s.picture.clear();
                }
                ok
            }
            0x0103 => {
                let rect = (le32(r, 24), le32(r, 28), le32(r, 32), le32(r, 36));
                let (scanout, id) = (le32(r, 40) as usize, le32(r, 44));
                if scanout >= self.scanouts.len() {
                    return err(error_code::INVALID_SCANOUT_ID);
                }
                if id == 0 {
                    let s = &mut self.scanouts[scanout];
                    s.resource = 0;
                    s.picture.clear();
                    return ok;
                }
                let Some(res) = self.resources.get(&id) else {
                    return err(error_code::INVALID_RESOURCE_ID);
                };
                let (x, y, w, h) = rect;
                let inside = w != 0
                    && h != 0
                    && u64::from(x) + u64::from(w) <= u64::from(res.width)
                    && u64::from(y) + u64::from(h) <= u64::from(res.height);
                if !inside {
                    return err(error_code::INVALID_PARAMETER);
                }
                let picture = copy_rect(res, rect);
                let s = &mut self.scanouts[scanout];
                s.resource = id;
                s.rect = rect;
                s.picture = picture;
                ok
            }
            0x0104 => {
                let rect = (le32(r, 24), le32(r, 28), le32(r, 32), le32(r, 36));
                let id = le32(r, 40);
                let Some(res) = self.resources.get(&id) else {
                    return err(error_code::INVALID_RESOURCE_ID);
                };
                if !rect_inside(res, rect) {
                    return err(error_code::INVALID_PARAMETER);
                }
                for s in self.scanouts.iter_mut().filter(|s| s.resource == id) {
                    // Only the part of the flush that is on this scanout.
                    let (sx, sy, sw, sh) = s.rect;
                    let (x0, y0) = (rect.0.max(sx), rect.1.max(sy));
                    let x1 =
                        (u64::from(rect.0) + u64::from(rect.2)).min(u64::from(sx) + u64::from(sw));
                    let y1 =
                        (u64::from(rect.1) + u64::from(rect.3)).min(u64::from(sy) + u64::from(sh));
                    if u64::from(x0) < x1 && u64::from(y0) < y1 {
                        for yy in y0..y1 as u32 {
                            let from = (yy as usize * res.width as usize + x0 as usize) * 4;
                            let to = ((yy - sy) as usize * sw as usize + (x0 - sx) as usize) * 4;
                            let n = (x1 as u32 - x0) as usize * 4;
                            s.picture[to..to + n].copy_from_slice(&res.pixels[from..from + n]);
                        }
                    }
                    s.flushes += 1;
                }
                ok
            }
            0x0105 => {
                let rect = (le32(r, 24), le32(r, 28), le32(r, 32), le32(r, 36));
                let (offset, id) = (le64(r, 40), le32(r, 48));
                let Some(res) = self.resources.get(&id) else {
                    return err(error_code::INVALID_RESOURCE_ID);
                };
                let Some((addr, len)) = res.backing else {
                    return err(error_code::UNSPEC);
                };
                if !rect_inside(res, rect) {
                    return err(error_code::INVALID_PARAMETER);
                }
                let (x, y, w, h) = rect;
                let stride = u64::from(res.width) * 4;
                // Gather every row first, so that a transfer that runs off the backing changes
                // nothing.
                let mut rows = Vec::new();
                for row in 0..u64::from(h) {
                    let from = offset + stride * row;
                    if from + u64::from(w) * 4 > u64::from(len) {
                        return err(error_code::INVALID_PARAMETER);
                    }
                    let Some((heap, at)) = self.region(addr + from, u64::from(w) * 4) else {
                        return err(error_code::UNSPEC);
                    };
                    let mut bytes = vec![0u8; w as usize * 4];
                    heap.read_bytes(at, &mut bytes);
                    rows.push(bytes);
                }
                let res = self.resources.get_mut(&id).unwrap();
                for (row, bytes) in rows.iter().enumerate() {
                    let to = ((y as usize + row) * res.width as usize + x as usize) * 4;
                    res.pixels[to..to + bytes.len()].copy_from_slice(bytes);
                }
                ok
            }
            0x0106 => {
                let (id, entries) = (le32(r, 24), le32(r, 28) as usize);
                if r.len() != 32 + 16 * entries || entries == 0 {
                    return err(error_code::INVALID_PARAMETER);
                }
                let Some(res) = self.resources.get(&id) else {
                    return err(error_code::INVALID_RESOURCE_ID);
                };
                if res.backing.is_some() {
                    return err(error_code::UNSPEC);
                }
                // The model keeps one piece, like the driver sends.
                if entries != 1 {
                    return err(error_code::UNSPEC);
                }
                let (addr, len) = (le64(r, 32), le32(r, 40));
                if self.region(addr, u64::from(len)).is_none() {
                    return err(error_code::UNSPEC);
                }
                self.resources.get_mut(&id).unwrap().backing = Some((addr, len));
                ok
            }
            0x0107 => {
                let id = le32(r, 24);
                match self.resources.get_mut(&id) {
                    Some(res) => {
                        res.backing = None;
                        ok
                    }
                    None => err(error_code::INVALID_RESOURCE_ID),
                }
            }
            _ => err(error_code::UNSPEC),
        }
    }
}

fn rect_inside(res: &ModelResource, (x, y, w, h): (u32, u32, u32, u32)) -> bool {
    w != 0
        && h != 0
        && u64::from(x) + u64::from(w) <= u64::from(res.width)
        && u64::from(y) + u64::from(h) <= u64::from(res.height)
}

fn copy_rect(res: &ModelResource, (x, y, w, h): (u32, u32, u32, u32)) -> Vec<u8> {
    let mut out = Vec::with_capacity(w as usize * h as usize * 4);
    for row in y..y + h {
        let from = (row as usize * res.width as usize + x as usize) * 4;
        out.extend_from_slice(&res.pixels[from..from + w as usize * 4]);
    }
    out
}
