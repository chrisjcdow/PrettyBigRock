// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Looking at the guest's screens, for the display self-test
//! (docs/design/0020-screens-and-virtio-gpu.md, section 7).
//!
//! The guest draws a known picture on every screen and prints which (see
//! `services/root/src/displaytest.rs`). The runner asks QEMU for a screenshot of each screen,
//! which QEMU writes as a PPM file: a short text header (`P6`, the width, the height, the
//! largest colour value) and then three bytes a pixel, red, green and blue. This file reads
//! those and compares them with the picture, which it works out with the same code the guest
//! drew it with (`pbr_devices::display::test`).

use pbr_devices::display::test;

/// A screen the guest says it drew on, from its line in the log.
#[derive(Debug, PartialEq, Eq)]
pub struct Shown {
    /// The device's name as the device manager lists it (it starts with the driver's name).
    pub device: String,
    /// Which of the device's screens.
    pub screen: u32,
    /// Pixels across.
    pub width: u32,
    /// Pixels down.
    pub height: u32,
}

/// The display devices the guest has asked the runner to look at so far for picture `number`,
/// in order. (`text` is the kernel log.)
pub fn asked(text: &str, number: u32) -> Vec<String> {
    let start = format!("Device self-test: look at the pictures ({number} on ");
    text.lines()
        .filter_map(|l| l.split_once(&start))
        .filter_map(|(_, rest)| rest.strip_suffix(')'))
        .map(String::from)
        .collect()
}

/// The screens of `device` the guest says show picture `number`: its lines
/// `Device self-test: picture N on DEVICE screen S: WxH`.
pub fn shown(text: &str, number: u32, device: &str) -> Vec<Shown> {
    let start = format!("Device self-test: picture {number} on {device} screen ");
    let parse = |rest: &str| -> Option<Shown> {
        let (screen, size) = rest.split_once(": ")?;
        let (width, height) = size.trim().split_once('x')?;
        Some(Shown {
            device: device.to_string(),
            screen: screen.parse().ok()?,
            width: width.parse().ok()?,
            height: height.parse().ok()?,
        })
    };
    text.lines().filter_map(|l| l.split_once(&start)).filter_map(|(_, rest)| parse(rest)).collect()
}

/// A screenshot: its size and its pixels, each `0xRRGGBB`.
#[derive(Debug, PartialEq, Eq)]
pub struct Shot {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

/// Reads a binary PPM file with 8 bits a colour, as QEMU's `screendump` writes.
pub fn read_ppm(file: &[u8]) -> Result<Shot, String> {
    // The header is four words separated by white space (and may have `#` comments), then
    // exactly one white-space byte, then the pixels.
    let mut at = 0;
    let mut word = || -> Option<&[u8]> {
        loop {
            while file.get(at)?.is_ascii_whitespace() {
                at += 1;
            }
            if file[at] != b'#' {
                break;
            }
            while *file.get(at)? != b'\n' {
                at += 1;
            }
        }
        let start = at;
        while !file.get(at)?.is_ascii_whitespace() {
            at += 1;
        }
        Some(&file[start..at])
    };
    let number = |w: Option<&[u8]>| -> Option<u32> { std::str::from_utf8(w?).ok()?.parse().ok() };
    if word() != Some(b"P6") {
        return Err("not a PPM picture (it doesn't start with P6)".into());
    }
    let (width, height, max) = (number(word()), number(word()), number(word()));
    let (Some(width), Some(height), Some(255)) = (width, height, max) else {
        return Err("a PPM header this can't read".into());
    };
    let count = u64::from(width) * u64::from(height);
    let body = file.get(at + 1..).unwrap_or_default();
    if count > 64 << 20 || (body.len() as u64) < count * 3 {
        return Err(format!("a picture of {width}x{height} with only {} bytes", body.len()));
    }
    let pixels = body
        .chunks_exact(3)
        .take(count as usize)
        .map(|p| u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
        .collect();
    Ok(Shot { width, height, pixels })
}

/// Compares a screenshot with picture `number` on a screen of the size the guest gave.
/// `Err` says how they differ.
pub fn compare(shot: &Shot, number: u32, want: &Shown) -> Result<(), String> {
    if (shot.width, shot.height) != (want.width, want.height) {
        return Err(format!(
            "the screenshot is {}x{}, the guest says the screen is {}x{}",
            shot.width, shot.height, want.width, want.height
        ));
    }
    let mut wrong = 0u64;
    let mut first = None;
    for y in 0..shot.height {
        for x in 0..shot.width {
            let got = shot.pixels[(y * shot.width + x) as usize];
            let expected = test::picture(number, shot.width, shot.height, x, y);
            if got != expected {
                wrong += 1;
                first.get_or_insert((x, y, got, expected));
            }
        }
    }
    match first {
        None => Ok(()),
        Some((x, y, got, expected)) => Err(format!(
            "{wrong} of {} pixels are wrong; the first is ({x}, {y}): {got:#08x}, expected \
             {expected:#08x}",
            shot.pixels.len()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ppm(width: u32, height: u32, pixel: impl Fn(u32, u32) -> u32) -> Vec<u8> {
        let mut file = format!("P6\n{width} {height}\n255\n").into_bytes();
        for y in 0..height {
            for x in 0..width {
                let p = pixel(x, y);
                file.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
        file
    }

    #[test]
    fn reads_the_guests_lines() {
        let text = "[root] Device self-test: picture 1 on virtio-gpu 00:05.0 screen 0: 1280x800\n\
                    [root] Device self-test: picture 1 on virtio-gpu 00:05.0 screen 1: 640x480\n\
                    [root] Device self-test: picture 2 on virtio-gpu 00:05.0 screen 0: 1280x800\n\
                    [root] Device self-test: picture 1 on framebuffer screen 0: 800x600\n\
                    [root] Device self-test: picture 1 on framebuffer screen x: 800x600\n\
                    [root] Device self-test: look at the pictures (1 on virtio-gpu 00:05.0)\n\
                    [root] Device self-test: look at the pictures (1 on framebuffer)\n";
        assert_eq!(asked(text, 1), ["virtio-gpu 00:05.0", "framebuffer"]);
        assert!(asked(text, 2).is_empty());
        let s = shown(text, 1, "virtio-gpu 00:05.0");
        assert_eq!(s.len(), 2);
        assert_eq!((s[1].screen, s[1].width, s[1].height), (1, 640, 480));
        assert_eq!(shown(text, 2, "virtio-gpu 00:05.0").len(), 1);
        // The line with a screen that isn't a number is left out.
        assert_eq!(shown(text, 1, "framebuffer").len(), 1);
    }

    #[test]
    fn reads_and_compares_screenshots() {
        let want = Shown { device: "d".into(), screen: 0, width: 64, height: 40 };
        for number in 1..=3 {
            let file = ppm(64, 40, |x, y| test::picture(number, 64, 40, x, y));
            let shot = read_ppm(&file).unwrap();
            assert_eq!((shot.width, shot.height, shot.pixels.len()), (64, 40, 64 * 40));
            compare(&shot, number, &want).unwrap();
            // It is not any other picture.
            for other in (1..=3).filter(|o| *o != number) {
                assert!(compare(&shot, other, &want).is_err());
            }
        }
        // One wrong pixel is found and named.
        let file =
            ppm(
                64,
                40,
                |x, y| {
                    if (x, y) == (5, 7) { 0x12_3456 } else { test::picture(1, 64, 40, x, y) }
                },
            );
        let e = compare(&read_ppm(&file).unwrap(), 1, &want).unwrap_err();
        assert!(e.contains("1 of 2560") && e.contains("(5, 7)") && e.contains("0x123456"), "{e}");
        // The wrong size.
        let small = read_ppm(&ppm(32, 40, |_, _| 0)).unwrap();
        assert!(compare(&small, 1, &want).unwrap_err().contains("32x40"));
    }

    #[test]
    fn refuses_files_that_are_not_pictures() {
        assert!(read_ppm(b"").is_err());
        assert!(read_ppm(b"P5\n1 1\n255\n\0").is_err());
        assert!(read_ppm(b"P6\n1 1\n65535\n\0\0\0\0\0\0").is_err());
        assert!(read_ppm(b"P6\n2 2\n255\n\0\0\0").is_err(), "too few pixels");
        assert!(read_ppm(b"P6\n4000000000 4000000000\n255\n").is_err());
        // Comments are allowed in the header.
        let shot = read_ppm(b"P6\n# made by a test\n1 1\n255\n\x01\x02\x03").unwrap();
        assert_eq!(shot.pixels, [0x01_0203]);
        let good = ppm(3, 2, |x, y| x + y);
        for cut in 0..good.len() {
            let _ = read_ppm(&good[..cut]); // must not panic, whatever is missing
        }
    }
}
