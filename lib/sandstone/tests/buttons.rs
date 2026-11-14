// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `Icon`, `IconButton`, `ToggleButton` and `CheckBox` through the harness: what each sends on
//! a click, on Space, when greyed out and when let go of elsewhere; that toggles and check
//! boxes flip the app's value and show it; Tab order; descriptions; and their look at 1x and
//! 2x in every state that looks different (note 0023, sections 5 and 6).

use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;

/// A 16 x 16 icon of our own: a red disc with a blue ring, on clear pixels, with a half-clear
/// stripe across it so that alpha shows.
fn icon_pixels() -> Vec<u32> {
    let mut p = vec![0u32; 256];
    for y in 0..16i32 {
        for x in 0..16i32 {
            let d = (2 * x - 15).pow(2) + (2 * y - 15).pow(2);
            p[(y * 16 + x) as usize] = if d <= 100 {
                0xFFD2_2B3C
            } else if d <= 196 {
                0xFF2B_5CD2
            } else {
                0
            };
        }
    }
    for x in 0..16 {
        p[7 * 16 + x] = 0x802B_8A3E;
        p[8 * 16 + x] = 0x802B_8A3E;
    }
    p
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Open,
    Bold(bool),
    Wrap(bool),
    Snap(bool),
    Pen(bool),
}

struct Demo {
    pixels: Vec<u32>,
    bold: bool,
    wrap: bool,
    snap: bool,
    pen: bool,
    /// The harness's pictures show these states.
    open_enabled: bool,
}

impl Demo {
    fn new() -> Demo {
        Demo {
            pixels: icon_pixels(),
            bold: false,
            wrap: true,
            snap: false,
            pen: true,
            open_enabled: false,
        }
    }
}

impl App for Demo {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let icon = Image::new(&self.pixels, 16, 16).expect("a 16 x 16 icon");
        let toolbar = Row::new()
            .spacing(2)
            .padding(3)
            .push(Icon::new(icon).label("Logo"))
            .push(Icon::new(icon).size(32, 32))
            .push(IconButton::new(icon).on_press(Msg::Open).id("open"))
            .push(IconButton::new(icon).label("Open").on_press(Msg::Open).id("open-label"))
            .push(IconButton::new(icon).on_press_maybe(self.open_enabled.then_some(Msg::Open)))
            .push(IconButton::new(icon).label("Save"));
        Column::new()
            .padding(8)
            .spacing(6)
            .push(
                Panel::new(toolbar).background(Theme::STANDARD.bar).edge(Theme::STANDARD.bar_edge),
            )
            .push(
                Row::new()
                    .spacing(6)
                    .push(ToggleButton::new(self.bold).label("Bold").on_toggle(Msg::Bold))
                    .push(ToggleButton::new(true).label("Pen").on_toggle(Msg::Pen))
                    .push(ToggleButton::new(self.pen).image(icon).on_toggle(Msg::Pen).id("pen"))
                    .push(ToggleButton::new(true).image(icon).label("Fill").on_toggle(Msg::Pen))
                    .push(ToggleButton::new(false).label("Grey")),
            )
            .push(
                Row::new()
                    .spacing(6)
                    .push(CheckBox::new("Wrap lines", self.wrap).on_toggle(Msg::Wrap))
                    .push(CheckBox::new("Snap", self.snap).on_toggle(Msg::Snap))
                    .push(CheckBox::new("Off", false))
                    .push(CheckBox::new("On", true)),
            )
            .into()
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Open => {}
            Msg::Bold(b) => self.bold = b,
            Msg::Wrap(b) => self.wrap = b,
            Msg::Snap(b) => self.snap = b,
            Msg::Pen(b) => self.pen = b,
        }
    }
}

fn settings() -> Settings {
    Settings { size: (340, 118), resizable: false, min_size: (0, 0), over: None, system: false }
}

fn demo() -> Harness<Demo> {
    Harness::new(Demo::new(), settings())
}

fn centre(h: &Harness<Demo>, key: &str) -> (i32, i32) {
    let r = h.find(key).expect("widget").1.rect;
    (r.x + r.width / 2, r.y + r.height / 2)
}

#[test]
fn sizes_are_as_briefed() {
    let h = demo();
    let open = h.find("open").unwrap().1.rect;
    assert_eq!((open.width, open.height), (26, 26));
    // 6 + 16 + 4 + "Open" (4 x 6) + 6.
    let labelled = h.find("open-label").unwrap().1.rect;
    assert_eq!((labelled.width, labelled.height), (6 + 16 + 4 + 24 + 6, 26));
    let bold = h.find("Bold").unwrap().1.rect;
    assert_eq!((bold.width, bold.height), (64, 24));
    let pen = h.find("pen").unwrap().1.rect;
    assert_eq!((pen.width, pen.height), (26, 24));
    let wrap = h.find("Wrap lines").unwrap().1.rect;
    assert_eq!((wrap.width, wrap.height), (14 + 6 + 60, 24));
    let icons: Vec<_> =
        h.describe(WindowId::MAIN).into_iter().filter(|d| d.what.role == "icon").collect();
    assert_eq!(icons.len(), 2);
    assert_eq!((icons[0].rect.width, icons[0].rect.height), (16, 16));
    assert_eq!((icons[1].rect.width, icons[1].rect.height), (32, 32));
    assert_eq!(icons[0].what.label, "Logo");
}

#[test]
fn icon_buttons_send_on_a_click_and_not_when_greyed_out_or_let_go_elsewhere() {
    let mut h = demo();
    h.click_on("open");
    h.click_on("open-label");
    assert_eq!(h.take_messages(), vec![Msg::Open, Msg::Open]);

    // Pressed over it, let go away from it: nothing.
    let r = h.find("open").unwrap().1.rect;
    h.move_to(r.x + 5, r.y + 5);
    h.press(1);
    h.move_to(r.x + 5, r.y + 80);
    h.release(1);
    assert!(h.messages().is_empty());

    // Pressed elsewhere, let go over it: nothing.
    h.move_to(r.x + 5, r.y + 80);
    h.press(1);
    h.move_to(r.x + 5, r.y + 5);
    h.release(1);
    assert!(h.messages().is_empty());

    // The greyed out ones (no message): the third icon button and "Save".
    let disabled: Vec<_> = h
        .describe(WindowId::MAIN)
        .into_iter()
        .filter(|d| d.what.role == "button" && !d.what.enabled)
        .collect();
    assert_eq!(disabled.len(), 2);
    for d in disabled {
        h.click_in(WindowId::MAIN, d.rect.x + 5, d.rect.y + 5);
    }
    assert!(h.messages().is_empty());
}

#[test]
fn icon_buttons_never_take_the_focus_and_are_not_in_the_tab_order() {
    let mut h = demo();
    h.click_on("open");
    assert!(h.focused().is_none());
    h.key(Key::Space);
    h.key(Key::Enter);
    assert!(h.messages().is_empty() || h.messages().iter().all(|m| *m == Msg::Open));
    assert_eq!(h.messages().len(), 1, "only the click, not Space or Enter");
    let mut order = Vec::new();
    for _ in 0..6 {
        h.key(Key::Tab);
        let f = h.focused().unwrap();
        order.push((f.what.role, f.what.label));
    }
    let toggles = |l: &str| ("toggle button", l.to_string());
    assert_eq!(
        order,
        vec![
            toggles("Bold"),
            toggles("Pen"),
            toggles(""),
            toggles("Fill"),
            ("check box", "Wrap lines".to_string()),
            ("check box", "Snap".to_string()),
        ]
    );
    // Then it wraps round to the first again (greyed out ones are skipped on the way).
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Bold");
    h.key_with(Key::Tab, Modifiers(Modifiers::SHIFT));
    assert_eq!(h.focused().unwrap().what.label, "Snap");
}

#[test]
fn a_toggle_button_flips_the_apps_value_and_shows_it() {
    let mut h = demo();
    assert_eq!(h.find("Bold").unwrap().1.what.checked, Some(false));
    h.click_on("Bold");
    assert_eq!(h.take_messages(), vec![Msg::Bold(true)]);
    assert!(h.app().bold);
    assert_eq!(h.find("Bold").unwrap().1.what.checked, Some(true));
    h.click_on("Bold");
    assert_eq!(h.take_messages(), vec![Msg::Bold(false)]);
    assert!(!h.app().bold);
    assert_eq!(h.find("Bold").unwrap().1.what.checked, Some(false));
}

#[test]
fn space_toggles_the_focused_toggle_button_and_check_box_only() {
    let mut h = demo();
    // Nothing focused: Space does nothing.
    h.key(Key::Space);
    assert!(h.messages().is_empty());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Bold");
    h.key(Key::Enter);
    assert!(h.messages().is_empty(), "Enter is not for a toggle");
    h.key(Key::Space);
    assert_eq!(h.take_messages(), vec![Msg::Bold(true)]);
    for _ in 0..4 {
        h.key(Key::Tab);
    }
    assert_eq!(h.focused().unwrap().what.label, "Wrap lines");
    h.key(Key::Space);
    assert_eq!(h.take_messages(), vec![Msg::Wrap(false)]);
    assert!(!h.app().wrap);
    h.key(Key::Space);
    assert_eq!(h.take_messages(), vec![Msg::Wrap(true)]);
}

#[test]
fn a_check_box_toggles_from_its_box_and_from_its_label() {
    let mut h = demo();
    let r = h.find("Snap").unwrap().1.rect;
    // The box.
    h.click(r.x + 7, r.y + 12);
    // The label, well away from the box.
    h.click(r.x + r.width - 3, r.y + 12);
    assert_eq!(h.take_messages(), vec![Msg::Snap(true), Msg::Snap(false)]);
    h.click_on("Snap");
    assert!(h.app().snap);
    assert_eq!(h.find("Snap").unwrap().1.what.checked, Some(true));
    // Let go outside: nothing.
    let n = h.messages().len();
    h.move_to(r.x + 7, r.y + 12);
    h.press(1);
    h.move_to(r.x + 7, r.y + 100);
    h.release(1);
    assert_eq!(h.messages().len(), n);
}

#[test]
fn greyed_out_toggles_and_check_boxes_do_nothing() {
    let mut h = demo();
    h.click_on("Grey");
    h.click_on("Off");
    h.click_on("On");
    assert!(h.messages().is_empty());
    let d = h.find("Off").unwrap().1;
    assert!(!d.what.enabled);
    assert_eq!(d.what.checked, Some(false));
    assert_eq!(h.find("On").unwrap().1.what.checked, Some(true));
    assert!(!h.find("Grey").unwrap().1.what.enabled);
}

#[test]
fn a_toggle_pressed_then_left_does_not_toggle() {
    let mut h = demo();
    let r = h.find("Bold").unwrap().1.rect;
    h.move_to(r.x + 5, r.y + 5);
    h.press(1);
    h.move_to(r.x + 5, r.y + 90);
    h.release(1);
    assert!(h.messages().is_empty());
}

#[test]
fn descriptions() {
    let h = demo();
    let lines = h.describe_lines(WindowId::MAIN);
    let has = |s: &str| assert!(lines.iter().any(|l| l.starts_with(s)), "{s}\n{lines:#?}");
    has("icon \"Logo\" at");
    has("button \"\" at 63,11 size 26x26 id open");
    has("button \"Open\"");
    has("button \"Save\"");
    has("toggle button \"Bold\"");
    has("toggle button \"Pen\"");
    has("check box \"Wrap lines\"");
    let all = lines.join("\n");
    assert!(all.contains("toggle button \"Pen\" at"), "{all}");
    assert!(all.contains("check box \"Snap\"") && all.contains("unchecked"), "{all}");
    assert!(all.contains("check box \"On\"") && all.contains("disabled checked"), "{all}");
    assert!(all.contains("button \"Save\" at") && all.contains("disabled"), "{all}");
}

#[test]
fn nothing_panics_at_odd_sizes() {
    struct Odd(Vec<u32>);
    impl App for Odd {
        type Message = ();
        fn view(&self) -> Element<'_, ()> {
            let icon = Image::new(&self.0, 16, 16).unwrap();
            Row::new()
                .width(Length::Fixed(10))
                .push(IconButton::new(icon).label("A very long label indeed").on_press(()))
                .push(ToggleButton::new(true).image(icon).label("Long toggle").on_toggle(|_| ()))
                .push(CheckBox::new("", true).on_toggle(|_| ()).width(Length::Fixed(0)))
                .push(CheckBox::new("Long long long label", true).width(Length::Fixed(20)))
                .push(Icon::new(icon).size(-4, 0))
                .push(Icon::new(icon).size(300, 3))
                .into()
        }
        fn update(&mut self, _m: (), _cx: &mut Context<'_, ()>) {}
    }
    for size in [(1, 1), (40, 20), (200, 40)] {
        let mut h = Harness::new(
            Odd(icon_pixels()),
            Settings { size, resizable: true, min_size: (0, 0), over: None, system: false },
        );
        h.click(3, 3);
        h.key(Key::Tab);
        h.key(Key::Space);
        h.resize(30, 30);
        h.click(0, 0);
        h.resize(0, 0);
        h.resize(120, 50);
    }
}

#[test]
fn pictures_at_1x_and_2x() {
    let mut h = demo();
    h.check_picture("buttons-1x");

    // Hovering an icon button shows its face; holding it down, the pressed face.
    let (x, y) = centre(&h, "open-label");
    h.move_to(x, y);
    h.check_picture("buttons-hover-1x");
    h.press(1);
    h.check_picture("buttons-pressed-1x");
    h.release(1);
    h.move_to(0, 100);

    // The keyboard on a toggle button, then on a ticked check box.
    h.key(Key::Tab);
    h.check_picture("buttons-focus-toggle-1x");
    for _ in 0..4 {
        h.key(Key::Tab);
    }
    h.check_picture("buttons-focus-check-1x");
    h.key(Key::Space);
    h.check_picture("buttons-unticked-1x");

    let mut h = Harness::with_scale(Demo::new(), settings(), 2);
    h.check_picture("buttons-2x");
    let (x, y) = centre(&h, "open");
    h.move_to(x, y);
    h.press(1);
    h.check_picture("buttons-pressed-2x");
    h.release(1);
    h.move_to(0, 100);
    h.key(Key::Tab);
    h.check_picture("buttons-focus-toggle-2x");
    for _ in 0..4 {
        h.key(Key::Tab);
    }
    h.check_picture("buttons-focus-check-2x");
}
