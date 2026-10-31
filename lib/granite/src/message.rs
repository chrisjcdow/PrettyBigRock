// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Messages and the message page (docs/design/0008-messages-and-interfaces.md, section 3).
//!
//! Each thread has a message page. [`Outgoing`] is what to send; it's written into the page
//! just before a call or reply. [`Incoming`] is a copy of what arrived, taken from the page
//! straight after.

use crate::handle::Handle;
use crate::sys::{Error, Result};

/// Bytes of data a message carries in its page.
pub const DATA_MAX: usize = 240;
/// The most capabilities in one message.
pub const CAPS_MAX: usize = 4;
/// The largest attached buffer.
pub const BUFFER_MAX: usize = 65536;

/// A capability to send.
#[derive(Clone, Copy, Debug)]
pub struct CapSend {
    /// The handle.
    pub handle: u32,
    /// Rights to keep (the rest are dropped on the way).
    pub mask: u32,
    /// Moved (the sender loses it) rather than copied.
    pub moving: bool,
}

impl CapSend {
    /// Sends a copy of `h` with only the rights in `mask`. The sender keeps `h`.
    pub fn copy(h: &Handle, mask: u32) -> CapSend {
        CapSend { handle: h.raw(), mask, moving: false }
    }

    /// Moves `h` to the receiver, with only the rights in `mask`.
    pub fn moved(h: Handle, mask: u32) -> CapSend {
        CapSend { handle: h.into_raw(), mask, moving: true }
    }
}

/// A message to send.
#[derive(Clone, Copy, Debug, Default)]
pub struct Outgoing<'a> {
    /// Which operation it is, or a reply's status.
    pub label: u32,
    /// Up to 240 bytes of data.
    pub data: &'a [u8],
    /// Up to 4 capabilities.
    pub caps: &'a [CapSend],
}

impl<'a> Outgoing<'a> {
    /// A message with just a label and data.
    pub fn new(label: u32, data: &'a [u8]) -> Self {
        Outgoing { label, data, caps: &[] }
    }
}

/// A message that arrived. Capabilities it brought are closed when it's dropped, unless taken
/// with [`Incoming::take_cap`].
#[derive(Debug)]
pub struct Incoming {
    /// Which operation it is, or a reply's status.
    pub label: u32,
    data: [u8; DATA_MAX],
    len: usize,
    caps: [u32; CAPS_MAX],
    rights: [u32; CAPS_MAX],
    count: usize,
    /// The badge of the capability the sender used (0 for replies).
    pub badge: u64,
    /// How many bytes of attached buffer arrived.
    pub buffer_len: usize,
}

impl Incoming {
    /// The data.
    pub fn data(&self) -> &[u8] {
        &self.data[..self.len]
    }

    /// How many capabilities came with it.
    pub fn cap_count(&self) -> usize {
        self.count
    }

    /// The rights capability `i` arrived with.
    pub fn cap_rights(&self, i: usize) -> u32 {
        if i < self.count { self.rights[i] } else { 0 }
    }

    /// Takes charge of capability `i`.
    pub fn take_cap(&mut self, i: usize) -> Option<Handle> {
        if i >= self.count || self.caps[i] == 0 {
            return None;
        }
        let h = self.caps[i];
        self.caps[i] = 0;
        Some(Handle::from_raw(h))
    }
}

impl Drop for Incoming {
    fn drop(&mut self) {
        for i in 0..self.count {
            if self.caps[i] != 0 {
                drop(Handle::from_raw(self.caps[i]));
            }
        }
    }
}

fn put(page: *mut u8, at: usize, bytes: &[u8]) {
    // SAFETY: `page` is this thread's message page (4 KiB), and `at + bytes.len()` stays
    // within the 304 bytes the kernel uses.
    unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), page.add(at), bytes.len()) };
}

fn get<const N: usize>(page: *const u8, at: usize) -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: as for `put`.
    unsafe { core::ptr::copy_nonoverlapping(page.add(at), b.as_mut_ptr(), N) };
    b
}

/// Writes `m` into the message page `page`, with an attached buffer of `send` bytes from
/// `buffer` and room for `buffer.len()` bytes to come back into it.
pub(crate) fn write(
    page: *mut u8,
    m: &Outgoing<'_>,
    buffer: *const u8,
    send: usize,
    capacity: usize,
) -> Result<()> {
    if m.data.len() > DATA_MAX || m.caps.len() > CAPS_MAX {
        return Err(Error::InvalidArgument);
    }
    if send > BUFFER_MAX {
        return Err(Error::TooBig);
    }
    put(page, 0, &m.label.to_le_bytes());
    put(page, 4, &(m.data.len() as u16).to_le_bytes());
    let mut moves = 0u8;
    for (i, c) in m.caps.iter().enumerate() {
        if c.moving {
            moves |= 1 << i;
        }
    }
    put(page, 6, &[m.caps.len() as u8, moves]);
    for i in 0..CAPS_MAX {
        let (h, mask) = m.caps.get(i).map_or((0, 0), |c| (c.handle, c.mask));
        put(page, 8 + 4 * i, &h.to_le_bytes());
        put(page, 24 + 4 * i, &mask.to_le_bytes());
    }
    put(page, 40, &0u64.to_le_bytes());
    put(page, 48, &(buffer as u64).to_le_bytes());
    put(page, 56, &(send as u32).to_le_bytes());
    put(page, 60, &(capacity.min(BUFFER_MAX) as u32).to_le_bytes());
    put(page, 64, m.data);
    Ok(())
}

/// Sets only the receive buffer fields, before waiting for a message.
pub(crate) fn set_receive_buffer(page: *mut u8, buffer: *mut u8, capacity: usize) {
    put(page, 48, &(buffer as u64).to_le_bytes());
    put(page, 56, &0u32.to_le_bytes());
    put(page, 60, &(capacity.min(BUFFER_MAX) as u32).to_le_bytes());
}

/// Copies what arrived out of the message page.
pub(crate) fn read(page: *const u8) -> Incoming {
    let label = u32::from_le_bytes(get(page, 0));
    let len = (u16::from_le_bytes(get(page, 4)) as usize).min(DATA_MAX);
    let [count, _] = get::<2>(page, 6);
    let count = (count as usize).min(CAPS_MAX);
    let mut caps = [0u32; CAPS_MAX];
    let mut rights = [0u32; CAPS_MAX];
    for i in 0..count {
        caps[i] = u32::from_le_bytes(get(page, 8 + 4 * i));
        rights[i] = u32::from_le_bytes(get(page, 24 + 4 * i));
    }
    let badge = u64::from_le_bytes(get(page, 40));
    let buffer_len = u32::from_le_bytes(get(page, 56)) as usize;
    let mut data = [0u8; DATA_MAX];
    data[..len].copy_from_slice(&get::<DATA_MAX>(page, 64)[..len]);
    Incoming { label, data, len, caps, rights, count, badge, buffer_len }
}
