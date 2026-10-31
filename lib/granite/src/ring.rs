// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Ring buffers in shared memory (docs/design/0008-messages-and-interfaces.md, section 6).
//!
//! One side produces entries, the other consumes them, through memory both have mapped. A
//! notification is the doorbell. The kernel knows nothing about rings.
//!
//! **The other side is never trusted.** It can change anything in the shared memory at any
//! moment, even maliciously. So every field is read once into a local copy before it's
//! checked, entries are copied out before they're looked at, and an impossible state is
//! reported as [`RingError::Broken`] rather than followed. Nothing here panics or overruns
//! whatever the other side writes.

use core::sync::atomic::{AtomicU32, Ordering};

/// The ring's magic number, `"RING"`.
pub const MAGIC: u32 = u32::from_le_bytes(*b"RING");
/// The layout version.
pub const VERSION: u16 = 1;
/// Where the entries start.
pub const ENTRIES_AT: usize = 256;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_ENTRY_SIZE: usize = 8;
const OFF_COUNT: usize = 12;
const OFF_HEAD: usize = 64;
const OFF_CONSUMER_WAITING: usize = 68;
const OFF_TAIL: usize = 128;
const OFF_PRODUCER_WAITING: usize = 132;

/// The kind of ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Entries of a fixed size.
    Entries = 0,
    /// A stream of bytes.
    Bytes = 1,
}

/// What went wrong.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RingError {
    /// Full (producing) or empty (consuming): try again after the doorbell.
    WouldBlock,
    /// The memory doesn't hold a valid ring, or the other side broke it. Stop using it.
    Broken,
    /// The arguments don't fit this ring (wrong entry size, or memory too small).
    Invalid,
}

/// The shared memory a ring lives in.
#[derive(Clone, Copy)]
struct Shared {
    base: *mut u8,
    len: usize,
}

impl Shared {
    fn word(&self, at: usize) -> &AtomicU32 {
        // SAFETY: `at` is one of the fixed header offsets, all 4-byte aligned and inside the
        // 256-byte header, which `new` checked fits; atomics tolerate the other side writing.
        unsafe { &*(self.base.add(at) as *const AtomicU32) }
    }

    fn get(&self, at: usize) -> u32 {
        self.word(at).load(Ordering::Acquire)
    }

    fn set(&self, at: usize, v: u32) {
        self.word(at).store(v, Ordering::Release);
    }
}

/// The geometry both sides agree on, read once and checked.
#[derive(Clone, Copy, Debug)]
struct Geometry {
    kind: Kind,
    entry_size: usize,
    count: u32,
}

fn geometry(s: &Shared) -> Result<Geometry, RingError> {
    if s.len < ENTRIES_AT {
        return Err(RingError::Broken);
    }
    let magic = s.get(OFF_MAGIC);
    let vk = s.get(OFF_VERSION);
    let entry_size = s.get(OFF_ENTRY_SIZE) as usize;
    let count = s.get(OFF_COUNT);
    let kind = match vk >> 16 {
        0 => Kind::Entries,
        1 => Kind::Bytes,
        _ => return Err(RingError::Broken),
    };
    if magic != MAGIC || vk & 0xFFFF != u32::from(VERSION) {
        return Err(RingError::Broken);
    }
    if entry_size == 0
        || count == 0
        || !count.is_power_of_two()
        || (kind == Kind::Bytes && entry_size != 1)
    {
        return Err(RingError::Broken);
    }
    let bytes = entry_size.checked_mul(count as usize).ok_or(RingError::Broken)?;
    if ENTRIES_AT.checked_add(bytes).is_none_or(|end| end > s.len) {
        return Err(RingError::Broken);
    }
    Ok(Geometry { kind, entry_size, count })
}

/// Sets up a new ring in `len` bytes of memory at `base`: the creator does this once, before
/// the other side maps it. Returns how many entries fit.
///
/// # Safety
/// `base` must point at `len` writable bytes, aligned to 64, that only rings use.
pub unsafe fn init(
    base: *mut u8,
    len: usize,
    kind: Kind,
    entry_size: usize,
) -> Result<u32, RingError> {
    if len < ENTRIES_AT + entry_size || entry_size == 0 || (kind == Kind::Bytes && entry_size != 1)
    {
        return Err(RingError::Invalid);
    }
    let fits = (len - ENTRIES_AT) / entry_size;
    let count = 1u32 << (usize::BITS - 1 - fits.leading_zeros()).min(31);
    // SAFETY: as the caller promises.
    unsafe { core::ptr::write_bytes(base, 0, ENTRIES_AT) };
    let s = Shared { base, len };
    s.set(OFF_ENTRY_SIZE, entry_size as u32);
    s.set(OFF_COUNT, count);
    s.set(OFF_VERSION, u32::from(VERSION) | ((kind as u32) << 16));
    s.set(OFF_MAGIC, MAGIC);
    Ok(count)
}

/// The producing side of a ring.
pub struct Producer {
    s: Shared,
    g: Geometry,
    /// Our own count of entries produced: never read back from shared memory.
    head: u32,
}

/// The consuming side of a ring.
pub struct Consumer {
    s: Shared,
    g: Geometry,
    /// Our own count of entries consumed.
    tail: u32,
}

// SAFETY: the shared memory is only reached through atomics and bounded copies.
unsafe impl Send for Producer {}
// SAFETY: as above.
unsafe impl Send for Consumer {}

impl Producer {
    /// The producing side of the ring in `len` bytes at `base`.
    ///
    /// # Safety
    /// `base` must point at `len` bytes mapped readable and writable for as long as this
    /// lives, aligned to 64. Their contents may be anything.
    pub unsafe fn new(base: *mut u8, len: usize) -> Result<Producer, RingError> {
        let s = Shared { base, len };
        let g = geometry(&s)?;
        Ok(Producer { s, g, head: s.get(OFF_HEAD) })
    }

    /// How many entries fit in the ring.
    pub fn capacity(&self) -> u32 {
        self.g.count
    }

    /// The ring's kind.
    pub fn kind(&self) -> Kind {
        self.g.kind
    }

    /// Free entries right now.
    pub fn free(&self) -> Result<u32, RingError> {
        let tail = self.s.get(OFF_TAIL);
        let used = self.head.wrapping_sub(tail);
        if used > self.g.count {
            return Err(RingError::Broken);
        }
        Ok(self.g.count - used)
    }

    fn put(&self, index: u32, bytes: &[u8]) {
        let at = ENTRIES_AT + (index & (self.g.count - 1)) as usize * self.g.entry_size;
        // SAFETY: `at + entry_size` is inside the checked geometry; only we write entries.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.s.base.add(at), bytes.len()) };
    }

    /// Adds one entry (exactly `entry_size` bytes). Returns whether the consumer asked for the
    /// doorbell.
    pub fn push(&mut self, entry: &[u8]) -> Result<bool, RingError> {
        if entry.len() != self.g.entry_size {
            return Err(RingError::Invalid);
        }
        if self.free()? == 0 {
            self.s.set(OFF_PRODUCER_WAITING, 1);
            // The consumer may have made room just before seeing that flag.
            if self.free()? == 0 {
                return Err(RingError::WouldBlock);
            }
        }
        self.s.set(OFF_PRODUCER_WAITING, 0);
        self.put(self.head, entry);
        self.head = self.head.wrapping_add(1);
        self.s.set(OFF_HEAD, self.head);
        Ok(self.s.get(OFF_CONSUMER_WAITING) == 1)
    }

    /// Writes as many bytes of `data` as fit (a byte ring). Returns how many, and whether the
    /// consumer asked for the doorbell.
    pub fn write(&mut self, data: &[u8]) -> Result<(usize, bool), RingError> {
        if self.g.kind != Kind::Bytes {
            return Err(RingError::Invalid);
        }
        let n = (self.free()? as usize).min(data.len());
        if n == 0 && !data.is_empty() {
            self.s.set(OFF_PRODUCER_WAITING, 1);
            if self.free()? == 0 {
                return Err(RingError::WouldBlock);
            }
            return self.write(data);
        }
        for (i, b) in data[..n].iter().enumerate() {
            self.put(self.head.wrapping_add(i as u32), core::slice::from_ref(b));
        }
        self.head = self.head.wrapping_add(n as u32);
        self.s.set(OFF_HEAD, self.head);
        Ok((n, self.s.get(OFF_CONSUMER_WAITING) == 1))
    }
}

impl Consumer {
    /// The consuming side of the ring in `len` bytes at `base`.
    ///
    /// # Safety
    /// As for [`Producer::new`].
    pub unsafe fn new(base: *mut u8, len: usize) -> Result<Consumer, RingError> {
        let s = Shared { base, len };
        let g = geometry(&s)?;
        Ok(Consumer { s, g, tail: s.get(OFF_TAIL) })
    }

    /// Entries waiting right now.
    pub fn available(&self) -> Result<u32, RingError> {
        let head = self.s.get(OFF_HEAD);
        let used = head.wrapping_sub(self.tail);
        if used > self.g.count {
            return Err(RingError::Broken);
        }
        Ok(used)
    }

    fn get(&self, index: u32, out: &mut [u8]) {
        let at = ENTRIES_AT + (index & (self.g.count - 1)) as usize * self.g.entry_size;
        // SAFETY: inside the checked geometry. The producer could be scribbling on it; we only
        // ever copy it out, never look at it in place.
        unsafe { core::ptr::copy_nonoverlapping(self.s.base.add(at), out.as_mut_ptr(), out.len()) };
    }

    /// Takes one entry into `out` (exactly `entry_size` bytes). Returns whether the producer
    /// asked for the doorbell.
    pub fn pop(&mut self, out: &mut [u8]) -> Result<bool, RingError> {
        if out.len() != self.g.entry_size {
            return Err(RingError::Invalid);
        }
        if self.available()? == 0 {
            self.s.set(OFF_CONSUMER_WAITING, 1);
            if self.available()? == 0 {
                return Err(RingError::WouldBlock);
            }
        }
        self.s.set(OFF_CONSUMER_WAITING, 0);
        self.get(self.tail, out);
        self.tail = self.tail.wrapping_add(1);
        self.s.set(OFF_TAIL, self.tail);
        Ok(self.s.get(OFF_PRODUCER_WAITING) == 1)
    }

    /// Reads up to `out.len()` bytes (a byte ring). Returns how many, and whether the
    /// producer asked for the doorbell.
    pub fn read(&mut self, out: &mut [u8]) -> Result<(usize, bool), RingError> {
        if self.g.kind != Kind::Bytes {
            return Err(RingError::Invalid);
        }
        let n = (self.available()? as usize).min(out.len());
        if n == 0 && !out.is_empty() {
            self.s.set(OFF_CONSUMER_WAITING, 1);
            if self.available()? == 0 {
                return Err(RingError::WouldBlock);
            }
            return self.read(out);
        }
        for i in 0..n {
            self.get(self.tail.wrapping_add(i as u32), &mut out[i..=i]);
        }
        self.tail = self.tail.wrapping_add(n as u32);
        self.s.set(OFF_TAIL, self.tail);
        Ok((n, self.s.get(OFF_PRODUCER_WAITING) == 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Shared memory for tests: 64-byte aligned.
    struct Mem(Vec<u64>);
    impl Mem {
        fn new(len: usize) -> Self {
            Mem(vec![0; len.div_ceil(8) + 8])
        }
        fn ptr(&self) -> *mut u8 {
            let p = self.0.as_ptr() as usize;
            p.next_multiple_of(64) as *mut u8
        }
    }

    // The memory these use is test memory: 64-byte aligned, at least `len` bytes, and alive
    // for the whole test.
    fn ring(at: *mut u8, len: usize, kind: Kind, size: usize) -> Result<u32, RingError> {
        // SAFETY: as above.
        unsafe { init(at, len, kind, size) }
    }
    fn producer(at: *mut u8, len: usize) -> Result<Producer, RingError> {
        // SAFETY: as above.
        unsafe { Producer::new(at, len) }
    }
    fn consumer(at: *mut u8, len: usize) -> Result<Consumer, RingError> {
        // SAFETY: as above.
        unsafe { Consumer::new(at, len) }
    }

    #[test]
    fn entries_round_trip_and_wrap() {
        let m = Mem::new(4096);
        let count = ring(m.ptr(), 4096, Kind::Entries, 32).unwrap();
        assert_eq!(count, 64);
        let (mut p, mut c) = (producer(m.ptr(), 4096).unwrap(), consumer(m.ptr(), 4096).unwrap());
        let mut out = [0u8; 32];
        assert_eq!(c.pop(&mut out), Err(RingError::WouldBlock));
        for round in 0..10u8 {
            for i in 0..64u8 {
                p.push(&[round ^ i; 32]).unwrap();
            }
            assert_eq!(p.push(&[0; 32]), Err(RingError::WouldBlock));
            for i in 0..64u8 {
                c.pop(&mut out).unwrap();
                assert_eq!(out, [round ^ i; 32]);
            }
        }
        assert_eq!(p.push(&[0; 31]), Err(RingError::Invalid));
    }

    #[test]
    fn bytes_round_trip() {
        let m = Mem::new(300);
        let n = ring(m.ptr(), 300, Kind::Bytes, 1).unwrap();
        assert_eq!(n, 32);
        let (mut p, mut c) = (producer(m.ptr(), 300).unwrap(), consumer(m.ptr(), 300).unwrap());
        let msg: Vec<u8> = (0..100u8).collect();
        let mut got = Vec::new();
        let mut sent = 0;
        while got.len() < msg.len() {
            if sent < msg.len()
                && let Ok((n, _)) = p.write(&msg[sent..])
            {
                sent += n;
            }
            let mut buf = [0u8; 7];
            if let Ok((n, _)) = c.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
        }
        assert_eq!(got, msg);
    }

    #[test]
    fn refuses_bad_rings() {
        let m = Mem::new(4096);
        // Not set up at all.
        assert!(matches!(consumer(m.ptr(), 4096), Err(RingError::Broken)));
        ring(m.ptr(), 4096, Kind::Entries, 16).unwrap();
        // Claims more entries than the memory holds.
        let s = Shared { base: m.ptr(), len: 4096 };
        s.set(OFF_COUNT, 1 << 20);
        assert!(matches!(consumer(m.ptr(), 4096), Err(RingError::Broken)));
        s.set(OFF_COUNT, 100); // not a power of two
        assert!(matches!(producer(m.ptr(), 4096), Err(RingError::Broken)));
        assert!(matches!(producer(m.ptr(), 100), Err(RingError::Broken)));
    }

    /// The other side writes garbage into the header and entries the whole time: the honest
    /// side must never panic, overrun or loop for ever; it may only get errors or junk data.
    #[test]
    fn survives_a_hostile_producer() {
        let m = Arc::new(Mem::new(4096));
        ring(m.ptr(), 4096, Kind::Entries, 16).unwrap();
        let base = m.ptr() as usize;
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (m2, stop2) = (m.clone(), stop.clone());
        let vandal = std::thread::spawn(move || {
            let _keep = m2;
            let mut x = 0x1234_5678u32;
            while !stop2.load(Ordering::Relaxed) {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let at = (x as usize % 1024) & !3;
                // SAFETY: inside the test memory, 4-byte aligned; atomics so the test itself is
                // sound while both threads touch it.
                let w = unsafe { &*((base + at) as *const AtomicU32) };
                w.store(x, Ordering::Relaxed);
            }
        });
        let mut out = [0u8; 16];
        let mut broken = 0;
        for _ in 0..200_000 {
            match consumer(m.ptr(), 4096) {
                Ok(mut c) => {
                    for _ in 0..8 {
                        if let Err(RingError::Broken) = c.pop(&mut out) {
                            broken += 1;
                            break;
                        }
                    }
                }
                Err(_) => broken += 1,
            }
        }
        stop.store(true, Ordering::Relaxed);
        vandal.join().unwrap();
        assert!(broken > 0, "the vandal should have broken the ring at least once");
    }

    /// A real producer and consumer on two threads, with the doorbell flags.
    #[test]
    fn two_threads() {
        let m = Arc::new(Mem::new(1024));
        ring(m.ptr(), 1024, Kind::Entries, 8).unwrap();
        let base = m.ptr() as usize;
        let m2 = m.clone();
        let t = std::thread::spawn(move || {
            let _keep = m2;
            let mut p = producer(base as *mut u8, 1024).unwrap();
            for i in 0..100_000u64 {
                loop {
                    match p.push(&i.to_le_bytes()) {
                        Ok(_) => break,
                        Err(RingError::WouldBlock) => std::thread::yield_now(),
                        Err(e) => panic!("{e:?}"),
                    }
                }
            }
        });
        let mut c = consumer(m.ptr(), 1024).unwrap();
        let mut out = [0u8; 8];
        let mut next = 0u64;
        while next < 100_000 {
            match c.pop(&mut out) {
                Ok(_) => {
                    assert_eq!(u64::from_le_bytes(out), next);
                    next += 1;
                }
                Err(RingError::WouldBlock) => std::thread::yield_now(),
                Err(e) => panic!("{e:?}"),
            }
        }
        t.join().unwrap();
    }
}
