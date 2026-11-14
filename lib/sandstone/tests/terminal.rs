// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `TerminalView` and `Canvas` through the harness: keys reaching the app (and the view's own
//! ones not), the clipboard both ways, selecting by dragging and the text it gives, drawing one
//! row at a time, the columns and rows that fit, the wheel; the canvas's drawing at its place,
//! pointer positions relative to it, drags that leave it, keys and releases while focused, the
//! version making it draw again and nothing else; and the look of both at 1x and 2x (note 0023,
//! section 5; note 0026, section 1).

use pbr_draw::Colour;
use pbr_interfaces::pbr_input as input;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;

// ---------------------------------------------------------------------------------------------
// TerminalView
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Key(KeyPress),
    Paste(String),
    Scroll(i32),
    Size(usize, usize),
    Ok,
    Ptr(Pointer),
    CanvasKey(KeyPress, bool),
    Clear,
}

/// A grid of the test's own: lines of characters, a colour for each row, a cursor.
struct Grid {
    lines: Vec<Vec<char>>,
    colours: Vec<Colour>,
    size: (usize, usize),
    cursor: Option<(usize, usize)>,
}

impl Grid {
    fn new(columns: usize, rows: usize) -> Grid {
        let palette = [0xd8dce6, 0x8fd18a, 0xf08a8a, 0xf2a0b8, 0xd9a066, 0x6fa8e8, 0xffffff];
        Grid {
            lines: vec![vec![' '; columns]; rows],
            colours: palette.iter().map(|c| Colour::solid(*c)).collect(),
            size: (columns, rows),
            cursor: None,
        }
    }

    fn set(&mut self, row: usize, text: &str) {
        for (i, c) in text.chars().enumerate() {
            self.lines[row][i] = c;
        }
    }
}

impl Cells for Grid {
    fn size(&self) -> (usize, usize) {
        self.size
    }

    fn cell(&self, column: usize, row: usize) -> Cell {
        let ch = self.lines.get(row).and_then(|l| l.get(column)).copied().unwrap_or(' ');
        Cell { ch, colour: self.colours[row % self.colours.len()], bold: row == 4 }
    }

    fn cursor(&self) -> Option<(usize, usize)> {
        self.cursor
    }
}

struct Term {
    grid: Grid,
    /// Whether `on_key` takes keys (otherwise it says `None` to all).
    keys: bool,
    button: bool,
    ok: u32,
    /// Keys are echoed into the grid, as a shell would: typing a character puts it at the
    /// cursor and moves on, Enter goes to the next line.
    echo: bool,
}

impl Term {
    fn new(columns: usize, rows: usize) -> Term {
        Term { grid: Grid::new(columns, rows), keys: true, button: false, ok: 0, echo: false }
    }
}

impl App for Term {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let mut t = TerminalView::new(&self.grid)
            .on_paste(Msg::Paste)
            .on_scroll(Msg::Scroll)
            .on_resize(Msg::Size);
        if self.keys {
            t = t.on_key(|k| Some(Msg::Key(k)));
        } else {
            t = t.on_key(|_| None);
        }
        if !self.button {
            return t.into();
        }
        Column::new().push(t).push(Button::new("OK").on_press(Msg::Ok).default()).into()
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Size(c, r) => {
                self.grid.size = (c, r);
                self.grid.lines.resize(r, vec![' '; c]);
                for l in &mut self.grid.lines {
                    l.resize(c, ' ');
                }
            }
            Msg::Ok => self.ok += 1,
            Msg::Key(k) if self.echo => {
                if let Some((c, r)) = self.grid.cursor {
                    match k.ch {
                        Some(ch) if k.typed => {
                            self.grid.lines[r][c] = ch;
                            self.grid.cursor = Some((c + 1, r));
                        }
                        _ if k.key == Key::Enter => self.grid.cursor = Some((0, r + 1)),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
}

fn settings() -> Settings {
    Settings { size: (200, 100), resizable: true, min_size: (0, 0), over: None }
}

/// A terminal window with a few lines in it, focused (a click on empty paper), messages
/// cleared.
fn harness() -> Harness<Term> {
    let mut t = Term::new(30, 7);
    t.grid.set(0, "hello world");
    t.grid.set(1, "second line   ");
    t.grid.set(2, "third");
    t.grid.set(3, "$ ls -l");
    t.grid.cursor = Some((7, 3));
    let mut h = Harness::new(t, settings());
    // The first event tells the app the size; it is already that.
    h.move_to(1, 1);
    h.take_messages();
    h
}

/// The terminal's description.
fn term<A: App>(h: &Harness<A>) -> Description
where
    A::Message: std::fmt::Debug,
{
    h.describe(WindowId::MAIN).into_iter().find(|d| d.what.role == "terminal").unwrap()
}

fn focused(mut h: Harness<Term>) -> Harness<Term> {
    h.key(Key::Tab);
    h.take_messages();
    h
}

/// The middle of cell (`c`, `r`) of the view at the window's corner.
fn at(c: i32, r: i32) -> (i32, i32) {
    (8 + 6 * c + 3, 6 + 12 * r + 6)
}

fn row_rect(r: i32) -> pbr_sandstone::Rect {
    Rect::new(0, 6 + 12 * r, 200, 12)
}

const CTRL: Modifiers = Modifiers(Modifiers::CTRL);

fn keys(msgs: &[Msg]) -> Vec<(Option<char>, Key, u16)> {
    msgs.iter()
        .filter_map(|m| match m {
            Msg::Key(k) => Some((k.ch, k.key, k.modifiers.0)),
            _ => None,
        })
        .collect()
}

#[test]
fn it_describes_itself_and_takes_the_focus() {
    let mut h = harness();
    let d = term(&h);
    assert_eq!(d.what.role, "terminal");
    assert_eq!(d.what.value, "$ ls -l");
    assert_eq!((d.rect.x, d.rect.y, d.rect.width, d.rect.height), (0, 0, 200, 100));
    assert!(h.focused().is_none());
    h.click(100, 90);
    assert_eq!(h.focused().unwrap().what.role, "terminal");
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Text);
    h.app_mut().grid.cursor = None;
    h.refresh();
    assert_eq!(term(&h).what.value, "");
}

#[test]
fn keys_reach_on_key_including_tab_enter_and_ctrl_c() {
    let mut h = focused(harness());
    h.type_text("a");
    h.key(Key::Enter);
    h.key(Key::Tab);
    h.key(Key::Up);
    h.key(Key::PageUp);
    h.key_with(Key::Character, CTRL);
    h.shortcut(Shortcut::ctrl('c'));
    h.key_with(Key::Character, Modifiers(Modifiers::CTRL | Modifiers::SHIFT));
    let got = keys(&h.take_messages());
    let c = Modifiers::CTRL;
    assert_eq!(got[0], (Some('a'), Key::Character, 0));
    assert_eq!(got[1].1, Key::Enter);
    assert_eq!(got[2].1, Key::Tab);
    assert_eq!(got[3].1, Key::Up);
    assert_eq!(got[4].1, Key::PageUp);
    assert_eq!(got[6], (Some('c'), Key::Character, c));
    assert_eq!(got.len(), 8);
    // Tab was the view's, so the focus stayed.
    assert_eq!(h.focused().unwrap().what.role, "terminal");
}

#[test]
fn keys_it_does_not_use_are_left_to_the_window() {
    let mut t = Term::new(30, 7);
    t.keys = false;
    t.button = true;
    let mut h = Harness::new(t, settings());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "terminal");
    h.key(Key::Enter);
    assert!(h.take_messages().contains(&Msg::Ok), "Enter reaches the default button");
    // With a taker, Enter is the shell's.
    h.app_mut().keys = true;
    h.refresh();
    h.take_messages();
    h.key(Key::Enter);
    assert_eq!(h.app().ok, 1);
    assert_eq!(keys(&h.take_messages()).len(), 1);
}

#[test]
fn alt_and_meta_and_a_modifier_alone_are_not_sent() {
    let mut h = focused(harness());
    h.key_with(Key::Character, Modifiers(Modifiers::ALT));
    h.key_with(Key::Character, Modifiers(Modifiers::META));
    h.key_with(Key::Modifier, Modifiers::NONE);
    assert_eq!(keys(&h.take_messages()).len(), 0);
}

#[test]
fn nothing_is_sent_without_the_focus() {
    let mut h = harness();
    h.type_text("abc");
    h.shortcut(Shortcut::ctrl_shift('v'));
    h.scroll(20, 20, 0);
    assert!(keys(&h.take_messages()).is_empty());
}

#[test]
fn the_views_own_keys_are_not_sent_on() {
    let mut h = focused(harness());
    h.set_clipboard_text("from outside");
    h.shortcut(Shortcut::ctrl_shift('v'));
    h.shortcut(Shortcut::ctrl_shift('c'));
    assert_eq!(h.take_messages(), vec![Msg::Paste("from outside".to_string())]);
}

#[test]
fn a_middle_click_pastes() {
    let mut h = harness();
    h.set_clipboard_text("middle");
    h.move_to(50, 50);
    h.press(input::BUTTON_MIDDLE as u16);
    h.release(input::BUTTON_MIDDLE as u16);
    assert_eq!(h.take_messages(), vec![Msg::Paste("middle".to_string())]);
    // Outside the view nothing happens.
    h.set_clipboard_text("x");
    h.release(input::BUTTON_MIDDLE as u16);
}

fn copied(h: &mut Harness<Term>) -> Option<String> {
    h.shortcut(Shortcut::ctrl_shift('c'));
    h.clipboard_text()
}

#[test]
fn dragging_selects_a_stream_of_cells_and_ctrl_shift_c_copies_it() {
    // One row.
    let mut h = harness();
    h.set_clipboard_text("old");
    h.drag(at(0, 0), at(4, 0));
    assert_eq!(copied(&mut h).as_deref(), Some("hello"));
    // Several rows: the rest of the first, the start of the last.
    h.drag(at(6, 0), at(4, 1));
    assert_eq!(copied(&mut h).as_deref(), Some("world\nsecon"));
    // Backwards.
    h.drag(at(4, 1), at(6, 0));
    assert_eq!(copied(&mut h).as_deref(), Some("world\nsecon"));
    // Trailing spaces go; a whole row in the middle comes whole.
    h.drag(at(0, 1), at(29, 2));
    assert_eq!(copied(&mut h).as_deref(), Some("second line\nthird"));
    // Further than the view: held within the grid.
    h.drag(at(0, 2), (400, 400));
    let text = copied(&mut h).unwrap();
    assert!(text.starts_with("third\n$ ls -l\n"), "{text:?}");
    // Nothing was sent to the app by any of it.
    assert!(h.take_messages().is_empty());
}

#[test]
fn a_click_clears_the_selection_and_nothing_is_copied() {
    let mut h = harness();
    h.drag(at(0, 0), at(4, 0));
    h.click(at(2, 2).0, at(2, 2).1);
    h.set_clipboard_text("old");
    assert_eq!(copied(&mut h).as_deref(), Some("old"));
    // Dragging inside one cell selects nothing either.
    h.drag((at(3, 0).0 - 1, at(3, 0).1), (at(3, 0).0 + 1, at(3, 0).1));
    assert_eq!(copied(&mut h).as_deref(), Some("old"));
}

#[test]
fn typing_redraws_only_its_row() {
    let mut t = Term::new(30, 7);
    t.echo = true;
    t.grid.set(3, "$ ls -l");
    t.grid.cursor = Some((7, 3));
    let mut h = Harness::new(t, settings());
    h.key(Key::Tab);
    let before = h.window(WindowId::MAIN).unwrap().draws;
    h.type_text("x");
    assert_eq!(h.app().grid.lines[3][7], 'x');
    let w = h.window(WindowId::MAIN).unwrap();
    assert_eq!(w.draws, before + 1);
    assert_eq!(w.damage, vec![row_rect(3)]);
    // Enter moves the cursor to the next row: those two rows and nothing else.
    h.key(Key::Enter);
    let w = h.window(WindowId::MAIN).unwrap();
    assert!(!w.damage.is_empty());
    for r in &w.damage {
        assert!(r.y >= row_rect(3).y && r.y + r.height <= row_rect(4).y + 12, "{r:?}");
    }
    // A key the app doesn't use draws nothing.
    let draws = h.window(WindowId::MAIN).unwrap().draws;
    h.key(Key::Escape);
    assert_eq!(h.window(WindowId::MAIN).unwrap().draws, draws);
}

#[test]
fn a_selection_redraws_only_the_rows_it_touches() {
    let mut h = harness();
    h.move_to(at(0, 1).0, at(0, 1).1);
    h.press(input::BUTTON_LEFT as u16);
    h.move_to(at(5, 1).0, at(5, 1).1);
    let w = h.window(WindowId::MAIN).unwrap();
    assert_eq!(w.damage, vec![row_rect(1)]);
    h.release(input::BUTTON_LEFT as u16);
}

#[test]
fn it_says_how_many_columns_and_rows_fit_when_the_size_changes() {
    let mut t = Term::new(5, 5);
    t.grid.cursor = None;
    let mut h = Harness::new(t, settings());
    // Told as soon as it is shown (`Event::Resized`): (200 - 16) / 6 columns, (100 - 12) / 12
    // rows; and again whenever the window's new size fits another number.
    assert_eq!(h.take_messages(), vec![Msg::Size(30, 7)]);
    h.move_to(2, 2);
    assert!(h.take_messages().is_empty());
    h.resize(300, 150);
    assert_eq!(h.take_messages(), vec![Msg::Size(47, 11)]);
    // A size that fits the same number says nothing.
    h.resize(301, 151);
    assert!(h.take_messages().is_empty());
    h.resize(10, 10);
    assert_eq!(h.take_messages(), vec![Msg::Size(1, 1)]);
    assert_eq!(h.app().grid.size, (1, 1));
}

#[test]
fn the_wheel_scrolls_by_lines() {
    let mut h = harness();
    h.scroll(50, 50, 2);
    h.scroll(50, 50, -1);
    assert_eq!(h.take_messages(), vec![Msg::Scroll(6), Msg::Scroll(-3)]);
}

#[test]
fn odd_sizes_and_cells_do_not_panic() {
    // No cells at all.
    let mut h = Harness::new(Term::new(0, 0), settings());
    h.click(50, 50);
    h.drag((10, 10), (190, 90));
    h.type_text("x");
    h.key(Key::Enter);
    h.shortcut(Shortcut::ctrl_shift('c'));
    h.scroll(50, 50, 1);
    // A grid far bigger than the view, with a tiny window.
    let mut t = Term::new(300, 300);
    t.grid.set(299, "end");
    t.grid.cursor = Some((299, 299));
    let mut h =
        Harness::new(t, Settings { size: (10, 10), resizable: true, min_size: (0, 0), over: None });
    h.click(5, 5);
    h.drag((0, 0), (9, 9));
    h.resize(1, 1);
    h.resize(2000, 1500);
    // A cursor outside the grid.
    h.app_mut().grid.cursor = Some((5000, 5000));
    h.refresh();
    term(&h);
}

fn term_for_pictures() -> Term {
    let mut t = Term::new(30, 7);
    t.grid.set(0, "PrettyBigRock 1.4  (c) 2026");
    t.grid.set(1, "Type 'help' for a list.");
    t.grid.set(2, "Unknown command 'frob'.");
    t.grid.set(3, "Hello, potato. <3");
    t.grid.set(4, "bold row of text");
    t.grid.set(5, "/home/chris");
    t.grid.set(6, "$ ls -l");
    t.grid.cursor = Some((7, 6));
    t
}

fn pictures(scale: u32, suffix: &str) {
    let mut h = Harness::with_scale(term_for_pictures(), settings(), scale);
    h.check_picture(&format!("terminal-unfocused-{suffix}"));
    h.key(Key::Tab);
    h.check_picture(&format!("terminal-focused-{suffix}"));
    h.drag(at(10, 1), at(6, 3));
    h.check_picture(&format!("terminal-selection-{suffix}"));
}

#[test]
fn the_terminal_at_1x() {
    pictures(1, "1x");
}

#[test]
fn the_terminal_at_2x() {
    pictures(2, "2x");
}

// ---------------------------------------------------------------------------------------------
// Canvas
// ---------------------------------------------------------------------------------------------

struct Board {
    version: u64,
    keys: bool,
    focusable: Option<bool>,
    size: (i32, i32),
}

const PAINT: Colour = Colour::solid(0x3355cc);

impl App for Board {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let version = self.version;
        let mut c = Canvas::new(self.size.0, self.size.1, move |canvas, r| {
            // The app's own drawing: a border, a square that moves with the version, a line.
            canvas.fill_rect(r, Colour::solid(0xffffff));
            canvas.stroke_rect(r, 1, Colour::solid(0x222222));
            let x = r.x + 10 + 8 * (version % 5) as i32;
            canvas.fill_rect(Rect::new(x, r.y + 10, 20, 20), PAINT);
            for i in 0..40 {
                canvas.fill_rect(Rect::new(r.x + 4 + i, r.y + r.height - 8 - i / 2, 1, 2), PAINT);
            }
        })
        .version(self.version)
        .cursor(pbr_window::Cursor::Crosshair)
        .on_pointer(|p| Some(Msg::Ptr(p)));
        if self.keys {
            c = c.on_key(|k, down| (k.key != Key::Tab).then_some(Msg::CanvasKey(k, down)));
        }
        if let Some(f) = self.focusable {
            c = c.focusable(f);
        }
        Row::new()
            .padding(8)
            .spacing(6)
            .push(c.id("canvas"))
            .push(Button::new("Clear").on_press(Msg::Clear))
            .into()
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        // The Clear button changes the picture.
        if m == Msg::Clear {
            self.version += 1;
        }
    }
}

fn board() -> Board {
    Board { version: 1, keys: true, focusable: None, size: (100, 60) }
}

fn board_settings() -> Settings {
    Settings { size: (200, 76), resizable: false, min_size: (0, 0), over: None }
}

fn pixel(h: &Harness<Board>, x: u32, y: u32) -> u32 {
    let p = h.picture(WindowId::MAIN);
    p.pixels[(y * p.width + x) as usize] & 0x00ff_ffff
}

fn pointers(msgs: Vec<Msg>) -> Vec<(PointerKind, i32, i32)> {
    msgs.into_iter()
        .filter_map(|m| match m {
            Msg::Ptr(p) => Some((p.kind, p.at.x, p.at.y)),
            _ => None,
        })
        .collect()
}

#[test]
fn the_canvas_draws_in_its_place_and_describes_itself() {
    let h = Harness::new(board(), board_settings());
    let d = h.find("canvas").unwrap().1;
    assert_eq!(d.what.role, "canvas");
    assert_eq!(d.what.value, "1");
    assert_eq!((d.rect.x, d.rect.y, d.rect.width, d.rect.height), (8, 8, 100, 60));
    // The white inside, the dark border, the square (version 1: 8 across), the page outside.
    assert_eq!(pixel(&h, 60, 50), 0xffffff);
    assert_eq!(pixel(&h, 8, 30), 0x222222);
    assert_eq!(pixel(&h, 8 + 10 + 8 + 2, 8 + 12), 0x3355cc);
    assert_eq!(pixel(&h, 4, 4), 0xf5f2ec);
}

#[test]
fn pointer_positions_are_relative_to_the_canvas() {
    let mut h = Harness::new(board(), board_settings());
    h.click(18, 28);
    let got = pointers(h.take_messages());
    let left = |k| matches!(k, PointerKind::Pressed { .. });
    assert!(got.iter().any(|(k, x, y)| left(*k) && (*x, *y) == (10, 20)), "{got:?}");
    assert!(got.iter().all(|(_, x, y)| (*x, *y) == (10, 20)));
    assert!(got.iter().any(|(k, ..)| matches!(k, PointerKind::Released { .. })));
    // Off the canvas, on the button's side: nothing.
    h.move_to(150, 40);
    assert!(pointers(h.take_messages()).is_empty());
    // The wheel over it.
    h.scroll(30, 30, 1);
    assert!(
        pointers(h.take_messages()).iter().any(|(k, ..)| matches!(k, PointerKind::Scrolled { .. }))
    );
}

#[test]
fn a_drag_that_leaves_the_canvas_still_reports_moves_and_the_release() {
    let mut h = Harness::new(board(), board_settings());
    h.move_to(50, 30);
    h.take_messages();
    h.press(input::BUTTON_LEFT as u16);
    h.move_to(150, 70);
    h.move_to(2, 2);
    h.release(input::BUTTON_LEFT as u16);
    let got = pointers(h.take_messages());
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(matches!(got[0].0, PointerKind::Pressed { .. }));
    assert_eq!((got[1].0, got[1].1, got[1].2), (PointerKind::Moved, 142, 62));
    assert_eq!((got[2].0, got[2].1, got[2].2), (PointerKind::Moved, -6, -6));
    assert!(matches!(got[3].0, PointerKind::Released { .. }));
    assert_eq!((got[3].1, got[3].2), (-6, -6));
    // Once let go, moves outside are not its.
    h.move_to(150, 40);
    assert!(pointers(h.take_messages()).is_empty());
}

#[test]
fn it_shows_the_pointer_shape_over_it() {
    let mut h = Harness::new(board(), board_settings());
    h.move_to(50, 30);
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Crosshair);
    h.move_to(190, 70);
    assert_eq!(h.window(WindowId::MAIN).unwrap().cursor, pbr_window::Cursor::Arrow);
}

#[test]
fn keys_and_releases_come_only_while_it_has_the_focus() {
    let mut h = Harness::new(board(), board_settings());
    h.type_text("a");
    assert!(h.take_messages().is_empty());
    h.click(30, 30);
    assert_eq!(h.focused().unwrap().what.role, "canvas");
    h.take_messages();
    h.type_text("a");
    h.key(Key::Left);
    let got: Vec<(Option<char>, Key, bool)> = h
        .take_messages()
        .into_iter()
        .filter_map(|m| match m {
            Msg::CanvasKey(k, down) => Some((k.ch, k.key, down)),
            _ => None,
        })
        .collect();
    assert_eq!(
        got,
        vec![
            (Some('a'), Key::Character, true),
            (Some('a'), Key::Character, false),
            (None, Key::Left, true),
            (None, Key::Left, false),
        ]
    );
    // Moving the focus to the button stops them.
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "button");
    h.take_messages();
    h.key(Key::Left);
    assert!(h.take_messages().iter().all(|m| !matches!(m, Msg::CanvasKey(..))));
}

#[test]
fn focus_follows_on_key_unless_told_otherwise() {
    // No keys: not focusable, so Tab goes to the button.
    let mut h = Harness::new(Board { keys: false, ..board() }, board_settings());
    h.click(30, 30);
    assert!(h.focused().is_none());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "button");
    // Keys but `focusable(false)`.
    let mut h = Harness::new(Board { focusable: Some(false), ..board() }, board_settings());
    h.click(30, 30);
    assert!(h.focused().is_none());
    // No keys but `focusable(true)`.
    let mut h =
        Harness::new(Board { keys: false, focusable: Some(true), ..board() }, board_settings());
    h.click(30, 30);
    assert_eq!(h.focused().unwrap().what.role, "canvas");
}

#[test]
fn only_a_new_version_makes_it_draw_again() {
    let mut h = Harness::new(board(), board_settings());
    let canvas = Rect::new(8, 8, 100, 60);
    // Something else changing draws that, not the canvas.
    let draws = h.window(WindowId::MAIN).unwrap().draws;
    h.move_to(150, 20);
    let w = h.window(WindowId::MAIN).unwrap();
    assert!(w.draws > draws);
    assert!(w.damage.iter().all(|r| r.x >= 114), "{:?}", w.damage);
    // The pointer moving over the canvas itself changes nothing of it.
    let draws = w.draws;
    h.move_to(30, 30);
    h.move_to(31, 31);
    assert!(
        h.window(WindowId::MAIN).unwrap().damage.iter().all(|r| r.x >= 108 || r.y >= 68),
        "{:?}",
        h.window(WindowId::MAIN).unwrap().damage
    );
    let _ = draws;
    // A new version: the canvas is drawn again (and the pressed button, which is the cause).
    h.click_on("Clear");
    assert_eq!(h.app().version, 2);
    assert_eq!(h.find("canvas").unwrap().1.what.value, "2");
    let w = h.window(WindowId::MAIN).unwrap();
    assert!(
        w.damage.iter().any(|r| r.intersect(&canvas) == canvas || *r == canvas),
        "{:?}",
        w.damage
    );
    assert_eq!(pixel(&h, 8 + 10 + 16 + 2, 8 + 12), 0x3355cc);
}

#[test]
fn zero_and_odd_sizes_do_not_panic() {
    for size in [(0, 0), (-5, 10), (1, 1), (5000, 5000)] {
        let mut h = Harness::new(Board { size, ..board() }, board_settings());
        h.click(10, 10);
        h.drag((0, 0), (190, 70));
        h.type_text("a");
        h.find("canvas");
    }
}

#[test]
fn the_canvas_at_1x_and_2x() {
    let mut h = Harness::new(board(), board_settings());
    h.check_picture("terminal-canvas-1x");
    let h2 = Harness::with_scale(board(), board_settings(), 2);
    h2.check_picture("terminal-canvas-2x");
    h.click_on("Clear");
    h.click_on("Clear");
    h.move_to(190, 70);
    h.check_picture("terminal-canvas-moved-1x");
}
