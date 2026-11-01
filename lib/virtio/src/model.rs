// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! A software virtio block device for the host tests: it has the registers of the modern PCI
//! transport and processes the queue the driver set up, reading and writing the driver's DMA
//! memory the way hardware would. It can be told to lie.

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

pub struct Dev {
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
    let dma = Rc::new(Heap::new(dma_size));
    let mut disk = vec![0u8; (sectors * 512) as usize];
    for (i, b) in disk.iter_mut().enumerate() {
        *b = (i as u32).wrapping_mul(2654435761).to_le_bytes()[2];
    }
    let dev = Rc::new(RefCell::new(Dev {
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
                (18, 2) => 1,
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
        if (DEVICE_AT..DEVICE_AT + 16).contains(&o) {
            let cfg = d.capacity.to_le_bytes();
            let mut bytes = [0u8; 16];
            bytes[..8].copy_from_slice(&cfg);
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
        if self.q_enable == 0 || self.status & 4 == 0 {
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
            let hdr = self.off(chain[0].0);
            let (kind, sector) = (dma.read32(hdr), dma.read64(hdr + 8));
            let (status_addr, _, _) = *chain.last().unwrap();
            let mut written = 1u32;
            let mut status = 0u8;
            self.requests += 1;
            match kind {
                0 | 1 => {
                    let (daddr, dlen, _) = chain[1];
                    let start = sector as usize * 512;
                    if start + dlen as usize > self.disk.len() {
                        status = 1;
                    } else if kind == 0 {
                        dma.write_bytes(self.off(daddr), &self.disk[start..start + dlen as usize]);
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
            let writable: u32 = chain.iter().filter(|c| c.2 & 2 != 0).map(|c| c.1).sum();
            let mut id = u32::from(head);
            match self.lie {
                Lie::BadStatus => status = 9,
                Lie::LongLength => written = writable + 1000,
                Lie::BadId => id = u32::from(size) + 5,
                Lie::Stranger => id = u32::from(head.wrapping_add(7) % size),
                Lie::ShortRead if kind == 0 => written = 1,
                _ => {}
            }
            dma.write8(self.off(status_addr), status);
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
    let mut c = Cfg([0; 1024]);
    let caps = vec![
        virtio_cap(&mut c, 0x40, 1, 4, COMMON_AT as u32, 56, None),
        virtio_cap(&mut c, 0x54, 2, 4, NOTIFY_AT as u32, 8, Some(4)),
        virtio_cap(&mut c, 0x6C, 3, 4, ISR_AT as u32, 4, None),
        virtio_cap(&mut c, 0x7C, 4, 4, DEVICE_AT as u32, 16, None),
    ];
    (c, caps)
}
