// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A software virtio block device (or, if given an [`InputDev`], input device) for the host
//! tests: it has the registers of the modern PCI
//! transport and processes the queue the driver set up, reading and writing the driver's DMA
//! memory the way hardware would. It can be told to lie.

use crate::model_gpu::{GpuLie, GpuModel};
use core::cell::RefCell;
use pbr_devices::pci::{Capability, Config, cap};
use pbr_devices::regs::{Heap, Registers};
use std::rc::Rc;

pub const DMA_BUS: u64 = 0x4000_0000;
pub const COMMON_AT: usize = 0x000;
pub const ISR_AT: usize = 0x100;
pub const DEVICE_AT: usize = 0x200;
pub const NOTIFY_AT: usize = 0x300;
pub const BAR_SIZE: usize = 0x1000;

/// What the device does wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Lie {
    #[default]
    None,
    /// Says it wrote more than the buffers hold.
    LongLength,
    /// Reports a buffer index outside the queue.
    BadId,
    /// Reports a buffer that was never made available.
    Phantom,
    /// Advances the used index by far more than it should.
    IndexJump,
    /// Writes a status byte that means nothing.
    BadStatus,
    /// Says a read wrote nothing.
    ShortRead,
    /// Completes the request twice.
    Twice,
    /// Reports a buffer in range that isn't in flight, instead of the right one.
    Stranger,
}

/// What a virtio input device says about itself when the driver asks through its
/// configuration (virtio 1.x specification, section 5.8.5). The tests build keyboards, mice and
/// tablets from it, honest and lying.
#[derive(Clone, Debug, Default)]
pub struct InputDev {
    pub name: String,
    /// The bitmaps of the event codes the device has, for `EV_KEY`, `EV_REL` and `EV_ABS`.
    pub key_bits: Vec<u8>,
    pub rel_bits: Vec<u8>,
    pub abs_bits: Vec<u8>,
    /// The raw answer for each of the two absolute axes' `ABS_INFO` (empty means no answer).
    pub abs_info: [Vec<u8>; 2],
}

/// What a tablet's `ABS_INFO` answer looks like: min, max, fuzz, flat, res.
pub fn abs_info(min: i32, max: i32) -> Vec<u8> {
    [min, max, 0, 0, 0].iter().flat_map(|v| v.to_le_bytes()).collect()
}

impl InputDev {
    /// A keyboard: has the letter A (key code 30).
    pub fn keyboard() -> Self {
        let mut key_bits = vec![0u8; 16];
        key_bits[30 / 8] |= 1 << (30 % 8);
        InputDev { name: "QEMU Virtio Keyboard".into(), key_bits, ..Default::default() }
    }

    /// A mouse: buttons, and relative X, Y and the wheel.
    pub fn mouse() -> Self {
        let mut key_bits = vec![0u8; 35];
        key_bits[0x110 / 8] |= 0b111; // BTN_LEFT, BTN_RIGHT, BTN_MIDDLE
        InputDev {
            name: "QEMU Virtio Mouse".into(),
            key_bits,
            rel_bits: vec![0b11, 0b1], // REL_X, REL_Y, REL_WHEEL
            ..Default::default()
        }
    }

    /// A tablet, as QEMU's is: like the mouse (buttons and a wheel) but with absolute X and Y
    /// axes with these ranges instead of relative ones.
    pub fn tablet(x: (i32, i32), y: (i32, i32)) -> Self {
        InputDev {
            name: "QEMU Virtio Tablet".into(),
            rel_bits: vec![0, 0b1],
            abs_bits: vec![0b11],
            abs_info: [abs_info(x.0, x.1), abs_info(y.0, y.1)],
            ..InputDev::mouse()
        }
    }

    /// The configuration space the driver sees after selecting `select` and `subsel`: the two
    /// selector bytes, the size of the answer, padding, and then the answer.
    fn config(&self, select: u8, subsel: u8) -> Vec<u8> {
        let answer: &[u8] = match (select, subsel) {
            (0x01, _) => self.name.as_bytes(),
            (0x11, 1) => &self.key_bits,
            (0x11, 2) => &self.rel_bits,
            (0x11, 3) => &self.abs_bits,
            (0x12, 0 | 1) => &self.abs_info[usize::from(subsel)],
            _ => &[],
        };
        let mut out = vec![select, subsel, answer.len().min(128) as u8, 0, 0, 0, 0, 0];
        out.extend_from_slice(&answer[..answer.len().min(128)]);
        out.resize(136, 0);
        out
    }
}

pub struct Dev {
    /// If this is an input device, what it says it is. (Then the queue holds empty buffers for
    /// events, which the model never fills, so it leaves them alone.)
    pub input: Option<InputDev>,
    pub status: u8,
    pub dev_select: u32,
    pub drv_select: u32,
    pub drv_features: u64,
    pub dev_features: u64,
    pub queue_select: u16,
    pub q_size: u16,
    pub q_vector: u16,
    pub q_enable: u16,
    pub q_addr: [u64; 3],
    pub config_vector: u16,
    pub generation: u8,
    pub capacity: u64,
    pub disk: Vec<u8>,
    pub last_avail: u16,
    pub used_idx: u16,
    pub lie: Lie,
    pub reject_features: bool,
    pub hang_on_reset: bool,
    pub no_vectors: bool,
    pub queue_max: u16,
    pub notify_off: u16,
    pub requests: u32,
    pub flushes: u32,
    pub dma: Rc<Heap>,
    /// Set for a GPU: the device then has a GPU's configuration and processes GPU commands.
    pub gpu: Option<GpuModel>,
}

pub struct Shared(pub Rc<Heap>);

macro_rules! delegate {
    ($($name:ident($($arg:ident: $t:ty),*) $(-> $r:ty)?),*) => {
        $(fn $name(&self, $($arg: $t),*) $(-> $r)? { self.0.$name($($arg),*) })*
    };
}

impl Registers for Shared {
    fn len(&self) -> usize {
        self.0.len()
    }
    delegate!(
        read8(o: usize) -> u8, read16(o: usize) -> u16, read32(o: usize) -> u32, read64(o: usize) -> u64,
        write8(o: usize, v: u8), write16(o: usize, v: u16), write32(o: usize, v: u32), write64(o: usize, v: u64),
        write_bytes(o: usize, d: &[u8])
    );
    fn read_bytes(&self, o: usize, out: &mut [u8]) {
        self.0.read_bytes(o, out)
    }
}

/// The device's BAR as the driver sees it.
pub struct Bar {
    pub dev: Rc<RefCell<Dev>>,
    pub plain: RefCell<Vec<u8>>,
}

pub fn make(sectors: u64, dma_size: usize) -> (Rc<RefCell<Dev>>, Bar, Shared) {
    build(sectors, dma_size, None)
}

/// A GPU with `scanouts` scanouts, offering `VERSION_1` and `EDID`.
pub fn make_gpu(scanouts: u32, dma_size: usize) -> (Rc<RefCell<Dev>>, Bar, Shared) {
    let (dev, bar, dma) = build(0, dma_size, Some(GpuModel::new(scanouts)));
    dev.borrow_mut().dev_features = (1 << 32) | crate::gpu::F_EDID;
    (dev, bar, dma)
}

fn build(sectors: u64, dma_size: usize, gpu: Option<GpuModel>) -> (Rc<RefCell<Dev>>, Bar, Shared) {
    let dma = Rc::new(Heap::new(dma_size));
    let mut disk = vec![0u8; (sectors * 512) as usize];
    for (i, b) in disk.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2654435761).to_le_bytes()[2];
    }
    let dev = Rc::new(RefCell::new(Dev {
        input: None,
        status: 0,
        dev_select: 0,
        drv_select: 0,
        drv_features: 0,
        dev_features: (1 << 32) | (1 << 9),
        queue_select: 0,
        q_size: 0,
        q_vector: 0xFFFF,
        q_enable: 0,
        q_addr: [0; 3],
        config_vector: 0xFFFF,
        generation: 0,
        capacity: sectors,
        disk,
        last_avail: 0,
        used_idx: 0,
        lie: Lie::None,
        reject_features: false,
        hang_on_reset: false,
        no_vectors: false,
        queue_max: 128,
        notify_off: 0,
        requests: 0,
        flushes: 0,
        dma: dma.clone(),
        gpu,
    }));
    let bar = Bar { dev: dev.clone(), plain: RefCell::new(vec![0; BAR_SIZE]) };
    (dev, bar, Shared(dma))
}

impl Bar {
    fn rd(&self, o: usize, width: usize) -> u64 {
        assert!(o.is_multiple_of(width) && o + width <= BAR_SIZE, "bad access {o}");
        let d = self.dev.borrow();
        if (COMMON_AT..COMMON_AT + 56).contains(&o) {
            return match (o - COMMON_AT, width) {
                (0, 4) => u64::from(d.dev_select),
                (4, 4) => match d.dev_select {
                    0 => d.dev_features & 0xFFFF_FFFF,
                    1 => d.dev_features >> 32,
                    _ => 0,
                },
                (8, 4) => u64::from(d.drv_select),
                (12, 4) => 0,
                (16, 2) => u64::from(d.config_vector),
                (18, 2) => {
                    if d.gpu.is_some() {
                        2
                    } else {
                        1
                    }
                }
                (20, 1) => u64::from(d.status),
                (21, 1) => u64::from(d.generation),
                (22, 2) => u64::from(d.queue_select),
                (24, 2) => {
                    if d.queue_select == 0 {
                        u64::from(if d.q_size != 0 { d.q_size } else { d.queue_max })
                    } else {
                        0
                    }
                }
                (26, 2) => u64::from(d.q_vector),
                (28, 2) => u64::from(d.q_enable),
                (30, 2) => u64::from(d.notify_off),
                (32, 4) => d.q_addr[0] & 0xFFFF_FFFF,
                (36, 4) => d.q_addr[0] >> 32,
                (40, 4) => d.q_addr[1] & 0xFFFF_FFFF,
                (44, 4) => d.q_addr[1] >> 32,
                (48, 4) => d.q_addr[2] & 0xFFFF_FFFF,
                (52, 4) => d.q_addr[2] >> 32,
                other => panic!("unmodelled common read {other:?}"),
            };
        }
        if o == ISR_AT {
            return 1;
        }
        if let Some(input) = &d.input
            && (DEVICE_AT..DEVICE_AT + 136).contains(&o)
        {
            // The driver wrote the selector bytes into the plain memory; answer for them.
            let m = self.plain.borrow();
            let bytes = input.config(m[DEVICE_AT], m[DEVICE_AT + 1]);
            return (0..width).fold(0, |v, i| v | (u64::from(bytes[o - DEVICE_AT + i]) << (8 * i)));
        }
        if (DEVICE_AT..DEVICE_AT + 16).contains(&o) {
            let mut bytes = [0u8; 16];
            match &d.gpu {
                Some(g) => bytes = g.config(),
                None => bytes[..8].copy_from_slice(&d.capacity.to_le_bytes()),
            }
            let mut v = 0u64;
            for i in 0..width {
                v |= u64::from(bytes[o - DEVICE_AT + i]) << (8 * i);
            }
            return v;
        }
        let m = self.plain.borrow();
        (0..width).fold(0, |v, i| v | (u64::from(m[o + i]) << (8 * i)))
    }

    fn wr(&self, o: usize, width: usize, v: u64) {
        assert!(o.is_multiple_of(width) && o + width <= BAR_SIZE, "bad access {o}");
        if (COMMON_AT..COMMON_AT + 56).contains(&o) {
            let mut d = self.dev.borrow_mut();
            match (o - COMMON_AT, width) {
                (0, 4) => d.dev_select = v as u32,
                (8, 4) => d.drv_select = v as u32,
                (12, 4) => {
                    let sh = 32 * d.drv_select.min(1);
                    d.drv_features =
                        (d.drv_features & !(0xFFFF_FFFFu64 << sh)) | ((v & 0xFFFF_FFFF) << sh);
                }
                (16, 2) => d.config_vector = if d.no_vectors { 0xFFFF } else { v as u16 },
                (20, 1) => {
                    if v == 0 {
                        if !d.hang_on_reset {
                            d.status = 0;
                            d.q_enable = 0;
                            d.q_size = 0;
                            d.last_avail = 0;
                            d.used_idx = 0;
                            d.drv_features = 0;
                        }
                    } else {
                        let mut s = v as u8;
                        if s & 8 != 0 && (d.reject_features || d.drv_features & (1 << 32) == 0) {
                            s &= !8;
                        }
                        d.status = s;
                    }
                }
                (22, 2) => d.queue_select = v as u16,
                (24, 2) => d.q_size = v as u16,
                (26, 2) => d.q_vector = if d.no_vectors { 0xFFFF } else { v as u16 },
                (28, 2) => d.q_enable = v as u16,
                (32, 4) | (36, 4) | (40, 4) | (44, 4) | (48, 4) | (52, 4) => {
                    let (i, hi) = ((o - COMMON_AT - 32) / 8, (o - COMMON_AT) % 8 == 4);
                    let sh = if hi { 32 } else { 0 };
                    d.q_addr[i] =
                        (d.q_addr[i] & !(0xFFFF_FFFFu64 << sh)) | ((v & 0xFFFF_FFFF) << sh);
                }
                other => panic!("unmodelled common write {other:?}"),
            }
            return;
        }
        if o == NOTIFY_AT {
            self.dev.borrow_mut().process();
            return;
        }
        // A GPU's `events_clear`: the bits written are cleared from `events_read`.
        if o == DEVICE_AT + 4
            && width == 4
            && let Some(g) = self.dev.borrow_mut().gpu.as_mut()
        {
            g.events_read &= !(v as u32);
            return;
        }
        let mut m = self.plain.borrow_mut();
        for i in 0..width {
            m[o + i] = (v >> (8 * i)) as u8;
        }
    }
}

impl Registers for Bar {
    fn len(&self) -> usize {
        BAR_SIZE
    }
    fn read8(&self, o: usize) -> u8 {
        self.rd(o, 1) as u8
    }
    fn read16(&self, o: usize) -> u16 {
        self.rd(o, 2) as u16
    }
    fn read32(&self, o: usize) -> u32 {
        self.rd(o, 4) as u32
    }
    fn read64(&self, o: usize) -> u64 {
        self.rd(o, 8)
    }
    fn write8(&self, o: usize, v: u8) {
        self.wr(o, 1, v.into())
    }
    fn write16(&self, o: usize, v: u16) {
        self.wr(o, 2, v.into())
    }
    fn write32(&self, o: usize, v: u32) {
        self.wr(o, 4, v.into())
    }
    fn write64(&self, o: usize, v: u64) {
        self.wr(o, 8, v)
    }
}

impl Dev {
    fn off(&self, bus: u64) -> usize {
        (bus - DMA_BUS) as usize
    }

    /// Handles everything the driver has made available.
    pub fn process(&mut self) {
        if self.q_enable == 0 || self.status & 4 == 0 || self.input.is_some() {
            return;
        }
        let dma = self.dma.clone();
        let (desc, avail, used) =
            (self.off(self.q_addr[0]), self.off(self.q_addr[1]), self.off(self.q_addr[2]));
        let size = self.q_size;
        let avail_idx = dma.read16(avail + 2);
        while self.last_avail != avail_idx {
            let head = dma.read16(avail + 4 + 2 * usize::from(self.last_avail % size));
            self.last_avail = self.last_avail.wrapping_add(1);
            // Walk the chain.
            let mut chain = Vec::new();
            let mut at = head;
            loop {
                let d = desc + 16 * usize::from(at);
                let (addr, len, flags, next) =
                    (dma.read64(d), dma.read32(d + 8), dma.read16(d + 12), dma.read16(d + 14));
                chain.push((addr, len, flags));
                if flags & 1 == 0 {
                    break;
                }
                at = next;
            }
            let writable: u32 = chain.iter().filter(|c| c.2 & 2 != 0).map(|c| c.1).sum();
            let mut id = u32::from(head);
            let mut written;
            // A block device's status byte is written after the lies are told; a GPU has none.
            let mut status_byte = None;
            if self.gpu.is_some() {
                self.requests += 1;
                written = self.gpu_request(&chain);
            } else {
                let hdr = self.off(chain[0].0);
                let (kind, sector) = (dma.read32(hdr), dma.read64(hdr + 8));
                let (status_addr, _, _) = *chain.last().unwrap();
                written = 1u32;
                let mut status = 0u8;
                self.requests += 1;
                match kind {
                    0 | 1 => {
                        let (daddr, dlen, _) = chain[1];
                        let start = sector as usize * 512;
                        if start + dlen as usize > self.disk.len() {
                            status = 1;
                        } else if kind == 0 {
                            dma.write_bytes(
                                self.off(daddr),
                                &self.disk[start..start + dlen as usize],
                            );
                            written += dlen;
                        } else {
                            let mut buf = vec![0u8; dlen as usize];
                            dma.read_bytes(self.off(daddr), &mut buf);
                            self.disk[start..start + dlen as usize].copy_from_slice(&buf);
                        }
                    }
                    4 => self.flushes += 1,
                    _ => status = 2,
                }
                match self.lie {
                    Lie::BadStatus => status = 9,
                    Lie::ShortRead if kind == 0 => written = 1,
                    _ => {}
                }
                status_byte = Some((status_addr, status));
            }
            match self.lie {
                Lie::LongLength => written = writable + 1000,
                Lie::BadId => id = u32::from(size) + 5,
                Lie::Stranger => id = u32::from(head.wrapping_add(7) % size),
                _ => {}
            }
            if let Some((addr, status)) = status_byte {
                dma.write8(self.off(addr), status);
            }
            let push = |this: &mut Dev, id: u32, len: u32| {
                let slot = usize::from(this.used_idx % size);
                dma.write32(used + 4 + 8 * slot, id);
                dma.write32(used + 8 + 8 * slot, len);
                this.used_idx = this.used_idx.wrapping_add(1);
            };
            if self.lie == Lie::Phantom {
                push(self, u32::from(head.wrapping_add(7) % size), 0);
            }
            push(self, id, written);
            if self.lie == Lie::Twice {
                push(self, id, written);
            }
            if self.lie == Lie::IndexJump {
                self.used_idx = self.used_idx.wrapping_add(1000);
            }
            dma.write16(used + 2, self.used_idx);
        }
    }
}

impl Dev {
    /// Runs a GPU command: the request is the first buffer of the chain, the response goes in
    /// the second. Returns how many bytes of response were written.
    fn gpu_request(&mut self, chain: &[(u64, u32, u16)]) -> u32 {
        let dma = self.dma.clone();
        let (request_at, request_len, _) = chain[0];
        let (response_at, response_len, _) = chain[1];
        let mut request = vec![0u8; request_len as usize];
        dma.read_bytes(self.off(request_at), &mut request);
        let gpu = self.gpu.as_mut().expect("a GPU");
        let mut response = gpu.execute(&request);
        if gpu.lie == GpuLie::Silent {
            // Claims the whole header, having written none of it.
            return 24;
        }
        response.truncate(response_len as usize);
        dma.write_bytes(self.off(response_at), &response);
        response.len() as u32
    }
}

/// A function's configuration space with the four virtio capabilities.
pub struct Cfg(pub [u32; 1024]);

impl Config for Cfg {
    fn read(&mut self, o: u16) -> u32 {
        self.0.get(usize::from(o) / 4).copied().unwrap_or(!0)
    }
    fn write(&mut self, o: u16, v: u32) {
        if let Some(r) = self.0.get_mut(usize::from(o) / 4) {
            *r = v;
        }
    }
}

/// One vendor capability: `kind`, BAR, offset, length (and the notify multiplier).
pub fn virtio_cap(
    cfg: &mut Cfg,
    at: usize,
    kind: u8,
    bar: u8,
    offset: u32,
    length: u32,
    mult: Option<u32>,
) -> Capability {
    let len: u8 = if mult.is_some() { 20 } else { 16 };
    cfg.0[at / 4] = 9 | (u32::from(len) << 16) | (u32::from(kind) << 24);
    cfg.0[at / 4 + 1] = u32::from(bar);
    cfg.0[at / 4 + 2] = offset;
    cfg.0[at / 4 + 3] = length;
    if let Some(m) = mult {
        cfg.0[at / 4 + 4] = m;
    }
    Capability { id: cap::VENDOR, offset: at as u8 }
}

/// The standard layout the model BAR has, found through capabilities.
pub fn standard_caps() -> (Cfg, Vec<Capability>) {
    caps_with_device_length(16)
}

/// The layout of an input device, whose configuration is longer (136 bytes).
pub fn input_caps() -> (Cfg, Vec<Capability>) {
    caps_with_device_length(136)
}

fn caps_with_device_length(device: u32) -> (Cfg, Vec<Capability>) {
    let mut c = Cfg([0; 1024]);
    let caps = vec![
        virtio_cap(&mut c, 0x40, 1, 4, COMMON_AT as u32, 56, None),
        virtio_cap(&mut c, 0x54, 2, 4, NOTIFY_AT as u32, 8, Some(4)),
        virtio_cap(&mut c, 0x6C, 3, 4, ISR_AT as u32, 4, None),
        virtio_cap(&mut c, 0x7C, 4, 4, DEVICE_AT as u32, device, None),
    ];
    (c, caps)
}
