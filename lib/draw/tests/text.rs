// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Drawing text: against a slow model written straight from the rules, measuring, clipping,
//! damage, hostile arguments, bounded time, and reference pictures at 1× and 2× (the font
//! sheet a person reviews the font with, and a sheet of styles).
//!
//! The model fills one logical-pixel block at a time for every dot of every character and
//! remembers which device pixels were covered, so a see-through colour is mixed once for each
//! pixel. It shares no code with the library.

mod common;

use common::{PAPER, Sheet};
use pbr_draw::{Canvas, Colour, Damage, Rect, TextStyle};
use std::time::Instant;

const MAX: i32 = i32::MAX;
const MIN: i32 = i32::MIN;

/// The characters the font lacks, that tests use.
const MISSING: [char; 4] = ['\n', '\u{7f}', '中', '😀'];

struct Rng(u64);

impl Rng {
    fn below(&mut self, n: u64) -> i32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % n) as i32
    }
}

/// The dots of a character as the rules give them: row by row, which columns (of 5) are set.
/// The box for a missing character is the outline of rows 2 to 8.
fn dots(c: char) -> Vec<[bool; 5]> {
    let mut rows = vec![[false; 5]; pbr_font::HEIGHT];
    match pbr_font::lookup(c) {
        Some(glyph) => {
            for (r, bits) in glyph.iter().enumerate() {
                for (col, dot) in rows[r].iter_mut().enumerate() {
                    *dot = bits >> (4 - col) & 1 == 1;
                }
            }
        }
        None => {
            for (r, row) in rows.iter_mut().enumerate().take(9).skip(pbr_font::ABOVE) {
                for (col, dot) in row.iter_mut().enumerate() {
                    *dot = r == pbr_font::ABOVE || r == 8 || col == 0 || col == 4;
                }
            }
        }
    }
    rows
}

/// Draws `text` onto `bg` (`cw` × `ch` device pixels, at `scale`), the slow way.
#[allow(clippy::too_many_arguments)]
fn model_text(
    bg: &mut [u32],
    (cw, ch, scale): (i32, i32, i64),
    (x, y): (i32, i32),
    text: &str,
    colour: Colour,
    size: i64,
    bold: bool,
) {
    let mut covered = vec![false; (cw * ch) as usize];
    let advance = (6 + i64::from(bold)) * size;
    for (n, c) in text.chars().enumerate() {
        let cell_x = i64::from(x) + n as i64 * advance;
        for (r, row) in dots(c).iter().enumerate() {
            for (col, &set) in row.iter().enumerate() {
                if !set {
                    continue;
                }
                // The dot, and for bold its twin one dot to the right.
                for extra in 0..=i64::from(bold) {
                    let left = cell_x + (col as i64 + extra) * size;
                    let top = i64::from(y) + r as i64 * size;
                    for py in top * scale..(top + size) * scale {
                        for px in left * scale..(left + size) * scale {
                            if (0..i64::from(cw)).contains(&px) && (0..i64::from(ch)).contains(&py)
                            {
                                covered[(py * i64::from(cw) + px) as usize] = true;
                            }
                        }
                    }
                }
            }
        }
    }
    for (pixel, &hit) in bg.iter_mut().zip(&covered) {
        if hit {
            *pixel = colour.over(*pixel);
        }
    }
}

fn background(w: i32, h: i32) -> Vec<u32> {
    (0..w * h)
        .map(|n| {
            let (x, y) = ((n % w) as u32, (n / w) as u32);
            0xFF00_0000
                | (((x * 5 + y * 3) & 0xFF) << 16)
                | (((x * 7 + 40) & 0xFF) << 8)
                | ((y * 11) & 0xFF)
        })
        .collect()
}

fn draw(cw: i32, ch: i32, scale: u32, f: impl FnOnce(&mut Canvas)) -> Vec<u32> {
    let mut pixels = background(cw, ch);
    let canvas = Canvas::new(&mut pixels, cw as u32, ch as u32, cw as u32).unwrap();
    f(&mut canvas.with_scale(scale));
    pixels
}

fn assert_same(got: &[u32], want: &[u32], width: i32, what: &str) {
    if let Some(at) = got.iter().zip(want).position(|(g, w)| g != w) {
        let (x, y) = (at as i32 % width, at as i32 / width);
        panic!("{what}: at {x},{y} got {:#010x}, the model says {:#010x}", got[at], want[at]);
    }
}

#[test]
fn every_character_agrees_with_the_model() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let (cw, ch) = (100, 140);
    let chars: Vec<char> = pbr_font::characters().chain(MISSING).collect();
    assert!(chars.len() > 95, "the font has at least the printable ASCII characters");
    for (n, &c) in chars.iter().enumerate() {
        let text = c.to_string();
        for bold in [false, true] {
            for size in 1..=4u32 {
                let scale = 1 + (size as usize + n) as u32 % 3;
                // See-through, sometimes; each pixel is mixed once, even with bold's twin dots.
                let colour = match (n + size as usize) % 3 {
                    0 => Colour::solid(0x2f6b55),
                    1 => Colour(0x80FF_FFFF),
                    _ => Colour(0xC0D2_577A),
                };
                // Mostly on the canvas, sometimes partly off the edges.
                let (x, y) = (rng.below(24) - 6, rng.below(8) - 3);
                let got = draw(cw, ch, scale, |c| {
                    let end = c.text(x, y, &text, colour, TextStyle { size, bold });
                    assert_eq!(end, x + (6 + i32::from(bold)) * size as i32);
                });
                let mut want = background(cw, ch);
                let geometry = (cw, ch, i64::from(scale));
                model_text(&mut want, geometry, (x, y), &text, colour, size.into(), bold);
                assert_same(
                    &got,
                    &want,
                    cw,
                    &format!("{c:?} size {size} bold {bold} scale {scale}"),
                );
            }
        }
    }
}

#[test]
fn strings_agree_with_the_model_at_any_position() {
    let mut rng = Rng(0xDEAD_BEEF_CAFE_F00D);
    let (cw, ch) = (90, 70);
    let sentences =
        ["Hello, World!", "Ça été déjà vu \u{1}\u{4e2d}", "iiii WWWW ~~~~", "x", "a\nb\u{7f}c😀"];
    for round in 0..400usize {
        let text = sentences[round % sentences.len()];
        let size = 1 + rng.below(3) as u32;
        let bold = round % 2 == 0;
        let scale = 1 + (round / 2) as u32 % 3;
        let colour = if round % 5 == 0 { Colour(0x40FF_8800) } else { Colour::solid(0x102030) };
        let (x, y) = (rng.below(120) - 70, rng.below(40) - 14);
        let got = draw(cw, ch, scale, |c| {
            c.text(x, y, text, colour, TextStyle { size, bold });
        });
        let mut want = background(cw, ch);
        model_text(&mut want, (cw, ch, scale.into()), (x, y), text, colour, size.into(), bold);
        assert_same(
            &got,
            &want,
            cw,
            &format!("{text:?} at {x},{y} size {size} bold {bold} scale {scale}"),
        );
    }
}

#[test]
fn measuring() {
    assert_eq!(TextStyle::PLAIN, TextStyle { size: 1, bold: false });
    assert_eq!(TextStyle::BOLD, TextStyle { size: 1, bold: true });
    assert_eq!(TextStyle::PLAIN.advance(), 6);
    assert_eq!(TextStyle::BOLD.advance(), 7);
    assert_eq!(TextStyle::PLAIN.sized(2).advance(), 12);
    assert_eq!(TextStyle::BOLD.sized(3).advance(), 21);
    assert_eq!(TextStyle::BOLD.sized(3), TextStyle { size: 3, bold: true });
    assert_eq!(TextStyle::PLAIN.line_height(), 11);
    assert_eq!(TextStyle::BOLD.sized(2).line_height(), 22);
    assert_eq!(TextStyle::PLAIN.sized(3).line_height(), 33);
    // Characters are counted, not bytes.
    assert_eq!(TextStyle::PLAIN.width(""), 0);
    assert_eq!(TextStyle::PLAIN.width("abc"), 18);
    assert_eq!(TextStyle::PLAIN.width("é€中😀"), 24);
    assert_eq!(TextStyle::BOLD.sized(2).width("ab"), 28);
    // Sizes are brought within 1 to 16.
    for (size, kept) in [(0, 1), (1, 1), (16, 16), (17, 16), (1000, 16), (u32::MAX, 16)] {
        let style = TextStyle { size, bold: false };
        assert_eq!(style.advance(), 6 * kept, "{size}");
        assert_eq!(style.line_height(), 11 * kept, "{size}");
        assert_eq!(style.width("ab"), 12 * kept);
    }
    // A string too wide for an `i32` stops at `i32::MAX`.
    let long = "x".repeat(400_000_000);
    assert_eq!(TextStyle::PLAIN.sized(16).width(&long), MAX);
    // The answer of `text` is the x just past the text, stopping at the limit.
    let mut pixels = vec![0u32; 40 * 20];
    let mut canvas = Canvas::new(&mut pixels, 40, 20, 40).unwrap();
    let ink = Colour::solid(0xFFFFFF);
    assert_eq!(canvas.text(3, 2, "abc", ink, TextStyle::PLAIN), 21);
    assert_eq!(canvas.text(-30, 2, "abc", ink, TextStyle::BOLD), -9);
    assert_eq!(canvas.text(3, 2, "", ink, TextStyle::PLAIN), 3);
    assert_eq!(canvas.text(3, 500, "abc", ink, TextStyle::PLAIN), 21, "below the canvas");
    assert_eq!(canvas.text(3, 2, "abc", Colour::CLEAR, TextStyle::PLAIN), 21);
    assert_eq!(canvas.text(0, 2, "ab", ink, TextStyle::PLAIN.sized(0)), 12);
    assert_eq!(canvas.text(0, 2, "ab", ink, TextStyle::PLAIN.sized(1000)), 192);
    assert_eq!(canvas.text(MAX, 2, "abc", ink, TextStyle::PLAIN), MAX);
    assert_eq!(canvas.text(MAX - 5, 2, "abc", ink, TextStyle::BOLD), MAX);
    assert_eq!(canvas.text(MIN, 2, "abc", ink, TextStyle::PLAIN), MIN + 18);
    assert_eq!(canvas.text(MIN, MIN, "é", ink, TextStyle::PLAIN.sized(7)), MIN + 42);
    assert_eq!(canvas.text(100, 2, &long, ink, TextStyle::PLAIN.sized(16)), MAX);
}

#[test]
fn the_box_and_the_font_are_what_the_rules_say() {
    // Drawn plain at 1×: the box is rows 2 to 8, five dots wide.
    let mut sheet = Sheet::new(8, 12, Colour::BLACK);
    sheet.canvas().text(1, 0, "\u{7f}", Colour::WHITE, TextStyle::PLAIN);
    let rows: Vec<String> = sheet
        .pixels
        .chunks(8)
        .map(|r| r.iter().map(|&p| if p == Colour::WHITE.0 { '#' } else { '.' }).collect())
        .collect();
    assert_eq!(
        rows,
        [
            "........", "........", ".#####..", ".#...#..", ".#...#..", ".#...#..", ".#...#..",
            ".#...#..", ".#####..", "........", "........", "........"
        ]
    );
    // Bold doubles it: one dot wider, the sides two dots thick.
    let mut sheet = Sheet::new(8, 12, Colour::BLACK);
    sheet.canvas().text(1, 0, "\u{7f}", Colour::WHITE, TextStyle::BOLD);
    let row3: String = sheet.pixels[3 * 8..4 * 8]
        .iter()
        .map(|&p| if p == Colour::WHITE.0 { '#' } else { '.' })
        .collect();
    assert_eq!(row3, ".##..##.");
    // A character the font has is not the box.
    let mut a = Sheet::new(8, 12, Colour::BLACK);
    a.canvas().text(1, 0, "A", Colour::WHITE, TextStyle::PLAIN);
    assert_ne!(a.pixels, sheet.pixels);
    // The space draws nothing; a control character does.
    let mut s = Sheet::new(8, 12, Colour::BLACK);
    s.canvas().text(1, 0, " ", Colour::WHITE, TextStyle::PLAIN);
    assert!(s.pixels.iter().all(|&p| p == Colour::BLACK.0));
}

#[test]
fn clipping_and_sub_canvases_never_change_a_pixel() {
    let text = "Hello, Wörld! Ça été 😀 ~";
    for scale in [1u32, 2, 3] {
        for (size, bold) in [(1u32, false), (1, true), (2, false), (3, true)] {
            let style = TextStyle { size, bold };
            let colour = Colour(0xB0FF_E080);
            let (w, h) = (190, 120);
            let call = |c: &mut Canvas| {
                c.text(-7, 3, text, colour, style);
                c.text(5, 40, text, Colour::solid(0x123456), style);
                c.text(60, 60, text, colour, style);
            };
            let whole = draw(w, h, scale, call);
            let bg = background(w, h);
            let s = scale as i32;
            for clip in [
                Rect::new(0, 0, 500, 500),
                Rect::new(1, 2, 3, 4),
                Rect::new(9, 0, 1, 200), // one column through a glyph
                Rect::new(0, 8, 200, 1), // one row through a glyph
                Rect::new(20, 5, 7, 35),
                Rect::new(-10, -10, 30, 30),
                Rect::new(33, 0, 0, 100),
                Rect::new(40, 40, 9, 9),
                Rect::new(MAX, MAX, 1, 1),
            ] {
                let cut = draw(w, h, scale, |c| call(&mut c.clipped(clip)));
                for y in 0..h {
                    for x in 0..w {
                        let inside = !clip.is_empty()
                            && i64::from(x) >= i64::from(clip.x) * i64::from(s)
                            && i64::from(y) >= i64::from(clip.y) * i64::from(s)
                            && i64::from(x) < clip.right() * i64::from(s)
                            && i64::from(y) < clip.bottom() * i64::from(s);
                        let at = (y * w + x) as usize;
                        let want = if inside { whole[at] } else { bg[at] };
                        assert_eq!(
                            cut[at], want,
                            "{clip:?} size {size} bold {bold} scale {scale}: {x},{y}"
                        );
                    }
                }
            }
            // A sub-canvas is the same as the clip it makes, with its corner moved.
            let area = Rect::new(11, 7, 40, 25);
            let moved = draw(w, h, scale, |c| {
                c.sub(area).text(-3, 1, text, colour, style);
            });
            let direct = draw(w, h, scale, |c| {
                c.clipped(area).text(8, 8, text, colour, style);
            });
            assert_eq!(moved, direct, "size {size} bold {bold} scale {scale}");
            assert_ne!(moved, bg);
        }
    }
}

#[test]
fn skipping_to_the_first_visible_character_changes_nothing() {
    let unit = "ab é中W~ ";
    let many = unit.repeat(20_000);
    let skipped = 19_990usize;
    for (size, bold) in [(1u32, false), (2, true), (3, false)] {
        let style = TextStyle { size, bold };
        // A few characters before the first, which may show at the left.
        let tail: String = many.chars().skip(skipped - 4).collect();
        for shift in [0, 1, 5, 6, 7, 13] {
            let x = shift - skipped as i32 * style.advance();
            let long = draw(70, 40, 1, |c| {
                c.text(x, 3, &many, Colour(0x90FF_FFFF), style);
            });
            let short = draw(70, 40, 1, |c| {
                c.text(shift - 4 * style.advance(), 3, &tail, Colour(0x90FF_FFFF), style);
            });
            assert_eq!(long, short, "size {size} shift {shift}");
        }
    }
}

#[test]
fn damage_covers_every_changed_pixel_and_a_call_that_draws_nothing_records_nothing() {
    for scale in [1u32, 2, 3] {
        let bg = background(80, 60);
        for (n, (x, y, text, style)) in [
            (2, 3, "Hello", TextStyle::PLAIN),
            (-20, 5, "Hello, World", TextStyle::BOLD),
            (25, 5, "Hello", TextStyle::PLAIN.sized(2)),
            (-3, -4, "Ça\u{7f}", TextStyle::BOLD.sized(2)),
            (0, 0, "WWWW", TextStyle::PLAIN.sized(3)),
        ]
        .into_iter()
        .enumerate()
        {
            let mut pixels = bg.clone();
            let mut damage = Damage::new();
            {
                let mut canvas = Canvas::new(&mut pixels, 80, 60, 80)
                    .unwrap()
                    .with_scale(scale)
                    .with_damage(&mut damage);
                canvas.text(x, y, text, Colour::solid(0xFFFFFF), style);
            }
            let mut changed = 0;
            for (at, (g, b)) in pixels.iter().zip(&bg).enumerate() {
                if g != b {
                    changed += 1;
                    let (px, py) = ((at % 80) as i32, (at / 80) as i32);
                    assert!(
                        damage.rects().iter().any(|r| r.contains(px, py)),
                        "call {n} scale {scale}: {px},{py} changed outside the damage"
                    );
                }
            }
            assert!(changed > 0, "call {n}");
            assert!(Rect::new(0, 0, 80, 60).contains_rect(&damage.bounds()), "cut to the clip");
        }
        // Nothing drawn, nothing recorded.
        let mut pixels = bg.clone();
        let mut damage = Damage::new();
        {
            let mut canvas = Canvas::new(&mut pixels, 80, 60, 80)
                .unwrap()
                .with_scale(scale)
                .with_damage(&mut damage);
            let ink = Colour::solid(0xFFFFFF);
            canvas.text(2, 3, "", ink, TextStyle::PLAIN);
            canvas.text(2, 3, "Hello", Colour::CLEAR, TextStyle::PLAIN);
            canvas.text(2, 3, "Hello", Colour(0x00FF_FFFF), TextStyle::BOLD);
            canvas.text(100, 3, "Hello", ink, TextStyle::PLAIN); // right of the canvas
            canvas.text(2, 100, "Hello", ink, TextStyle::PLAIN); // below
            canvas.text(-100, 3, "Hello", ink, TextStyle::PLAIN); // left
            canvas.text(2, -20, "Hello", ink, TextStyle::PLAIN); // above
            canvas.text(MIN, MIN, "Hello", ink, TextStyle::PLAIN);
            canvas.text(MAX, MAX, "Hello", ink, TextStyle::PLAIN);
            canvas.clipped(Rect::EMPTY).text(2, 3, "Hello", ink, TextStyle::PLAIN);
            canvas.clipped(Rect::new(90, 90, 5, 5)).text(2, 3, "Hello", ink, TextStyle::PLAIN);
        }
        assert!(damage.is_empty(), "{:?}", damage.rects());
        assert_eq!(pixels, bg);
    }
}

#[test]
fn hostile_arguments_never_panic_and_never_leave_the_canvas() {
    const SIDE: usize = 100;
    let long = "Wg é中😀\n".repeat(50_000);
    for scale in [1u32, 2, 8] {
        let mut mem = vec![0xDEAD_BEEFu32; SIDE * SIDE];
        let start = 10 * SIDE + 10;
        {
            let mut canvas =
                Canvas::new(&mut mem[start..], 64, 64, SIDE as u32).unwrap().with_scale(scale);
            let places = [MIN, MIN + 1, -1_000_000, -1, 0, 1, 7, 63, 64, 1_000_000, MAX - 1, MAX];
            let styles = [
                TextStyle { size: 0, bold: false },
                TextStyle::PLAIN,
                TextStyle::BOLD,
                TextStyle { size: 1000, bold: true },
                TextStyle { size: u32::MAX, bold: false },
            ];
            for &x in &places {
                for &y in &places {
                    for style in styles {
                        canvas.text(x, y, "Hello é\u{7f}", Colour(0x80FF_0000), style);
                    }
                }
            }
            for style in styles {
                canvas.text(-30, -3, &long, Colour(0x80FF_0000), style);
                canvas.text(MIN, MIN, &long, Colour(0x80FF_0000), style);
                let mut part = canvas.sub(Rect::new(MAX - 3, 2, MAX, 5));
                part.text(MIN, 0, "Hello", Colour::WHITE, style);
            }
        }
        for (at, &pixel) in mem.iter().enumerate() {
            let (x, y) = (at % SIDE, at / SIDE);
            if (10..74).contains(&x) && (10..74).contains(&y) {
                // Drawn on (so solid) or never reached (so as it was).
                assert!(pixel >> 24 == 0xFF || pixel == 0xDEAD_BEEF, "scale {scale}: {x},{y}");
            } else {
                assert_eq!(pixel, 0xDEAD_BEEF, "scale {scale}: {x},{y} outside the canvas");
            }
        }
    }
}

#[test]
fn a_million_characters_take_milliseconds() {
    let long: String =
        "The quick brown fox é中 ".repeat(1_000_000 / 22 + 1).chars().take(1_000_000).collect();
    assert_eq!(long.chars().count(), 1_000_000);
    let mut pixels = background(64, 64);
    let start = Instant::now();
    let mut calls = 0;
    {
        let mut canvas = Canvas::new(&mut pixels, 64, 64, 64).unwrap();
        let positions = [
            (0, 0),
            (-5, -2),
            (MIN, 0),
            (-1_000_000, 10),
            (-5_999_990, 20), // only the last characters show
            (-6_000_000, 5),
            (-500_000, 30),
            (63, 63),
            (64, 0),
            (1000, 1000),
            (MAX, MAX),
            (0, MIN),
        ];
        for (i, &(x, y)) in positions.iter().enumerate() {
            for style in [TextStyle::PLAIN, TextStyle::BOLD.sized(2)] {
                let end = canvas.text(x, y, &long, Colour(0x80FF_FFFF), style);
                assert_eq!(end, x.saturating_add(style.width(&long)), "{i}");
                calls += 1;
            }
        }
    }
    let took = start.elapsed();
    assert!(calls >= 20);
    println!("{calls} calls on a million characters took {took:?}");
    assert!(took.as_secs_f64() < 5.0, "{took:?}");
}

// ---- Reference pictures ----

const INK: Colour = Colour::solid(0x22252c);

/// Every character of the font, in code point order, 32 to a line, each line labelled with
/// the code point of its first character in hex. Each character has a cell 8 logical pixels
/// wide (a little wider than text is drawn) so that each can be looked at on its own.
fn font_sheet(c: &mut Canvas, lines: usize) {
    let characters: Vec<char> = pbr_font::characters().collect();
    assert_eq!(characters.len().div_ceil(32), lines);
    for (row, line) in characters.chunks(32).enumerate() {
        let y = 6 + row as i32 * 16;
        let label = format!("{:04x}", u32::from(line[0]));
        c.text(6, y, &label, Colour::solid(0x2f6b55), TextStyle::BOLD);
        for (n, &ch) in line.iter().enumerate() {
            let mut buffer = [0u8; 4];
            c.text(44 + n as i32 * 8, y, ch.encode_utf8(&mut buffer), INK, TextStyle::PLAIN);
        }
    }
}

fn font_sheet_size() -> (u32, u32, usize) {
    let lines = pbr_font::characters().count().div_ceil(32);
    (44 + 32 * 8 + 6, 12 + lines as u32 * 16, lines)
}

#[test]
fn the_font_at_scale_1() {
    let (w, h, lines) = font_sheet_size();
    let mut sheet = Sheet::new(w, h, PAPER);
    font_sheet(&mut sheet.canvas(), lines);
    sheet.check("text-font-1x");
}

#[test]
fn the_font_at_scale_2() {
    let (w, h, lines) = font_sheet_size();
    let mut sheet = Sheet::new(w * 2, h * 2, PAPER);
    font_sheet(&mut sheet.canvas().with_scale(2), lines);
    sheet.check("text-font-2x");
}

/// The same calls for every scale: 450 × 232 logical pixels.
fn styles(c: &mut Canvas) {
    let pangram = "Pack my box with five dozen liquor jugs. Sphinx of black quartz!";
    c.text(6, 6, "Plain: The quick brown fox jumps over the lazy dog", INK, TextStyle::PLAIN);
    c.text(6, 20, "Bold: The quick brown fox jumps over the lazy dog", INK, TextStyle::BOLD);
    c.text(6, 36, "Size 2: Hello, World", INK, TextStyle::PLAIN.sized(2));
    c.text(6, 60, "Size 2 bold: Hello", INK, TextStyle::BOLD.sized(2));
    c.text(6, 86, "Size 3 Wg", INK, TextStyle::PLAIN.sized(3));
    // The 1.x title bar, with white bold text.
    c.fill_rect(Rect::new(230, 84, 214, 22), Colour::solid(0x2f6b55));
    c.text(238, 89, "Title bar: Notepad", Colour::WHITE, TextStyle::BOLD);
    // See-through text over a shape: the shape shows through the letters.
    c.fill_rect(Rect::new(6, 122, 90, 44), Colour::solid(0xd2577a));
    c.fill_rect(Rect::new(60, 134, 90, 36), Colour::solid(0x2a2e3d));
    c.text(10, 126, "Half black", Colour(0x8000_0000), TextStyle::BOLD);
    c.text(64, 156, "Half white", Colour(0x80FF_FFFF), TextStyle::BOLD);
    c.text(14, 140, "See", Colour(0x70FF_FFFF), TextStyle::PLAIN.sized(3));
    // Clipped by a part of the canvas, in the middle of a word.
    c.stroke_rect(Rect::new(200, 122, 100, 22), 1, Colour::solid(0x999999));
    c.sub(Rect::new(201, 123, 98, 20)).text(
        -17,
        5,
        "Clipped in the middle of words",
        INK,
        TextStyle::PLAIN,
    );
    c.sub(Rect::new(201, 146, 40, 20)).text(2, -5, "Cut above", INK, TextStyle::PLAIN.sized(2));
    // The box for characters the font lacks.
    c.text(310, 124, "Boxes: \u{7f}\u{4e2d}\u{1F600}\n!", INK, TextStyle::PLAIN);
    c.text(310, 138, "Boxes: \u{7f}\u{4e2d}\u{1F600}\n!", INK, TextStyle::BOLD);
    c.text(310, 152, "\u{7f}\u{4e2d}", INK, TextStyle::PLAIN.sized(2));
    // A pangram, digits and punctuation, and accents.
    c.text(6, 176, pangram, INK, TextStyle::PLAIN);
    c.text(6, 190, "0123456789 !\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~", INK, TextStyle::PLAIN);
    c.text(
        6,
        206,
        "Ça été déjà vu: naïve façade, über Straße, Łódź, Ångström, señor, £5 €10",
        INK,
        TextStyle::PLAIN,
    );
    c.text(6, 220, "ÀÉÎÕÜ ĀĒŌŪ ČŠŽ ŁŃŚ ĄĘ ÇÑ ÅÆØ ß", INK, TextStyle::PLAIN);
}

#[test]
fn styles_at_scale_1() {
    let mut sheet = Sheet::new(450, 236, PAPER);
    styles(&mut sheet.canvas());
    sheet.check("text-styles-1x");
}

#[test]
fn styles_at_scale_2() {
    let mut sheet = Sheet::new(900, 472, PAPER);
    styles(&mut sheet.canvas().with_scale(2));
    sheet.check("text-styles-2x");
}
