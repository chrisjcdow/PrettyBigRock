// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

//! The code generated for `interfaces/pbr.test.echo.pbi`, tested on the host: every message
//! survives a round trip, the bytes are exactly what note 0008 section 8 says, and damaged
//! messages are refused without panicking. (The program self-test then runs the same code
//! between two programs on PrettyBigRock.)

use granite::Handle;
use granite::interface::{Decoder, Encoded, Malformed};
use granite::memory::Memory;
use granite::notification::Notification;
use pbr_interfaces::pbr_test_echo::{self as echo, EchoError, Flags, Item, Pair, Shape};

/// A decoder over an encoded message, taking its capabilities as if they had arrived.
fn decoder(m: &Encoded) -> Decoder<'_> {
    let (fixed, var, caps) = m;
    let mut arrived: [Option<Handle>; 4] = Default::default();
    for (slot, c) in arrived.iter_mut().zip(caps) {
        *slot = Some(Handle::from_raw(c.handle));
    }
    Decoder::new(fixed, var, arrived)
}

fn pair(left: i16, right: Option<&str>) -> Pair {
    Pair { left, right: right.map(String::from) }
}

#[test]
fn numbers_and_labels() {
    assert_eq!(echo::NAME, "pbr.test.echo");
    assert_eq!(echo::VERSION, 1);
    assert_eq!(echo::reverse::LABEL, 0x1_0001);
    assert_eq!(echo::fail::LABEL, 0x1_0006);
    // reverse: one reference; up to 200 bytes of text.
    assert_eq!((echo::reverse::REQUEST_FIXED, echo::reverse::REQUEST_BUFFER), (8, 200));
    // total: values (8) + items (8) + min (2), padded to 4; the buffer holds 64 values, then
    // 16 items of 20 bytes, each with a name (32) and 4 tags (8).
    assert_eq!(echo::total::REQUEST_FIXED, 20);
    assert_eq!(echo::total::REQUEST_BUFFER, 512 + 320 + 16 * (32 + 8));
    assert_eq!(echo::REQUEST_BUFFER, echo::total::REQUEST_BUFFER);
    assert_eq!((echo::pass::REQUEST_CAPS, echo::pass::REPLY_CAPS), (2, 1));
    assert_eq!(echo::MAX_NAME, 32);
}

/// The exact bytes of one request, worked out by hand from note 0008, section 8.
#[test]
fn bytes_on_the_wire() {
    let (fixed, var, caps) =
        echo::swap::encode_request(&pair(-2, Some("hi")), Flags::LOUD | Flags::TWICE);
    assert!(caps.is_empty());
    #[rustfmt::skip]
    let want_fixed = [
        0xFE, 0xFF,             // left: -2 as i16
        0, 0,                   // padding: an option starts at its value's alignment (4)
        1,                      // right: present
        0, 0, 0,                // padding to 4
        0, 0, 0, 0, 2, 0, 0, 0, // right's value: a reference, offset 0, length 2
        0b1001,                 // flags: Loud (bit 0) and Twice (bit 3)
        0, 0, 0,                // padding to the parameters' alignment of 4
    ];
    assert_eq!(fixed, want_fixed);
    assert_eq!(var, b"hi");

    let (fixed, var, _) = echo::total::encode_request(&[1, -1], &[], 7);
    #[rustfmt::skip]
    let want_fixed = [
        0, 0, 0, 0, 16, 0, 0, 0, // values: offset 0, 16 bytes
        16, 0, 0, 0, 0, 0, 0, 0, // items: offset 16, empty
        7, 0,                    // min
        0, 0,                    // padding to 4
    ];
    assert_eq!(fixed, want_fixed);
    let mut want_var = 1i64.to_le_bytes().to_vec();
    want_var.extend((-1i64).to_le_bytes());
    assert_eq!(var, want_var);
}

#[test]
fn round_trips() {
    // Strings, including ones that use all of their maximum.
    for text in ["", "héllo", &"x".repeat(200)] {
        let m = echo::reverse::encode_request(text);
        assert_eq!(echo::reverse::decode_request(decoder(&m)).unwrap().text, text);
        let m = echo::reverse::encode_reply(text);
        assert_eq!(echo::reverse::decode_reply(decoder(&m)).unwrap(), text);
    }

    let m = echo::add::encode_request(u32::MAX, 7);
    let r = echo::add::decode_request(decoder(&m)).unwrap();
    assert_eq!((r.a, r.b), (u32::MAX, 7));

    for (p, f) in [(pair(i16::MIN, None), Flags::default()), (pair(5, Some("right")), Flags::ALL)] {
        let m = echo::swap::encode_request(&p, f);
        let r = echo::swap::decode_request(decoder(&m)).unwrap();
        assert_eq!((r.pair, r.flags), (p.clone(), f));
        let m = echo::swap::encode_reply(&p, f, Shape::LeftOnly);
        let r = echo::swap::decode_reply(decoder(&m)).unwrap();
        assert_eq!((r.pair, r.flags, r.shape), (p, f, Shape::LeftOnly));
    }

    let items = vec![
        Item { name: "a".repeat(32), weight: 255, tags: vec![1, 2, 3, 4] },
        Item { name: String::new(), weight: 0, tags: vec![] },
    ];
    let values: Vec<i64> = (0..64).map(|i| i * -1_000_000_007).collect();
    let m = echo::total::encode_request(&values, &items, u16::MAX);
    assert!(m.1.len() <= echo::total::REQUEST_BUFFER);
    let r = echo::total::decode_request(decoder(&m)).unwrap();
    assert_eq!((r.values, r.items, r.min), (values, items, u16::MAX));
    let long: Vec<String> = (0..16).map(|i| "n".repeat(i * 2)).collect();
    let m = echo::total::encode_reply(i64::MIN, &long, &[[1, 2, 3], [4, 5, 6]]);
    let r = echo::total::decode_reply(decoder(&m)).unwrap();
    assert_eq!((r.sum, r.long_names, r.grid), (i64::MIN, long, [[1, 2, 3], [4, 5, 6]]));

    let m = echo::fail::encode_request();
    assert_eq!(m.0.len(), 0);
    echo::fail::decode_request(decoder(&m)).unwrap();
}

#[test]
fn handles() {
    let m = echo::pass::encode_request(
        3,
        Notification(Handle::from_raw(0x10_0005)),
        Some(Memory(Handle::from_raw(0x20_0009))),
    );
    let raw: Vec<(u32, bool)> = m.2.iter().map(|c| (c.handle, c.moving)).collect();
    assert_eq!(raw, [(0x10_0005, true), (0x20_0009, true)]);
    // bits (8), note's index 0 (4), the option's tag and padding (4), its index 1 (4), then
    // padding to 8.
    assert_eq!(&m.0[8..20], &[0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]);
    let r = echo::pass::decode_request(decoder(&m)).unwrap();
    assert_eq!(r.bits, 3);
    assert_eq!(r.note.0.into_raw(), 0x10_0005);
    assert_eq!(r.memory.unwrap().0.into_raw(), 0x20_0009);

    // An absent handle is all zeros, and takes no capability.
    let m = echo::pass::encode_request(0, Notification(Handle::from_raw(7)), None);
    assert_eq!(m.2.len(), 1);
    assert!(m.0[12..20].iter().all(|&b| b == 0));
    let r = echo::pass::decode_request(decoder(&m)).unwrap();
    assert!(r.memory.is_none());
    let _ = r.note.0.into_raw();

    // A capability too many, or one missing, is malformed.
    let (fixed, var, _) = echo::pass::encode_request(0, Notification(Handle::from_raw(7)), None);
    let extra = [Some(Handle::from_raw(7)), Some(Handle::from_raw(8)), None, None];
    assert_eq!(
        echo::pass::decode_request(Decoder::new(&fixed, &var, extra)).err(),
        Some(Malformed)
    );
    assert_eq!(
        echo::pass::decode_request(Decoder::new(&fixed, &var, Default::default())).err(),
        Some(Malformed)
    );
}

#[test]
fn errors_and_enums() {
    assert_eq!(u32::from(EchoError::Overflow), 1);
    assert_eq!(EchoError::from_u32(1), Some(EchoError::Overflow));
    // A flexible enum keeps values it doesn't know; a strict one refuses them.
    assert_eq!(EchoError::from_u32(99), Some(EchoError::Unknown(99)));
    assert_eq!(Shape::from_u32(1), Some(Shape::LeftOnly));
    assert_eq!(Shape::from_u32(2), None);
    let mut m = echo::swap::encode_reply(&pair(0, None), Flags::default(), Shape::Whole);
    let at = m.0.len() - 4;
    m.0[at] = 2;
    assert_eq!(echo::swap::decode_reply(decoder(&m)).err(), Some(Malformed));
    assert!(Flags::ALL.contains(Flags::TWICE) && !Flags::LOUD.contains(Flags::ALL));
}

/// Changing any single byte of a valid message either gives a message that's refused, or one
/// whose values re-encode to a message that decodes to the same values again. (Receivers
/// don't insist strings sit exactly where an encoder would put them: a reference anywhere
/// inside the buffer is accepted.) Nothing panics.
#[test]
fn damaged_messages() {
    let items = [Item { name: "nm".into(), weight: 9, tags: vec![7, 8] }];
    let messages: Vec<(&str, Encoded)> = vec![
        ("swap", echo::swap::encode_request(&pair(1, Some("ab")), Flags::LOUD)),
        ("total", echo::total::encode_request(&[5, 6], &items, 1)),
        ("reverse", echo::reverse::encode_request("é")),
    ];
    for (op, m) in &messages {
        for part in 0..2 {
            let len = if part == 0 { m.0.len() } else { m.1.len() };
            for i in 0..len {
                for v in [0u8, 1, 2, 0x7F, 0x80, 0xFF] {
                    let mut d = (m.0.clone(), m.1.clone(), m.2.clone());
                    let b = if part == 0 { &mut d.0[i] } else { &mut d.1[i] };
                    if *b == v {
                        continue;
                    }
                    *b = v;
                    let reencode = |d: &Encoded| match *op {
                        "swap" => echo::swap::decode_request(decoder(d))
                            .map(|r| echo::swap::encode_request(&r.pair, r.flags)),
                        "total" => echo::total::decode_request(decoder(d))
                            .map(|r| echo::total::encode_request(&r.values, &r.items, r.min)),
                        _ => echo::reverse::decode_request(decoder(d))
                            .map(|r| echo::reverse::encode_request(&r.text)),
                    };
                    if let Ok(e) = reencode(&d) {
                        let twice = reencode(&e).expect("a re-encoded message decodes");
                        assert_eq!(
                            (&twice.0, &twice.1),
                            (&e.0, &e.1),
                            "{op}: byte {i} of part {part} = {v}"
                        );
                    }
                }
            }
        }
    }
    // Wrong sizes.
    let (fixed, var, _) = echo::add::encode_request(1, 2);
    for n in [0, 4, 7, 9] {
        let mut f = fixed.clone();
        f.resize(n, 0);
        assert_eq!(
            echo::add::decode_request(Decoder::new(&f, &var, Default::default())).err(),
            Some(Malformed)
        );
    }
}

/// Random bytes never panic any decoder.
#[test]
fn random_messages() {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    for _ in 0..3000 {
        let mut fixed = vec![0u8; (next() % 30) as usize];
        let mut var = vec![0u8; (next() % 80) as usize];
        for b in fixed.iter_mut().chain(var.iter_mut()) {
            let r = next();
            *b = if r & 3 == 0 { (r >> 8) as u8 } else { (r >> 8) as u8 & 0x7 };
        }
        let d = || Decoder::new(&fixed, &var, Default::default());
        let _ = echo::reverse::decode_request(d());
        let _ = echo::swap::decode_request(d());
        let _ = echo::swap::decode_reply(d());
        let _ = echo::total::decode_request(d());
        let _ = echo::total::decode_reply(d());
        let _ = echo::pass::decode_request(d());
        let _ = echo::add::decode_reply(d());
    }
}
