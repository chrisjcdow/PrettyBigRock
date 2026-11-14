// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `Scroll` and `List` through the harness: the wheel, thumbs and tracks, offsets kept in range
//! when the window or the content changes, clicks that land where a scrolled child now is (and
//! not where it isn't), both bars; a list's selection by pointer and by every key, activation,
//! sorting, keeping the selection in view, the empty text, ten thousand rows, and the look of
//! both at 1x and 2x (note 0023, sections 5 and 6).

use pbr_interfaces::pbr_input as input;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;
use pbr_window::{Event as Incoming, PointerEvent, PointerKind as WinPointer, Win};

// ---------------------------------------------------------------------------------------------
// Scroll
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Item(usize),
    Select(usize),
    Activate(usize),
    Sort(usize),
}

/// A column of buttons in a scroll area.
struct Scrolled {
    items: usize,
    /// 0: up and down, 1: across, 2: both.
    mode: u8,
    /// Buttons are 300 wide (so that there is something to scroll across).
    wide: bool,
    /// The scroll area is 100 high, with empty room below it.
    short: bool,
}

impl Scrolled {
    fn new(items: usize, mode: u8, wide: bool) -> Scrolled {
        Scrolled { items, mode, wide, short: false }
    }
}

impl App for Scrolled {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let mut col = Column::new();
        for i in 0..self.items {
            let mut b = Button::new(&format!("Item {i}")).on_press(Msg::Item(i));
            if self.wide {
                b = b.width(Length::Fixed(300));
            }
            col = col.push(b);
        }
        let mut s = match self.mode {
            0 => Scroll::new(col),
            1 => Scroll::new(col).horizontal(),
            _ => Scroll::new(col).both(),
        };
        if self.short {
            s = s.height(Length::Fixed(100));
        }
        Column::new().push(s.id("scroll")).into()
    }

    fn update(&mut self, _m: Msg, _cx: &mut Context<'_, Msg>) {}
}

fn small() -> Settings {
    Settings { size: (160, 120), resizable: true, min_size: (0, 0), over: None, system: false }
}

/// The scroll area's offsets, "x,y".
fn offsets<A: App>(h: &Harness<A>) -> String
where
    A::Message: std::fmt::Debug,
{
    h.find("scroll").expect("a scroll area").1.what.value
}

/// Turns the wheel sideways.
fn hwheel<A: App>(h: &mut Harness<A>, x: i32, y: i32, steps: i32)
where
    A::Message: std::fmt::Debug,
{
    h.move_to(x, y);
    // The first window the harness makes is number 1.
    h.send(Incoming::Pointer(PointerEvent {
        window: Win(1),
        what: WinPointer::Scroll { axis: input::AXIS_HWHEEL as u16, steps },
        x,
        y,
        raw_x: x * 256,
        raw_y: y * 256,
        buttons: 0,
        serial: 0,
    }));
}

#[test]
fn the_wheel_scrolls_by_steps_and_stops_at_the_ends() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    assert_eq!(h.find("scroll").unwrap().1.what.role, "scroll");
    assert_eq!(offsets(&h), "0,0");
    h.scroll(50, 50, 1);
    assert_eq!(offsets(&h), "0,33");
    h.scroll(50, 50, 2);
    assert_eq!(offsets(&h), "0,99");
    h.scroll(50, 50, -1);
    assert_eq!(offsets(&h), "0,66");
    // 20 buttons of 24 are 480 high, 120 show: the end is 360.
    h.scroll(50, 50, 100);
    assert_eq!(offsets(&h), "0,360");
    h.scroll(50, 50, -100);
    assert_eq!(offsets(&h), "0,0");
    // The wheel over the bar scrolls too.
    h.scroll(154, 60, 1);
    assert_eq!(offsets(&h), "0,33");
}

#[test]
fn dragging_the_thumb_scrolls_in_proportion() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    // The bar is 12 wide at the right (x 148 to 160); the thumb is 120 * 120 / 480 = 30 high,
    // and travels 90 for 360 of scrolling.
    h.drag((154, 15), (154, 60));
    assert_eq!(offsets(&h), "0,180");
    h.drag((154, 70), (154, 500));
    assert_eq!(offsets(&h), "0,360");
    h.drag((154, 110), (154, -50));
    assert_eq!(offsets(&h), "0,0");
    // Nothing was clicked on the way.
    assert!(h.messages().is_empty());
}

#[test]
fn a_thumb_let_go_outside_the_window_stops_following_the_pointer() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    h.move_to(154, 10);
    h.press(input::BUTTON_LEFT as u16);
    h.move_to(154, 40);
    let dragged = offsets(&h);
    // The button went up where the window heard nothing: the next move says no button is held.
    h.send(Incoming::Pointer(PointerEvent {
        window: Win(1),
        what: WinPointer::Moved,
        x: 154,
        y: 100,
        raw_x: 154 * 256,
        raw_y: 100 * 256,
        buttons: 0,
        serial: 0,
    }));
    assert_eq!(offsets(&h), dragged);
}

#[test]
fn clicking_the_track_scrolls_by_a_page() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    // A page is the view's 120 less one wheel step.
    h.click(154, 100);
    assert_eq!(offsets(&h), "0,87");
    h.click(154, 115);
    assert_eq!(offsets(&h), "0,174");
    // The thumb is now at 87 * 90 / 360 = 21 to 51: above it goes back.
    h.click(154, 5);
    h.click(154, 5);
    h.click(154, 5);
    assert_eq!(offsets(&h), "0,0");
    assert!(h.messages().is_empty());
}

#[test]
fn offsets_are_brought_within_range_when_the_window_or_the_content_changes() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    h.scroll(50, 50, 100);
    assert_eq!(offsets(&h), "0,360");
    h.resize(160, 300);
    assert_eq!(offsets(&h), "0,180");
    h.resize(160, 600);
    assert_eq!(offsets(&h), "0,0");
    // No bar now: the track's place is just the area.
    h.scroll(50, 50, 1);
    assert_eq!(offsets(&h), "0,0");
    h.resize(160, 120);
    h.scroll(50, 50, 100);
    assert_eq!(offsets(&h), "0,360");
    h.app_mut().items = 5;
    h.refresh();
    assert_eq!(offsets(&h), "0,0");
    h.app_mut().items = 20;
    h.refresh();
    h.scroll(50, 50, 100);
    h.app_mut().items = 10;
    h.refresh();
    // 10 buttons are 240 high: the end is 120.
    assert_eq!(offsets(&h), "0,120");
}

#[test]
fn a_button_in_a_scrolled_column_is_clicked_where_it_now_is() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    h.scroll(50, 50, 5);
    assert_eq!(offsets(&h), "0,165");
    let at = h.find("Item 10").unwrap().1.rect;
    assert_eq!(at.y, 240 - 165);
    h.click_on("Item 10");
    assert_eq!(h.take_messages(), vec![Msg::Item(10)]);
    // One that has scrolled out of view can't be clicked, wherever its rectangle is.
    h.click_on("Item 0");
    h.click_on("Item 19");
    assert!(h.messages().is_empty());
}

#[test]
fn nothing_outside_the_viewport_is_clickable() {
    let mut app = Scrolled::new(20, 0, false);
    app.short = true;
    let mut h = Harness::new(app, Settings { size: (160, 150), ..small() });
    // Item 4 is at 96 to 120; the area ends at 100. Below that is empty room.
    h.click(30, 110);
    assert!(h.messages().is_empty());
    h.click(30, 98);
    assert_eq!(h.take_messages(), vec![Msg::Item(4)]);
    // Over the bar, and in the room beyond the area.
    h.click(154, 98);
    h.click(30, 140);
    assert!(h.messages().is_empty());
}

#[test]
fn a_button_pressed_in_the_area_still_hears_the_release_outside_it() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    let r = h.find("Item 2").unwrap().1.rect;
    h.move_to(r.x + 5, r.y + 5);
    h.press(input::BUTTON_LEFT as u16);
    // Out over the bar and beyond the window, then back in over it.
    h.move_to(154, 60);
    h.move_to(300, 300);
    h.move_to(r.x + 5, r.y + 5);
    h.release(input::BUTTON_LEFT as u16);
    assert_eq!(h.take_messages(), vec![Msg::Item(2)]);
    // Pressed, then let go away from it: no click.
    h.move_to(r.x + 5, r.y + 5);
    h.press(input::BUTTON_LEFT as u16);
    h.move_to(154, 60);
    h.release(input::BUTTON_LEFT as u16);
    assert!(h.messages().is_empty());
}

#[test]
fn leaving_the_viewport_clears_what_the_pointer_was_over() {
    let plain = Harness::new(Scrolled::new(20, 0, false), small());
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    h.move_to(30, 12);
    assert_ne!(h.picture(WindowId::MAIN), plain.picture(WindowId::MAIN), "no hover shown");
    h.move_to(154, 60);
    assert_eq!(h.picture(WindowId::MAIN), plain.picture(WindowId::MAIN));
}

#[test]
fn both_bars_make_room_for_each_other() {
    let mut h = Harness::new(Scrolled::new(20, 2, true), small());
    // 300 by 480 of buttons in 160 by 120: the vertical bar leaves 148 across, the horizontal
    // one leaves 108 down. The ends are 152 and 372.
    h.scroll(50, 50, 1000);
    assert_eq!(offsets(&h), "0,372");
    hwheel(&mut h, 50, 50, 1000);
    assert_eq!(offsets(&h), "152,372");
    hwheel(&mut h, 50, 50, -1);
    assert_eq!(offsets(&h), "119,372");
    h.scroll(50, 50, -1000);
    hwheel(&mut h, 50, 50, -1000);
    assert_eq!(offsets(&h), "0,0");
    // The horizontal thumb: 148 * 148 / 300 = 73 wide, travelling 75 for 152.
    h.drag((30, 114), (60, 114));
    assert_eq!(offsets(&h), "60,0");
    // A click in the corner where the bars meet does nothing.
    h.click(154, 114);
    assert_eq!(offsets(&h), "60,0");
    // The horizontal track's page: 148 less a step.
    h.click(140, 114);
    assert_eq!(offsets(&h), "152,0");
    // Buttons scrolled both ways are still where they seem.
    h.scroll(50, 50, 2);
    let at = h.find("Item 5").unwrap().1.rect;
    assert_eq!((at.x, at.y), (-152, 120 - 66));
    h.click(20, 120 - 66 + 5);
    assert_eq!(h.take_messages(), vec![Msg::Item(5)]);
}

#[test]
fn with_only_a_horizontal_bar_the_wheel_scrolls_across() {
    let mut h = Harness::new(Scrolled::new(3, 1, true), small());
    h.scroll(50, 50, 1);
    assert_eq!(offsets(&h), "33,0");
    hwheel(&mut h, 50, 50, 1);
    assert_eq!(offsets(&h), "66,0");
    h.scroll(50, 50, -5);
    assert_eq!(offsets(&h), "0,0");
}

#[test]
fn a_wheel_over_an_area_with_nothing_to_scroll_is_left_alone() {
    let mut h = Harness::new(Scrolled::new(3, 0, false), small());
    // Three buttons are 72 high in 120: no bar, nothing used.
    h.scroll(50, 50, 3);
    assert_eq!(offsets(&h), "0,0");
}

#[test]
fn page_keys_scroll_when_the_focus_is_inside_and_nothing_else_uses_them() {
    let mut h = Harness::new(Scrolled::new(20, 0, false), small());
    // No focus: the keys aren't ours.
    h.key(Key::End);
    assert_eq!(offsets(&h), "0,0");
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Item 0");
    h.key(Key::PageDown);
    assert_eq!(offsets(&h), "0,87");
    h.key(Key::PageDown);
    assert_eq!(offsets(&h), "0,174");
    h.key(Key::PageUp);
    assert_eq!(offsets(&h), "0,87");
    h.key(Key::End);
    assert_eq!(offsets(&h), "0,360");
    h.key(Key::Home);
    assert_eq!(offsets(&h), "0,0");
    // A shortcut isn't a request to scroll.
    h.key_with(Key::End, Modifiers(Modifiers::CTRL));
    assert_eq!(offsets(&h), "0,0");
    // Space still presses the focused button.
    h.key(Key::Space);
    assert_eq!(h.take_messages(), vec![Msg::Item(0)]);
}

#[test]
fn page_keys_go_across_when_there_is_only_a_horizontal_bar() {
    let mut h = Harness::new(Scrolled::new(3, 1, true), small());
    h.key(Key::Tab);
    h.key(Key::End);
    assert_eq!(offsets(&h), "140,0");
    h.key(Key::Home);
    assert_eq!(offsets(&h), "0,0");
}

#[test]
fn a_scroll_area_too_small_for_its_bars_does_not_panic() {
    let mut h = Harness::new(Scrolled::new(20, 2, true), Settings { size: (5, 5), ..small() });
    h.scroll(2, 2, 3);
    hwheel(&mut h, 2, 2, 3);
    h.click(2, 2);
    h.drag((1, 1), (4, 4));
    h.key(Key::Tab);
    h.key(Key::PageDown);
    h.resize(1, 1);
    h.scroll(0, 0, 1);
    h.click(0, 0);
    h.resize(160, 120);
    h.resize(11, 200);
    h.key(Key::End);
    h.key(Key::Home);
    h.app_mut().items = 0;
    h.refresh();
    assert_eq!(offsets(&h), "0,0");
}

// ---------------------------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------------------------

struct Entry {
    name: String,
    size: String,
    date: String,
    dir: bool,
}

/// A 16 by 16 folder, drawn in the test so that no picture file is needed.
fn folder_icon() -> Vec<u32> {
    let mut p = vec![0u32; 256];
    let mut set = |x0: usize, y0: usize, x1: usize, y1: usize, c: u32| {
        for y in y0..y1 {
            for x in x0..x1 {
                p[y * 16 + x] = c;
            }
        }
    };
    set(1, 3, 7, 5, 0xFF9A_7020);
    set(1, 4, 15, 14, 0xFF9A_7020);
    set(2, 5, 14, 13, 0xFFE8_B848);
    set(2, 5, 14, 7, 0xFFF4_D07A);
    p
}

/// A 16 by 16 sheet of paper.
fn file_icon() -> Vec<u32> {
    let mut p = vec![0u32; 256];
    let mut set = |x0: usize, y0: usize, x1: usize, y1: usize, c: u32| {
        for y in y0..y1 {
            for x in x0..x1 {
                p[y * 16 + x] = c;
            }
        }
    };
    set(3, 1, 13, 15, 0xFF5A_5F6A);
    set(4, 2, 12, 14, 0xFFFF_FFFF);
    for y in [5, 7, 9, 11] {
        set(5, y, 11, y + 1, 0xFF8A_8F98);
    }
    p
}

/// A folder's entries in a list.
struct Files {
    rows: Vec<Entry>,
    selected: Option<usize>,
    sorted: Option<(usize, bool)>,
    icons: bool,
    folder: Vec<u32>,
    file: Vec<u32>,
}

impl Files {
    fn new(n: usize) -> Files {
        let rows = (0..n)
            .map(|i| Entry {
                name: if i % 4 == 0 { format!("Folder {i}") } else { format!("file{i}.txt") },
                size: if i % 4 == 0 { String::new() } else { format!("{} KB", i * 3) },
                date: format!("2026-11-{:02}", i % 28 + 1),
                dir: i % 4 == 0,
            })
            .collect();
        Files {
            rows,
            selected: None,
            sorted: None,
            icons: true,
            folder: folder_icon(),
            file: file_icon(),
        }
    }
}

impl App for Files {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let columns = vec![
            ListColumn::new("Name", Length::Fill),
            ListColumn::new("Size", Length::Fixed(80)),
            ListColumn::new("Modified", Length::Fixed(100)),
        ];
        let rows = self
            .rows
            .iter()
            .map(|e| {
                let icon = if e.dir { &self.folder } else { &self.file };
                ListRow::new()
                    .icon_maybe(if self.icons { Image::new(icon, 16, 16) } else { None })
                    .cell(e.name.as_str())
                    .cell(e.size.as_str())
                    .cell(e.date.as_str())
            })
            .collect();
        let mut list = List::new(columns, rows)
            .selected(self.selected)
            .on_select(Msg::Select)
            .on_activate(Msg::Activate)
            .on_sort(Msg::Sort)
            .empty("This folder is empty.");
        if let Some((c, asc)) = self.sorted {
            list = list.sorted(c, asc);
        }
        list.id("files")
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Select(i) => self.selected = Some(i),
            Msg::Sort(c) => {
                let asc = match self.sorted {
                    Some((s, asc)) if s == c => !asc,
                    _ => true,
                };
                self.sorted = Some((c, asc));
            }
            Msg::Activate(_) | Msg::Item(_) => {}
        }
    }
}

fn list_settings() -> Settings {
    Settings { size: (300, 140), resizable: true, min_size: (0, 0), over: None, system: false }
}

/// Where the middle of row `n` is, when nothing is scrolled (the list's rows start at 22, and
/// are 20 high).
fn row_y(n: i32) -> i32 {
    22 + 20 * n + 10
}

/// A click a while after the last one, so that it is never counted as the second of a double
/// click (the harness's clock only moves when a test says so).
fn click_later<A: App>(h: &mut Harness<A>, x: i32, y: i32)
where
    A::Message: std::fmt::Debug,
{
    h.advance(1000);
    h.click(x, y);
}

#[test]
fn a_list_describes_itself() {
    let mut h = Harness::new(Files::new(5), list_settings());
    let d = h.find("files").unwrap().1;
    assert_eq!(d.what.role, "list");
    assert_eq!(d.what.label, "Name|Size|Modified");
    assert_eq!(d.what.value, "");
    h.click(50, row_y(1));
    let d = h.find("files").unwrap().1;
    assert_eq!(d.what.value, "file1.txt|3 KB|2026-11-02");
    assert!(d.focused);
}

#[test]
fn a_click_selects_and_takes_the_focus() {
    let mut h = Harness::new(Files::new(5), list_settings());
    assert!(h.focused().is_none());
    h.click(50, row_y(2));
    assert_eq!(h.take_messages(), vec![Msg::Select(2)]);
    assert_eq!(h.app().selected, Some(2));
    assert_eq!(h.focused().unwrap().what.role, "list");
    // Another row.
    h.click(250, row_y(0));
    assert_eq!(h.take_messages(), vec![Msg::Select(0)]);
    // The room below the last row (rows end at 122, the list at 139): no selection.
    click_later(&mut h, 50, 130);
    assert!(h.messages().is_empty());
    assert_eq!(h.app().selected, Some(0));
    // Outside the list altogether (the heading's line and the edge).
    h.click(0, 0);
    assert!(h.messages().is_empty());
}

#[test]
fn a_right_click_selects_the_row_too() {
    let mut h = Harness::new(Files::new(5), list_settings());
    h.right_click_in(WindowId::MAIN, 50, row_y(3));
    assert_eq!(h.take_messages(), vec![Msg::Select(3)]);
    assert_eq!(h.app().selected, Some(3));
    assert!(h.focused().is_some());
}

#[test]
fn a_double_click_activates_and_enter_activates_the_selected_row() {
    let mut h = Harness::new(Files::new(5), list_settings());
    h.double_click(50, row_y(2));
    assert_eq!(h.take_messages(), vec![Msg::Select(2), Msg::Activate(2)]);
    h.key(Key::Enter);
    assert_eq!(h.take_messages(), vec![Msg::Activate(2)]);
    // Two slow clicks aren't a double click.
    h.click(50, row_y(1));
    h.advance(2000);
    h.click(50, row_y(1));
    assert_eq!(h.take_messages(), vec![Msg::Select(1), Msg::Select(1)]);
}

#[test]
fn enter_with_nothing_selected_is_left_for_the_window() {
    let mut h = Harness::new(Files::new(5), list_settings());
    // 5 rows end at 122; the list ends at 139.
    h.click(50, 130);
    assert!(h.focused().is_some());
    h.key(Key::Enter);
    assert!(h.messages().is_empty());
}

#[test]
fn the_arrow_keys_move_the_selection() {
    let mut h = Harness::new(Files::new(100), list_settings());
    // Not focused: keys do nothing.
    h.key(Key::Down);
    assert!(h.messages().is_empty());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.role, "list");
    // Nothing selected: Down selects the first row.
    h.key(Key::Down);
    assert_eq!(h.take_messages(), vec![Msg::Select(0)]);
    // At the top, Up has nowhere to go.
    h.key(Key::Up);
    assert!(h.messages().is_empty());
    h.key(Key::Down);
    h.key(Key::Down);
    assert_eq!(h.app().selected, Some(2));
    h.key(Key::Up);
    assert_eq!(h.app().selected, Some(1));
    // A page is what shows (117 high: 5 rows) less one.
    h.key(Key::PageDown);
    assert_eq!(h.app().selected, Some(5));
    h.key(Key::PageUp);
    assert_eq!(h.app().selected, Some(1));
    h.key(Key::PageUp);
    assert_eq!(h.app().selected, Some(0));
    h.key(Key::End);
    assert_eq!(h.app().selected, Some(99));
    h.take_messages();
    h.key(Key::End);
    h.key(Key::Down);
    h.key(Key::PageDown);
    assert!(h.messages().is_empty());
    h.key(Key::Home);
    assert_eq!(h.take_messages(), vec![Msg::Select(0)]);
    // A shortcut's keys aren't ours.
    h.key_with(Key::Down, Modifiers(Modifiers::CTRL));
    assert!(h.messages().is_empty());
}

#[test]
fn with_nothing_selected_up_and_page_up_start_from_the_end() {
    let mut h = Harness::new(Files::new(30), list_settings());
    h.key(Key::Tab);
    h.key(Key::Up);
    assert_eq!(h.take_messages(), vec![Msg::Select(29)]);
    let mut h = Harness::new(Files::new(30), list_settings());
    h.key(Key::Tab);
    h.key(Key::PageDown);
    assert_eq!(h.take_messages(), vec![Msg::Select(0)]);
}

#[test]
fn the_selected_row_is_kept_in_view() {
    let mut h = Harness::new(Files::new(100), list_settings());
    h.click(50, row_y(0));
    for _ in 0..10 {
        h.key(Key::Down);
    }
    assert_eq!(h.app().selected, Some(10));
    // Row 10 is at 22 + 200 = 222: scrolled up by 103 it ends at the list's bottom, 119 to 139.
    click_later(&mut h, 50, 129);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(10)));
    // Going back up brings the row to the top.
    for _ in 0..8 {
        h.key(Key::Up);
    }
    assert_eq!(h.app().selected, Some(2));
    click_later(&mut h, 50, 22 + 5);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(2)));
    // The app may move the selection by itself.
    h.app_mut().selected = Some(60);
    h.refresh();
    click_later(&mut h, 50, 129);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(60)));
    // And the last row, with the end of the list at the bottom.
    h.key(Key::End);
    click_later(&mut h, 50, 129);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(99)));
}

#[test]
fn scrolling_away_from_the_selection_is_not_undone_by_the_next_view() {
    let mut h = Harness::new(Files::new(100), list_settings());
    h.click(50, row_y(0));
    h.scroll(50, 60, 3);
    // A step is 33: 99 down. Row at y = 129 is (129 - 22 + 99) / 20 = 10.
    click_later(&mut h, 50, 129);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(10)));
}

#[test]
fn the_wheel_and_the_thumb_scroll_the_rows() {
    let mut h = Harness::new(Files::new(100), list_settings());
    h.scroll(50, 60, 1);
    click_later(&mut h, 50, 129);
    // (129 - 22 + 33) / 20
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(7)));
    h.scroll(50, 60, 1000);
    click_later(&mut h, 50, 129);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(99)));
    h.scroll(50, 60, -1000);
    click_later(&mut h, 50, 32);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(0)));
    // The thumb: 117 * 117 / 2000 is 6, so the shortest, 16; the bar is at x 287 to 299.
    h.drag((293, 30), (293, 130));
    click_later(&mut h, 50, 129);
    let last = h.take_messages();
    let Some(Msg::Select(n)) = last.last() else {
        panic!("no selection");
    };
    assert!(*n > 80, "dragged far down but row {n}");
    // A click on the track below the thumb is a page: 117 - 33 = 84.
    h.scroll(50, 60, -1000);
    click_later(&mut h, 293, 120);
    click_later(&mut h, 50, 32);
    assert_eq!(h.take_messages().last(), Some(&Msg::Select(4)));
    // Clicks on the bar never select.
    assert!(h.messages().is_empty());
}

#[test]
fn a_wheel_over_a_list_that_fits_is_not_used() {
    let mut h = Harness::new(Files::new(3), list_settings());
    h.scroll(50, 60, 3);
    h.click(50, row_y(2));
    assert_eq!(h.take_messages(), vec![Msg::Select(2)]);
}

#[test]
fn a_click_on_a_heading_asks_to_sort_by_it() {
    let mut h = Harness::new(Files::new(5), list_settings());
    // The columns, in 298 less the fixed 80 and 100: Name 1 to 119, Size to 199, Modified to 299.
    h.click(30, 10);
    h.click(150, 10);
    h.click(250, 10);
    assert_eq!(h.take_messages(), vec![Msg::Sort(0), Msg::Sort(1), Msg::Sort(2)]);
    assert_eq!(h.app().sorted, Some((2, true)));
    h.click(250, 10);
    assert_eq!(h.app().sorted, Some((2, false)));
    // A heading takes the focus, and selects nothing.
    assert!(h.focused().is_some());
    assert_eq!(h.app().selected, None);
}

#[test]
fn the_heading_stays_while_the_rows_scroll() {
    let mut h = Harness::new(Files::new(100), list_settings());
    h.scroll(50, 60, 5);
    h.click(150, 10);
    assert_eq!(h.take_messages(), vec![Msg::Sort(1)]);
}

#[test]
fn an_empty_list_says_so_and_keeps_quiet() {
    let mut h = Harness::new(Files::new(0), list_settings());
    h.key(Key::Tab);
    for k in [Key::Down, Key::Up, Key::PageDown, Key::PageUp, Key::Home, Key::End, Key::Enter] {
        h.key(k);
    }
    h.click(50, row_y(0));
    h.right_click_in(WindowId::MAIN, 50, row_y(0));
    h.scroll(50, 60, 2);
    assert!(h.messages().is_empty());
    assert_eq!(h.find("files").unwrap().1.what.value, "");
    h.click(150, 10);
    assert_eq!(h.take_messages(), vec![Msg::Sort(1)]);
}

#[test]
fn ten_thousand_rows_cost_what_a_screenful_does() {
    let mut h = Harness::new(Files::new(10_000), list_settings());
    let start = std::time::Instant::now();
    h.click(50, row_y(0));
    h.key(Key::End);
    assert_eq!(h.app().selected, Some(9_999));
    h.key(Key::Home);
    h.key(Key::PageDown);
    h.scroll(50, 60, 500);
    h.drag((293, 30), (293, 100));
    h.key(Key::Up);
    assert!(start.elapsed().as_secs() < 30, "took {:?}", start.elapsed());
    let d = h.find("files").unwrap().1;
    assert_eq!(d.what.role, "list");
    // A change of one row's text redraws just the list, not more: the window is only it.
    h.app_mut().rows[0].name = "renamed".to_string();
    h.refresh();
}

#[test]
fn rows_without_cells_or_with_too_many_do_not_panic() {
    struct Odd;
    impl App for Odd {
        type Message = Msg;
        fn view(&self) -> Element<'_, Msg> {
            let long = "x".repeat(5000);
            List::new(
                vec![
                    ListColumn::new(
                        "A very long title indeed for a narrow column",
                        Length::Fixed(30),
                    ),
                    ListColumn::new("", Length::Portion(2)),
                    ListColumn::new("Shrinking", Length::Shrink),
                ],
                vec![
                    ListRow::new(),
                    ListRow::new().cell(long.clone()).cell("b").cell("c").cell("d").cell("e"),
                    ListRow::new().cell("").cell(long),
                ],
            )
            .selected(Some(99))
            .sorted(7, false)
            .on_select(Msg::Select)
            .into()
        }
        fn update(&mut self, _m: Msg, _cx: &mut Context<'_, Msg>) {}
    }
    let mut h = Harness::new(Odd, list_settings());
    h.key(Key::Tab);
    h.key(Key::Down);
    h.click(10, row_y(1));
    h.click(10, 10);
    h.resize(3, 3);
    h.click(1, 1);
    h.key(Key::End);
    h.resize(30, 30);
    h.scroll(5, 25, 4);
    h.resize(300, 140);
    assert!(!h.messages().is_empty());
}

// ---------------------------------------------------------------------------------------------
// Pictures
// ---------------------------------------------------------------------------------------------

/// Checks `name-1x` and `name-2x`, each made by running `steps` on a fresh harness at that
/// scale.
fn shots<A: App>(
    name: &str,
    make: impl Fn() -> A,
    settings: Settings,
    steps: impl Fn(&mut Harness<A>),
) where
    A::Message: std::fmt::Debug,
{
    for scale in [1, 2] {
        let mut h = Harness::with_scale(make(), settings.clone(), scale);
        steps(&mut h);
        h.check_picture(&format!("{name}-{scale}x"));
    }
}

#[test]
fn a_scrolled_column_with_both_bars() {
    shots(
        "list-scroll-both",
        || Scrolled::new(20, 2, true),
        small(),
        |h| {
            h.scroll(50, 50, 2);
            hwheel(h, 50, 50, 1);
        },
    );
}

#[test]
fn a_scroll_area_with_one_bar_at_the_start_and_with_the_thumb_held() {
    shots("list-scroll-start", || Scrolled::new(20, 0, false), small(), |_| {});
    shots(
        "list-scroll-held",
        || Scrolled::new(20, 0, false),
        small(),
        |h| {
            h.move_to(154, 10);
            h.press(input::BUTTON_LEFT as u16);
            h.move_to(154, 40);
        },
    );
}

#[test]
fn a_list_with_icons() {
    shots("list-icons", || Files::new(5), list_settings(), |_| {});
}

#[test]
fn a_selected_row_focused_and_not() {
    shots(
        "list-selected-focused",
        || Files::new(5),
        list_settings(),
        |h| {
            h.click(50, row_y(1));
        },
    );
    shots(
        "list-selected",
        || Files { selected: Some(1), ..Files::new(5) },
        list_settings(),
        |_| {},
    );
}

#[test]
fn a_sort_arrow_after_the_title() {
    shots(
        "list-sorted-up",
        || Files { sorted: Some((0, true)), selected: Some(2), ..Files::new(5) },
        list_settings(),
        |_| {},
    );
    shots(
        "list-sorted-down",
        || Files { sorted: Some((2, false)), ..Files::new(5) },
        list_settings(),
        |_| {},
    );
}

#[test]
fn the_empty_list() {
    shots("list-empty", || Files::new(0), list_settings(), |_| {});
}

#[test]
fn a_list_scrolled_to_its_middle() {
    shots(
        "list-middle",
        || Files::new(60),
        list_settings(),
        |h| {
            h.scroll(50, 60, 17);
            h.click(50, 60);
        },
    );
}

#[test]
fn a_list_without_icons() {
    shots(
        "list-plain",
        || Files { icons: false, selected: Some(0), ..Files::new(4) },
        list_settings(),
        |_| {},
    );
}

/// A list whose second column fits its title, to check that a click on a heading is matched
/// with the columns as the theme's font laid them out.
struct Shrunk;

impl App for Shrunk {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let columns =
            vec![ListColumn::new("Name", Length::Fill), ListColumn::new("Kind", Length::Shrink)];
        List::new(columns, vec![ListRow::new().cell("a").cell("b")]).on_sort(Msg::Sort).into()
    }

    fn update(&mut self, _m: Msg, _cx: &mut Context<'_, Msg>) {}
}

#[test]
fn a_column_that_fits_its_title_is_found_by_a_click_on_its_heading() {
    let mut h = Harness::new(Shrunk, list_settings());
    // "Kind" in bold is 4 cells of 7 with 6 either side: 40 wide, at the right of the list.
    h.click(285, 10);
    h.click(100, 10);
    assert_eq!(h.take_messages(), vec![Msg::Sort(1), Msg::Sort(0)]);
}
