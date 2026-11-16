// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Paint on the host: strokes against a model, the toolbar, saving and opening through a pretend
//! disk and file picker (with a picture 1.3's own Paint saved), the dialogs' words, the
//! clipboard, the colour remembered, and the look (`tests/pictures/paint/`).

use super::*;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Key, WindowId};
use pbr_window::Event;
use std::collections::{BTreeMap, VecDeque};
use std::path::PathBuf;

/// A picture 1.3's Paint saved (the same file `lib/bmp`'s tests use).
const ROCK: &[u8] = include_bytes!("../../../tests/data/classic-1.3-rock.bmp");

/// `pbr.input`'s number for the left button.
const LEFT: u16 = 1;
/// The picture's top left in the window: below the toolbar (37) and its line (1).
const TOP: i32 = 38;
const DATA: &str = "/Home/Apps/paint";
const COLOUR_FILE: &str = "/Home/Apps/paint/colour.txt";

/// A pretend disk and file picker: files in a map, the picker's requests written down and its
/// answers given by the test.
#[derive(Default)]
struct Fake {
    files: BTreeMap<String, Vec<u8>>,
    /// What the picker was asked: the window's number, the serial, and for what.
    asked: Vec<(Option<Win>, u32, Pick)>,
    answers: VecDeque<PickAnswer>,
    /// The paths written to, in order.
    writes: Vec<String>,
    /// There is no picker.
    no_picker: bool,
    /// Every write fails.
    full: bool,
}

impl Disk for Fake {
    fn exists(&mut self, path: &str) -> bool {
        self.files.contains_key(path)
    }

    fn read(&mut self, path: &str, max: usize) -> Result<Vec<u8>, DiskError> {
        let data = self.files.get(path).ok_or_else(|| DiskError::Failed("not found".into()))?;
        if data.len() > max { Err(DiskError::TooBig) } else { Ok(data.clone()) }
    }

    fn write(&mut self, path: &str, data: &[u8]) -> Result<(), DiskError> {
        if self.full {
            return Err(DiskError::Failed("the disk is full".into()));
        }
        self.writes.push(path.into());
        self.files.insert(path.into(), data.to_vec());
        Ok(())
    }

    fn pick(&mut self, window: Option<Win>, serial: u32, pick: Pick) -> Result<(), String> {
        if self.no_picker {
            return Err("Paint has no file picker".into());
        }
        self.asked.push((window, serial, pick));
        Ok(())
    }

    fn answer(&mut self) -> Option<PickAnswer> {
        self.answers.pop_front()
    }
}

type Test = Harness<Paint<Fake>>;

fn paint_with(fake: Fake) -> Paint<Fake> {
    Paint::new(fake, DATA)
}

fn harness_scaled(fake: Fake, factor: u32) -> Test {
    Harness::with_scale(paint_with(fake), settings(), factor)
}

fn harness() -> Test {
    harness_scaled(Fake::default(), 1)
}

/// The picker answers: the answer is given to the app, and the notification the program would
/// send for it comes.
fn answer(h: &mut Test, a: PickAnswer) {
    h.app_mut().disk_mut().answers.push_back(a);
    h.send(Event::Notified(1 << PICKED));
}

/// Clicks a tool on the toolbar (the toolbar starts at the window's top left corner).
fn choose(h: &mut Test, tool: Tool) {
    let (x, y) = tool_centre(tool);
    h.click(x, y);
}

/// Draws with the left button: down at the first point, moving through the rest, up at the last.
fn stroke(h: &mut Test, points: &[(i32, i32)]) {
    h.move_to(points[0].0, TOP + points[0].1);
    h.press(LEFT);
    for p in &points[1..] {
        h.move_to(p.0, TOP + p.1);
    }
    h.release(LEFT);
}

/// What the picture should be after a stroke through `points` (in picture pixels) in `colour`
/// with a brush of radius `r`, worked out the plain way: every pixel within `r` of a point on
/// the line between each pair is coloured. A pixel is within a disc of radius `r` when its
/// squared distance is at most `r * r`; the program works along each row with a square root
/// instead, and the two must agree. The lines in these tests are horizontal, vertical or exactly
/// diagonal, where stepping along the longer side is Bresenham's line.
fn model(base: &[u32], points: &[(i32, i32)], colour: u32, r: i32) -> Vec<u32> {
    let mut stamps = vec![points[0]];
    for pair in points.windows(2) {
        let ((x0, y0), (x1, y1)) = (pair[0], pair[1]);
        let n = (x1 - x0).abs().max((y1 - y0).abs());
        for i in 1..=n {
            stamps.push((x0 + (x1 - x0) * i / n, y0 + (y1 - y0) * i / n));
        }
    }
    let mut pixels = base.to_vec();
    for (cx, cy) in stamps {
        for y in (cy - r).max(0)..=(cy + r).min(PICTURE_HEIGHT as i32 - 1) {
            for x in (cx - r).max(0)..=(cx + r).min(PICTURE_WIDTH as i32 - 1) {
                if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                    pixels[y as usize * PICTURE_WIDTH + x as usize] = colour;
                }
            }
        }
    }
    pixels
}

fn white() -> Vec<u32> {
    vec![WHITE; PICTURE_WIDTH * PICTURE_HEIGHT]
}

/// The text of the dialog on show, if there is one.
fn dialog_text(h: &Test) -> Option<String> {
    let id = h.dialog()?;
    // Labels that wrap give a line each; join them back into words.
    let words: Vec<String> = h
        .describe(id)
        .iter()
        .filter(|d| d.what.role == "label")
        .map(|d| d.what.label.clone())
        .collect();
    Some(words.join(" "))
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/paint")
}

// ---- The toolbar. ----

#[test]
fn every_tool_is_where_1_3_had_it() {
    // 1.3: swatch i at 8 + 23 i, 20 wide; sizes from 8 + 13 * 23 + 8 = 315, 27 apart, 26 wide.
    assert_eq!(tool_at(8, 10), Some(Tool::Colour(0)));
    assert_eq!(tool_at(27, 10), Some(Tool::Colour(0)));
    assert_eq!(tool_at(28, 10), None);
    assert_eq!(tool_at(31, 36), Some(Tool::Colour(1)));
    assert_eq!(tool_at(8 + 12 * 23 + 19, 0), Some(Tool::Colour(12)));
    assert_eq!(tool_at(314, 10), None);
    assert_eq!(tool_at(315, 10), Some(Tool::Size(0)));
    assert_eq!(tool_at(315 + 27 * 3 + 25, 10), Some(Tool::Size(3)));
    assert_eq!(tool_at(315 + 27 * 3 + 26, 10), None);
    // Below the toolbar isn't a tool, nor is above it.
    assert_eq!(tool_at(10, 37), None);
    assert_eq!(tool_at(10, -1), None);
    for i in 0..13 {
        let (x, y) = tool_centre(Tool::Colour(i));
        assert_eq!(tool_at(x, y), Some(Tool::Colour(i)));
    }
    for i in 0..4 {
        let (x, y) = tool_centre(Tool::Size(i));
        assert_eq!(tool_at(x, y), Some(Tool::Size(i)));
    }
}

#[test]
fn choosing_a_colour_and_a_size() {
    let mut h = harness();
    assert_eq!((h.app().colour(), h.app().size()), (0, 1));
    assert_eq!(h.app_mut().take_log(), ["colour 0 size 1"]);
    choose(&mut h, Tool::Colour(3));
    choose(&mut h, Tool::Size(3));
    assert_eq!((h.app().colour(), h.app().size()), (3, 3));
    assert_eq!(h.app_mut().take_log(), ["colour 3 size 1", "colour 3 size 3"]);
    // The same again changes nothing, and says nothing.
    choose(&mut h, Tool::Colour(3));
    assert!(h.app_mut().take_log().is_empty());
    // Clicking between the swatches does nothing.
    h.click(29, 20);
    assert_eq!(h.app().colour(), 3);
}

#[test]
fn the_last_colour_is_kept_and_comes_back() {
    let mut h = harness();
    // A new colour is written in the app's own folder, as its number.
    choose(&mut h, Tool::Colour(9));
    assert_eq!(h.app().disk().files[COLOUR_FILE], b"9\n");
    choose(&mut h, Tool::Size(0));
    assert_eq!(h.app().disk().writes, [COLOUR_FILE]);
    // The next time Paint starts, it is chosen.
    let fake = Fake { files: h.app().disk().files.clone(), ..Fake::default() };
    let again = harness_scaled(fake, 1);
    assert_eq!((again.app().colour(), again.app().size()), (9, 1));
    // Nonsense in the file, or a number that isn't a colour, means the first.
    for text in ["", "red", "13", "-1", "99999999999999999999", "\u{0}\u{1}"] {
        let mut fake = Fake::default();
        fake.files.insert(COLOUR_FILE.into(), text.as_bytes().to_vec());
        assert_eq!(paint_with(fake).colour(), 0, "{text:?}");
    }
    let mut fake = Fake::default();
    fake.files.insert(COLOUR_FILE.into(), b" 12 \n".to_vec());
    assert_eq!(paint_with(fake).colour(), 12);
    // A disk that can't be written to doesn't stop the colour being chosen.
    let mut h = harness_scaled(Fake { full: true, ..Fake::default() }, 1);
    choose(&mut h, Tool::Colour(4));
    assert_eq!(h.app().colour(), 4);
    assert_eq!(
        h.app_mut().take_log().last().unwrap(),
        "couldn't keep the colour: the disk is full"
    );
}

// ---- Drawing. ----

#[test]
fn a_stroke_colours_the_same_picture_pixels_at_1x_and_2x() {
    // Each of the brush sizes, after choosing a colour.
    let points = [(100, 80), (140, 80), (140, 120), (100, 160)];
    for factor in [1, 2] {
        for (size, radius) in BRUSHES.into_iter().enumerate() {
            let mut h = harness_scaled(Fake::default(), factor);
            choose(&mut h, Tool::Colour(3));
            choose(&mut h, Tool::Size(size));
            stroke(&mut h, &points);
            let want = model(&white(), &points, PALETTE[3], radius);
            assert!(h.app().pixels() == want, "scale {factor}, size {size}");
            assert!(h.app().is_dirty());
        }
    }
}

#[test]
fn the_picture_is_600_by_362_pixels_at_any_scale() {
    for factor in [1, 2] {
        let h = harness_scaled(Fake::default(), factor);
        assert_eq!(h.app().pixels().len(), 600 * 362);
        let picture = h.find("picture").unwrap().1.rect;
        assert_eq!((picture.x, picture.y, picture.width, picture.height), (0, 38, 600, 362));
        // The window's own size is 600 x 400, whatever the scale.
        assert_eq!(h.picture(WindowId::MAIN).width, 600 * factor);
        assert_eq!(h.picture(WindowId::MAIN).height, 400 * factor);
    }
}

#[test]
fn a_dab_without_moving_and_strokes_at_the_edges() {
    let mut h = harness();
    choose(&mut h, Tool::Size(3));
    // A press and release in the same place is a dab, cut at the corner.
    stroke(&mut h, &[(0, 0)]);
    let want = model(&white(), &[(0, 0)], PALETTE[0], 18);
    assert!(h.app().pixels() == want);
    // Down the right edge and off the bottom: clipped, nothing wraps round to the other side.
    stroke(&mut h, &[(599, 300), (599, 361)]);
    let want = model(&want, &[(599, 300), (599, 361)], PALETTE[0], 18);
    assert!(h.app().pixels() == want);
    assert_eq!(h.app().pixels()[20 * PICTURE_WIDTH], WHITE, "the left edge was not touched");
}

#[test]
fn moving_without_the_button_draws_nothing_and_a_drag_may_leave_the_window() {
    let mut h = harness();
    h.move_to(100, 100);
    h.move_to(200, 200);
    assert!(h.app().pixels() == white());
    assert!(!h.app().is_dirty());
    // Press in the picture and drag far out of the window, then back.
    h.move_to(300, TOP + 100);
    h.press(LEFT);
    h.move_to(900, TOP + 700);
    h.move_to(300, TOP + 100);
    h.release(LEFT);
    assert!(h.app().pixels() != white());
    // And moving after the release draws nothing more.
    let drawn = h.app().pixels().to_vec();
    h.move_to(50, TOP + 50);
    assert!(h.app().pixels() == drawn);
    // Only the left button draws.
    h.move_to(10, TOP + 10);
    h.press(2);
    h.move_to(20, TOP + 20);
    h.release(2);
    assert!(h.app().pixels() == drawn);
}

#[test]
fn strokes_far_outside_are_safe() {
    let mut paint = paint_with(Fake::default());
    paint.stroke(Stroke::Down(i32::MIN, i32::MAX));
    paint.stroke(Stroke::To(i32::MAX, i32::MIN));
    paint.stroke(Stroke::To(0, 0));
    paint.stroke(Stroke::Up);
    // No stroke is under way: a move does nothing.
    let before = paint.pixels().to_vec();
    paint.stroke(Stroke::To(5, 5));
    assert!(paint.pixels() == before);
    assert_eq!(paint.pixels().len(), PICTURE_WIDTH * PICTURE_HEIGHT);
}

#[test]
fn a_stroke_logs_its_picture_when_it_ends() {
    let mut h = harness();
    assert!(!h.app_mut().take_picture_changed());
    let before = h.app().picture_line();
    // A white picture's checksum: the CRC-32 of 600 * 362 white pixels, 4 bytes each.
    let bytes: Vec<u8> =
        std::iter::repeat_n([0xFF, 0xFF, 0xFF, 0x00], 600 * 362).flatten().collect();
    assert_eq!(before, format!("picture 600x362 crc {:08x}", pbr_crc::crc32(&bytes)));
    h.move_to(100, TOP + 100);
    h.press(LEFT);
    h.move_to(150, TOP + 100);
    assert!(!h.app_mut().take_picture_changed(), "only when the stroke ends");
    h.release(LEFT);
    assert!(h.app_mut().take_picture_changed());
    assert!(!h.app_mut().take_picture_changed());
    assert_ne!(h.app().picture_line(), before);
}

#[test]
fn clear_makes_a_white_picture_from_no_file() {
    let mut fake = Fake::default();
    fake.files.insert("/Home/Pictures/rock.bmp".into(), ROCK.to_vec());
    let mut h = harness_scaled(fake, 1);
    h.click_on("Open");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/rock.bmp".into()));
    assert_eq!(h.app().path(), Some("/Home/Pictures/rock.bmp"));
    stroke(&mut h, &[(10, 10), (20, 20)]);
    assert!(h.app().is_dirty());
    h.app_mut().take_picture_changed();
    h.click_on("Clear");
    assert!(h.app().pixels() == white());
    assert_eq!(h.app().path(), None);
    assert!(!h.app().is_dirty());
    assert!(h.app_mut().take_picture_changed());
}

// ---- The title. ----

#[test]
fn the_title_says_untitled_or_the_file_and_the_bullet_for_unsaved_changes() {
    let mut h = harness();
    assert_eq!(h.app().title(), "Untitled");
    stroke(&mut h, &[(10, 10), (30, 30)]);
    assert_eq!(h.app().title(), "• Untitled");
    h.click_on("Save");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/Drawing 1.bmp".into()));
    assert_eq!(h.app().title(), "Drawing 1.bmp");
    stroke(&mut h, &[(10, 10), (30, 30)]);
    assert_eq!(h.app().title(), "• Drawing 1.bmp");
}

// ---- Saving. ----

/// What saving the picture as it is should write.
fn bytes_of(h: &Test) -> Vec<u8> {
    pbr_bmp::encode(&pbr_bmp::Picture::new(600, 362, h.app().pixels().to_vec()).unwrap())
}

#[test]
fn save_asks_for_drawing_n_and_writes_lib_bmps_bytes() {
    let mut h = harness();
    stroke(&mut h, &[(10, 10), (60, 40)]);
    h.click_on("Save");
    // The picker is asked with the first free name, from the window and the press that asked.
    let asked = &h.app().disk().asked;
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0].2, Pick::Save { name: "Drawing 1.bmp".into() });
    assert!(asked[0].0.is_some() && asked[0].1 != 0);
    assert!(h.app().disk().writes.is_empty(), "nothing is written before the user chooses");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/Drawing 1.bmp".into()));
    let want = bytes_of(&h);
    assert_eq!(h.app().disk().files["/Home/Pictures/Drawing 1.bmp"], want);
    assert_eq!(h.app().path(), Some("/Home/Pictures/Drawing 1.bmp"));
    assert!(!h.app().is_dirty());
    let log = h.app_mut().take_log();
    assert_eq!(
        log.last().unwrap(),
        &format!("saved /Home/Pictures/Drawing 1.bmp ({} bytes)", want.len())
    );
    // 24-bit, 600 x 362: 54 bytes of header and 1800 a row.
    assert_eq!(want.len(), 54 + 1800 * 362);
}

#[test]
fn save_again_writes_without_asking_and_save_as_always_asks() {
    let path = "/Home/Pictures/Drawing 1.bmp";
    let mut h = harness();
    h.click_on("Save");
    answer(&mut h, PickAnswer::Chosen(path.into()));
    assert_eq!(h.app().disk().asked.len(), 1);
    stroke(&mut h, &[(10, 10), (60, 40)]);
    // The button and Ctrl+S both write again.
    h.click_on("Save");
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.app().disk().asked.len(), 1, "the picker wasn't asked again");
    assert_eq!(h.app().disk().writes.iter().filter(|p| *p == path).count(), 3);
    let want = bytes_of(&h);
    assert_eq!(h.app().disk().files[path], want);
    assert!(!h.app().is_dirty());
    // Save as asks, offering the file's own name, and the answer is where it goes.
    h.shortcut(Shortcut::ctrl_shift('s'));
    let asked = &h.app().disk().asked;
    assert_eq!(asked.len(), 2);
    assert_eq!(asked[1].2, Pick::Save { name: "Drawing 1.bmp".into() });
    answer(&mut h, PickAnswer::Chosen("/Volumes/USB/copy.bmp".into()));
    assert_eq!(h.app().path(), Some("/Volumes/USB/copy.bmp"));
    assert_eq!(h.app().disk().files["/Volumes/USB/copy.bmp"], want);
    assert_eq!(h.app().title(), "copy.bmp");
}

#[test]
fn the_suggested_name_skips_the_drawings_that_exist() {
    let mut fake = Fake::default();
    for n in [1, 2, 4] {
        fake.files.insert(format!("/Home/Pictures/Drawing {n}.bmp"), vec![0]);
    }
    let mut h = harness_scaled(fake, 1);
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.app().disk().asked[0].2, Pick::Save { name: "Drawing 3.bmp".into() });
}

#[test]
fn cancelling_the_picker_does_nothing_and_it_can_be_asked_again() {
    let mut h = harness();
    stroke(&mut h, &[(10, 10), (60, 40)]);
    h.click_on("Save");
    // One picker at a time: pressing again while it is open doesn't ask a second.
    h.click_on("Save");
    h.click_on("Open");
    assert_eq!(h.app().disk().asked.len(), 1);
    answer(&mut h, PickAnswer::Cancelled);
    assert!(h.app().disk().writes.is_empty());
    assert!(h.dialog().is_none());
    assert!(h.app().is_dirty());
    h.click_on("Open");
    assert_eq!(h.app().disk().asked.len(), 2);
    assert_eq!(h.app().disk().asked[1].2, Pick::Open);
}

#[test]
fn a_disk_that_cant_be_written_says_so() {
    let mut h = harness();
    h.click_on("Save");
    h.app_mut().disk_mut().full = true;
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/Drawing 1.bmp".into()));
    assert_eq!(dialog_text(&h).as_deref(), Some("Couldn't save the picture. the disk is full."));
    assert_eq!(h.app().path(), None);
    assert_eq!(
        h.app_mut().take_log().last().unwrap(),
        "couldn't save /Home/Pictures/Drawing 1.bmp: the disk is full"
    );
}

#[test]
fn no_picker_or_one_that_fails_says_so() {
    let mut h = harness_scaled(Fake { no_picker: true, ..Fake::default() }, 1);
    h.click_on("Open");
    assert_eq!(
        dialog_text(&h).as_deref(),
        Some("Couldn't open the file picker. Paint has no file picker.")
    );
    let mut h = harness_scaled(Fake { no_picker: true, ..Fake::default() }, 1);
    h.click_on("Save");
    assert_eq!(
        dialog_text(&h).as_deref(),
        Some("Couldn't save the picture. Paint has no file picker.")
    );
    // A picker that was asked and then failed.
    let mut h = harness();
    h.click_on("Save");
    answer(&mut h, PickAnswer::Failed("BadPath".into()));
    assert_eq!(dialog_text(&h).as_deref(), Some("Couldn't save the picture. BadPath."));
    // An answer nobody asked for is ignored.
    let mut h = harness();
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/x.bmp".into()));
    assert!(h.dialog().is_none());
    assert!(h.app().disk().writes.is_empty());
}

// ---- Opening. ----

#[test]
fn opening_a_picture_1_3_saved_shows_it() {
    let mut fake = Fake::default();
    fake.files.insert("/Home/Pictures/rock.bmp".into(), ROCK.to_vec());
    let mut h = harness_scaled(fake, 1);
    h.shortcut(Shortcut::ctrl('o'));
    assert_eq!(h.app().disk().asked[0].2, Pick::Open);
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/rock.bmp".into()));
    let rock = pbr_bmp::decode(ROCK).unwrap();
    assert_eq!((rock.width(), rock.height()), (600, 362));
    assert!(h.app().pixels() == rock.pixels());
    assert_eq!(h.app().path(), Some("/Home/Pictures/rock.bmp"));
    assert_eq!(h.app().title(), "rock.bmp");
    assert!(!h.app().is_dirty());
    assert!(h.app_mut().take_picture_changed());
    assert_eq!(h.app_mut().take_log().last().unwrap(), "opened /Home/Pictures/rock.bmp");
    // Saving it again gives the bytes 1.3 wrote.
    h.click_on("Save");
    assert_eq!(h.app().disk().files["/Home/Pictures/rock.bmp"], ROCK);
}

#[test]
fn a_smaller_picture_sits_at_the_top_left_on_white_and_a_bigger_one_is_cut() {
    let mut fake = Fake::default();
    let small = pbr_bmp::Picture::new(3, 2, vec![0xff0000, 0x00ff00, 0x0000ff, 1, 2, 3]).unwrap();
    fake.files.insert("/Home/Pictures/small.bmp".into(), pbr_bmp::encode(&small));
    let (w, h_) = (700u32, 400u32);
    let big: Vec<u32> = (0..w * h_).map(|i| (i % w) | ((i / w) << 12)).collect();
    let big = pbr_bmp::Picture::new(w, h_, big).unwrap();
    fake.files.insert("/Home/Pictures/big.bmp".into(), pbr_bmp::encode(&big));
    let mut h = harness_scaled(fake, 1);
    h.click_on("Open");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/small.bmp".into()));
    let px = h.app().pixels();
    assert_eq!(&px[..3], [0xff0000, 0x00ff00, 0x0000ff]);
    assert_eq!(&px[3..5], [WHITE, WHITE]);
    assert_eq!(&px[PICTURE_WIDTH..PICTURE_WIDTH + 4], [1, 2, 3, WHITE]);
    assert!(px[2 * PICTURE_WIDTH..].iter().all(|&p| p == WHITE));
    assert!(px[5..PICTURE_WIDTH].iter().all(|&p| p == WHITE));
    // A bigger one is cut to 600 x 362, from the top left.
    h.click_on("Open");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/big.bmp".into()));
    for (x, y) in [(0usize, 0usize), (599, 0), (0, 361), (599, 361), (300, 200)] {
        assert_eq!(h.app().pixels()[y * 600 + x], big.pixels()[y * 700 + x], "({x}, {y})");
    }
}

#[test]
fn a_file_paint_cant_open_gets_1_3s_words() {
    let sentence =
        "bad.bmp isn't a picture Paint can open. It opens uncompressed 24- or 32-bit .bmp files.";
    let mut fake = Fake::default();
    fake.files.insert("/Home/Pictures/bad.bmp".into(), b"This is not a bitmap.".to_vec());
    let mut compressed = pbr_bmp::encode(&pbr_bmp::Picture::new(2, 2, vec![0; 4]).unwrap());
    compressed[30] = 1;
    fake.files.insert("/Volumes/USB/bad.bmp".into(), compressed);
    fake.files.insert("/Home/Pictures/huge.bmp".into(), vec![b'B'; MAX_FILE + 1]);
    let mut h = harness_scaled(fake, 1);
    stroke(&mut h, &[(10, 10), (30, 30)]);
    let drawn = h.app().pixels().to_vec();
    for path in ["/Home/Pictures/bad.bmp", "/Volumes/USB/bad.bmp"] {
        h.click_on("Open");
        answer(&mut h, PickAnswer::Chosen(path.into()));
        assert_eq!(dialog_text(&h).as_deref(), Some(sentence), "{path}");
        // OK closes it; the picture and what it came from are as they were.
        h.click_on("OK");
        assert!(h.dialog().is_none());
        assert!(h.app().pixels() == drawn);
        assert_eq!(h.app().path(), None);
    }
    // Bigger than 8 MiB is refused without reading, in the same words, naming that file.
    h.click_on("Open");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/huge.bmp".into()));
    assert_eq!(dialog_text(&h).as_deref(), Some(&*sentence.replace("bad.bmp", "huge.bmp")));
    h.click_on("OK");
    // A file that can't be read at all.
    h.click_on("Open");
    answer(&mut h, PickAnswer::Chosen("/Home/Pictures/gone.bmp".into()));
    assert_eq!(dialog_text(&h).as_deref(), Some("Couldn't open gone.bmp. not found."));
    let log = h.app_mut().take_log();
    assert!(log.iter().any(|l| l.starts_with("couldn't open /Home/Pictures/gone.bmp")));
}

#[test]
fn a_file_the_program_was_started_with_opens_at_the_start() {
    let mut fake = Fake::default();
    fake.files.insert("/Home/Pictures/rock.bmp".into(), ROCK.to_vec());
    let paint = paint_with(fake).with_file("/Home/Pictures/rock.bmp");
    let h = Harness::new(paint, settings());
    assert!(h.app().pixels() == pbr_bmp::decode(ROCK).unwrap().pixels());
    assert_eq!(h.app().title(), "rock.bmp");
    // One that can't be opened says so, and Paint starts with a white picture.
    let paint = paint_with(Fake::default()).with_file("/Home/Pictures/nothing.bmp");
    let h = Harness::new(paint, settings());
    assert!(h.app().pixels() == white());
    assert_eq!(dialog_text(&h).as_deref(), Some("Couldn't open nothing.bmp. not found."));
}

// ---- The clipboard. ----

#[test]
fn copy_and_paste() {
    let mut h = harness();
    // Nothing on the clipboard: Ctrl+V does nothing.
    h.shortcut(Shortcut::ctrl('v'));
    assert!(h.app().pixels() == white());
    assert!(!h.app().is_dirty());
    choose(&mut h, Tool::Colour(6));
    stroke(&mut h, &[(100, 100), (200, 100)]);
    let drawn = h.app().pixels().to_vec();
    h.shortcut(Shortcut::ctrl('c'));
    let (w, hh, pixels) = h.clipboard_picture().expect("the picture was copied");
    assert_eq!((w, hh), (600, 362));
    assert_eq!(pixels, drawn);
    // Clear, then paste: the picture is back, and it needs saving.
    h.click_on("Clear");
    assert!(!h.app().is_dirty());
    h.app_mut().take_picture_changed();
    h.shortcut(Shortcut::ctrl('v'));
    assert!(h.app().pixels() == drawn);
    assert!(h.app().is_dirty());
    assert!(h.app_mut().take_picture_changed());
}

#[test]
fn a_pasted_picture_goes_on_the_top_left_cut_to_fit() {
    let mut paint = paint_with(Fake::default());
    // 2 x 2 on a white picture: only that corner changes.
    paint.paste_pixels(2, 2, &[1, 2, 3, 4]);
    assert_eq!(&paint.pixels()[..3], [1, 2, WHITE]);
    assert_eq!(&paint.pixels()[PICTURE_WIDTH..PICTURE_WIDTH + 3], [3, 4, WHITE]);
    assert!(paint.pixels()[2 * PICTURE_WIDTH..].iter().all(|&p| p == WHITE));
    // Wider and taller than the picture: cut, and nothing runs into the next row.
    let (w, h) = (PICTURE_WIDTH as u32 + 5, PICTURE_HEIGHT as u32 + 5);
    let big: Vec<u32> = (0..w * h).map(|i| 0xFF00_0000 | i).collect();
    paint.paste_pixels(w, h, &big);
    assert_eq!(paint.pixels()[0], 0);
    assert_eq!(paint.pixels()[PICTURE_WIDTH - 1], PICTURE_WIDTH as u32 - 1, "alpha is dropped");
    assert_eq!(paint.pixels()[PICTURE_WIDTH], w);
    assert_eq!(paint.pixels().len(), PICTURE_WIDTH * PICTURE_HEIGHT);
    let last = (PICTURE_HEIGHT as u32 - 1) * w + PICTURE_WIDTH as u32 - 1;
    assert_eq!(*paint.pixels().last().unwrap(), last);
}

// ---- The look. ----

#[test]
fn the_buttons_are_where_1_3_had_them() {
    let h = harness();
    for (label, x, w) in [("Open", 436, 46), ("Save", 486, 46), ("Clear", 536, 56)] {
        let r = h.find(label).unwrap().1.rect;
        assert_eq!((r.x, r.y, r.width, r.height), (x, 8, w, 22), "{label}");
    }
}

#[test]
fn plain_keys_do_nothing_and_shift_makes_save_as() {
    let mut h = harness();
    h.key(Key::Enter);
    h.type_text("s");
    assert!(h.app().disk().asked.is_empty());
    h.shortcut(Shortcut::ctrl_shift('s'));
    assert_eq!(h.app().disk().asked.len(), 1);
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "paint-1x"), (2, "paint-2x")] {
        let h = harness_scaled(Fake::default(), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    // After strokes in chosen colours and sizes, with the colour's ring and the size's
    // highlight moved to match.
    let mut h = harness();
    choose(&mut h, Tool::Colour(9));
    choose(&mut h, Tool::Size(3));
    stroke(&mut h, &[(120, 100), (220, 140), (330, 90), (450, 200)]);
    choose(&mut h, Tool::Colour(3));
    choose(&mut h, Tool::Size(2));
    stroke(&mut h, &[(80, 250), (300, 300), (520, 250)]);
    h.check_window_picture_in(WindowId::MAIN, &dir(), "paint-drawn-1x");
}
