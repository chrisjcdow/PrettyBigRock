// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Plugging a screen in and out of QEMU's virtio-gpu, for the display self-test
//! (docs/design/0020-screens-and-virtio-gpu.md, section 7).
//!
//! QEMU has no command for "plug a monitor into output 2". An output of its virtio-gpu counts
//! as connected when a viewer has told QEMU what size it wants that output to be, and the only
//! viewer that works without a window is a VNC client (VNC is a protocol for looking at a
//! machine's screen over a connection; its messages are the RFB protocol, RFC 6143). So the
//! runner has QEMU listen for VNC on a socket for the second output, and this file is just
//! enough of a client to connect and send the one message that matters, "set the desktop size":
//! a size plugs the screen in, and 0 by 0 unplugs it. Each time, QEMU tells the guest that its
//! screens changed. Nothing QEMU sends back is looked at, and no picture is ever asked for.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// A connection to QEMU's VNC server for one screen. The screen stays as it was last set for
/// as long as this is kept (and afterwards: QEMU doesn't unplug it when the viewer goes).
pub struct Vnc {
    stream: UnixStream,
}

impl Vnc {
    /// Connects to the VNC server listening on the Unix socket `path` and gets as far as being
    /// allowed to send messages.
    pub fn connect(path: &Path) -> Result<Vnc, String> {
        let stream =
            UnixStream::connect(path).map_err(|e| format!("VNC: connecting to {}: {e}", path.display()))?;
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let mut vnc = Vnc { stream };
        // The server says which version of the protocol it speaks; we answer 3.8.
        let version = vnc.read(12)?;
        if !version.starts_with(b"RFB ") {
            return Err("VNC: the server didn't say RFB".into());
        }
        vnc.write(b"RFB 003.008\n")?;
        // It lists the ways of proving who we are that it accepts. 1 is "none": the socket is
        // a file only this user can reach, so the runner asks for nothing more.
        let count = vnc.read(1)?[0];
        if count == 0 {
            return Err("VNC: the server refused the connection".into());
        }
        if !vnc.read(usize::from(count))?.contains(&1) {
            return Err("VNC: the server wants a password".into());
        }
        vnc.write(&[1])?;
        if vnc.read(4)? != [0, 0, 0, 0] {
            return Err("VNC: the server didn't let us in".into());
        }
        // "Others may look too", then the server describes its screen: 20 bytes, a length, and
        // a name of that length.
        vnc.write(&[1])?;
        let init = vnc.read(24)?;
        let name = u32::from_be_bytes([init[20], init[21], init[22], init[23]]);
        if name > 4096 {
            return Err("VNC: the server's name is absurdly long".into());
        }
        vnc.read(name as usize)?;
        // Say we understand being told about, and asking for, a change of size (the
        // "extended desktop size" encoding, number -308): servers ignore the request otherwise.
        let mut encodings = vec![2, 0, 0, 1];
        encodings.extend((-308i32).to_be_bytes());
        vnc.write(&encodings)?;
        Ok(vnc)
    }

    /// Asks for the screen to be `width` by `height` pixels: for QEMU's virtio-gpu, plugs a
    /// screen of that size in, or with 0 by 0 unplugs it.
    pub fn set_size(&mut self, width: u16, height: u16) -> Result<(), String> {
        // Message 251 (SetDesktopSize): a padding byte, the size, how many screens (one), a
        // padding byte, then that screen: its id, where it is, its size and its flags.
        let mut m = vec![251, 0];
        m.extend(width.to_be_bytes());
        m.extend(height.to_be_bytes());
        m.extend([1, 0]);
        m.extend(0u32.to_be_bytes());
        m.extend([0, 0, 0, 0]);
        m.extend(width.to_be_bytes());
        m.extend(height.to_be_bytes());
        m.extend(0u32.to_be_bytes());
        self.write(&m)
    }

    fn read(&mut self, n: usize) -> Result<Vec<u8>, String> {
        let mut bytes = vec![0; n];
        self.stream.read_exact(&mut bytes).map_err(|e| format!("VNC: reading: {e}"))?;
        Ok(bytes)
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.stream.write_all(bytes).map_err(|e| format!("VNC: writing: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    /// A pretend server: plays its side of the handshake, then returns everything the client
    /// sent after it.
    fn serve(listener: UnixListener, security: &'static [u8], expect: usize) -> Vec<u8> {
        let (mut s, _) = listener.accept().unwrap();
        s.write_all(b"RFB 003.008\n").unwrap();
        let mut version = [0; 12];
        s.read_exact(&mut version).unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        s.write_all(security).unwrap();
        if !security[1..].contains(&1) {
            return Vec::new();
        }
        let mut one = [0; 1];
        s.read_exact(&mut one).unwrap();
        assert_eq!(one, [1], "the client chooses no authentication");
        s.write_all(&[0, 0, 0, 0]).unwrap();
        s.read_exact(&mut one).unwrap();
        let mut init = vec![0u8; 20];
        init.extend(4u32.to_be_bytes());
        init.extend(b"QEMU");
        s.write_all(&init).unwrap();
        let mut rest = vec![0; expect];
        s.read_exact(&mut rest).unwrap();
        rest
    }

    fn socket(name: &str) -> (std::path::PathBuf, UnixListener) {
        let path = std::env::temp_dir().join(format!("pbr-vnc-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        (path, listener)
    }

    #[test]
    fn connects_and_asks_for_a_size() {
        let (path, listener) = socket("size");
        let server = std::thread::spawn(move || serve(listener, &[2, 2, 1], 8 + 24 + 24));
        let mut vnc = Vnc::connect(&path).unwrap();
        vnc.set_size(1024, 768).unwrap();
        vnc.set_size(0, 0).unwrap();
        let sent = server.join().unwrap();
        let _ = std::fs::remove_file(&path);
        // SetEncodings with the one encoding, -308.
        assert_eq!(sent[..8], [2, 0, 0, 1, 0xFF, 0xFF, 0xFE, 0xCC]);
        // SetDesktopSize 1024x768 with one screen of that size at (0, 0).
        let size = &sent[8..32];
        assert_eq!(size[..8], [251, 0, 4, 0, 3, 0, 1, 0]);
        assert_eq!(size[8..], [0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 3, 0, 0, 0, 0, 0]);
        // And 0x0.
        assert_eq!(sent[32..40], [251, 0, 0, 0, 0, 0, 1, 0]);
    }

    #[test]
    fn refuses_a_server_that_wants_a_password() {
        let (path, listener) = socket("password");
        let server = std::thread::spawn(move || serve(listener, &[1, 2], 0));
        let e = Vnc::connect(&path).err().unwrap();
        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(e.contains("password"), "{e}");
    }
}
