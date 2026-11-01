// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Access to device registers and to memory shared with a device, as a trait, so the code
//! that knows a device's protocol (virtio, MSI-X) can be tested on the host against a fake.

/// A block of registers or shared memory with volatile, bounds-checked accesses.
///
/// Offsets and widths come from the driver's own code, never from a device, so an access outside
/// the block or not aligned to its width is a bug in the driver and the implementation panics.
/// Anything a device tells a driver (an offset in a capability, an index in a ring) is checked
/// by the driver *before* it becomes an offset here.
pub trait Registers {
    /// The size of the block in bytes.
    fn len(&self) -> usize;
    /// True if the block is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Reads 8 bits.
    fn read8(&self, offset: usize) -> u8;
    /// Reads 16 bits.
    fn read16(&self, offset: usize) -> u16;
    /// Reads 32 bits.
    fn read32(&self, offset: usize) -> u32;
    /// Reads 64 bits.
    fn read64(&self, offset: usize) -> u64;
    /// Writes 8 bits.
    fn write8(&self, offset: usize, value: u8);
    /// Writes 16 bits.
    fn write16(&self, offset: usize, value: u16);
    /// Writes 32 bits.
    fn write32(&self, offset: usize, value: u32);
    /// Writes 64 bits.
    fn write64(&self, offset: usize, value: u64);

    /// Copies bytes out of the block. The default reads a byte at a time; blocks of ordinary
    /// shared memory can do better.
    fn read_bytes(&self, offset: usize, out: &mut [u8]) {
        for (i, b) in out.iter_mut().enumerate() {
            *b = self.read8(offset + i);
        }
    }

    /// Copies bytes into the block.
    fn write_bytes(&self, offset: usize, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.write8(offset + i, b);
        }
    }
}

/// `offset..offset + length` fits in a block of `size` bytes.
pub fn fits(offset: u64, length: u64, size: u64) -> bool {
    offset.checked_add(length).is_some_and(|end| end <= size)
}

/// Ordinary memory behind the trait: for tests and for fake devices.
pub struct Heap(core::cell::RefCell<alloc::vec::Vec<u8>>);

impl Heap {
    /// `len` bytes of zeros.
    pub fn new(len: usize) -> Heap {
        Heap(core::cell::RefCell::new(alloc::vec![0; len]))
    }

    fn get<const N: usize>(&self, o: usize) -> [u8; N] {
        let m = self.0.borrow();
        assert!(o.is_multiple_of(N) && o + N <= m.len(), "bad access at {o}");
        let mut b = [0; N];
        b.copy_from_slice(&m[o..o + N]);
        b
    }

    fn put<const N: usize>(&self, o: usize, v: [u8; N]) {
        let mut m = self.0.borrow_mut();
        assert!(o.is_multiple_of(N) && o + N <= m.len(), "bad access at {o}");
        m[o..o + N].copy_from_slice(&v);
    }
}

impl Registers for Heap {
    fn len(&self) -> usize {
        self.0.borrow().len()
    }
    fn read8(&self, o: usize) -> u8 {
        self.get::<1>(o)[0]
    }
    fn read16(&self, o: usize) -> u16 {
        u16::from_le_bytes(self.get(o))
    }
    fn read32(&self, o: usize) -> u32 {
        u32::from_le_bytes(self.get(o))
    }
    fn read64(&self, o: usize) -> u64 {
        u64::from_le_bytes(self.get(o))
    }
    fn write8(&self, o: usize, v: u8) {
        self.put(o, [v])
    }
    fn write16(&self, o: usize, v: u16) {
        self.put(o, v.to_le_bytes())
    }
    fn write32(&self, o: usize, v: u32) {
        self.put(o, v.to_le_bytes())
    }
    fn write64(&self, o: usize, v: u64) {
        self.put(o, v.to_le_bytes())
    }
    fn read_bytes(&self, o: usize, out: &mut [u8]) {
        let m = self.0.borrow();
        assert!(o + out.len() <= m.len(), "bad access at {o}");
        out.copy_from_slice(&m[o..o + out.len()]);
    }
    fn write_bytes(&self, o: usize, data: &[u8]) {
        let mut m = self.0.borrow_mut();
        assert!(o + data.len() <= m.len(), "bad access at {o}");
        m[o..o + data.len()].copy_from_slice(data);
    }
}
