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

/// True if an access of `width` bytes at `offset` is allowed in a block of `len` bytes: inside
/// it, and aligned to its width. Every [`Registers`] implementation checks this (and panics if
/// it fails), so the drivers' wrappers and the host's fake refuse exactly the same accesses.
pub fn access_ok(offset: usize, width: usize, len: usize) -> bool {
    width != 0
        && offset.is_multiple_of(width)
        && offset.checked_add(width).is_some_and(|end| end <= len)
}

/// True if copying `count` bytes at `offset` stays inside a block of `len` bytes.
pub fn copy_ok(offset: usize, count: usize, len: usize) -> bool {
    offset.checked_add(count).is_some_and(|end| end <= len)
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
        assert!(access_ok(o, N, m.len()), "bad access at {o}");
        let mut b = [0; N];
        b.copy_from_slice(&m[o..o + N]);
        b
    }

    fn put<const N: usize>(&self, o: usize, v: [u8; N]) {
        let mut m = self.0.borrow_mut();
        assert!(access_ok(o, N, m.len()), "bad access at {o}");
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
        assert!(copy_ok(o, out.len(), m.len()), "bad access at {o}");
        out.copy_from_slice(&m[o..o + out.len()]);
    }
    fn write_bytes(&self, o: usize, data: &[u8]) {
        let mut m = self.0.borrow_mut();
        assert!(copy_ok(o, data.len(), m.len()), "bad access at {o}");
        m[o..o + data.len()].copy_from_slice(data);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accesses_inside_and_aligned_are_allowed() {
        for width in [1, 2, 4, 8] {
            for offset in (0..64).step_by(width) {
                assert!(access_ok(offset, width, 64), "{width} at {offset}");
            }
        }
    }

    #[test]
    fn misaligned_outside_and_wrapping_accesses_are_refused() {
        assert!(!access_ok(1, 2, 64));
        assert!(!access_ok(2, 4, 64));
        assert!(!access_ok(4, 8, 64));
        assert!(!access_ok(64, 1, 64)); // just past the end
        assert!(!access_ok(60, 8, 64)); // aligned to 4 but not 8, and runs off the end
        assert!(!access_ok(56, 8, 60)); // runs off a block that isn't a multiple of 8
        assert!(!access_ok(0, 1, 0)); // an empty block
        assert!(!access_ok(0, 0, 64)); // no width
        assert!(!access_ok(usize::MAX - 7, 8, usize::MAX)); // the end wraps round
    }

    #[test]
    fn copies_are_checked_without_wrapping() {
        assert!(copy_ok(0, 64, 64));
        assert!(copy_ok(64, 0, 64));
        assert!(!copy_ok(1, 64, 64));
        assert!(!copy_ok(usize::MAX, 2, usize::MAX));
    }

    #[test]
    fn the_fake_refuses_what_the_check_refuses() {
        let h = Heap::new(16);
        h.write64(8, 0x0102_0304_0506_0708);
        assert_eq!(h.read32(8), 0x0506_0708);
        for (offset, width) in [(1usize, 2usize), (16, 1), (12, 8), (usize::MAX, 1)] {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match width {
                1 => u64::from(h.read8(offset)),
                2 => u64::from(h.read16(offset)),
                _ => h.read64(offset),
            }));
            assert!(r.is_err(), "{width} at {offset} was allowed");
        }
    }
}
