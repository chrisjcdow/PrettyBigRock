// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `MenuBar`, `ContextMenu`, `Toolbar` and `StatusBar` through the harness: where a menu's
//! popup opens, choosing by click, by press-drag-release and by keyboard, toggling and switching
//! between menus, Escape and clicks outside, disabled rows, shortcuts with the menu shut, the
//! context menu by right click and Shift+F10, the widgets' descriptions and sizes, odd input
//! that must not panic, and their look at 1x and 2x (note 0023, sections 5 and 6).

use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Menu(&'static str),
    Wrap,
    Ctx(&'static str),
    Tool(&'static str),
}

struct Notes {
    can_save: bool,
    wrap: bool,
}

impl App for Notes {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let file = Menu::new(
            "File",
            vec![
                MenuItem::new("New", Msg::Menu("new")).shortcut(Shortcut::ctrl('n')),
                MenuItem::new("Open…", Msg::Menu("open")).shortcut(Shortcut::ctrl('o')),
                MenuItem::separator(),
                MenuItem::new("Save", Msg::Menu("save"))
                    .shortcut(Shortcut::ctrl('s'))
                    .enabled(self.can_save),
                MenuItem::new("Word wrap", Msg::Wrap).checked(self.wrap),
            ],
        );
        let edit = Menu::new(
            "Edit",
            vec![
                MenuItem::new("Undo", Msg::Menu("undo")).shortcut(Shortcut::ctrl('z')),
                MenuItem::separator(),
                MenuItem::new("Cut", Msg::Menu("cut")).shortcut(Shortcut::ctrl('x')),
                MenuItem::new("Copy", Msg::Menu("copy")).shortcut(Shortcut::ctrl('c')),
                MenuItem::new("Paste", Msg::Menu("paste")).shortcut(Shortcut::ctrl('v')),
            ],
        );
        let context = vec![
            MenuItem::new("Copy", Msg::Ctx("copy")),
            MenuItem::new("Paste", Msg::Ctx("paste")).enabled(false),
            MenuItem::separator(),
            MenuItem::new("Select all", Msg::Ctx("all")),
        ];
        Column::new()
            .height(Length::Fill)
            .push(MenuBar::new(vec![file, edit]))
            .push(
                Toolbar::new()
                    .push(Button::new("Tool A").on_press(Msg::Tool("a")))
                    .push(Button::new("Tool B").on_press(Msg::Tool("b")))
                    .separator()
                    .push(Button::new("Tool C").on_press(Msg::Tool("c")))
                    .push_right(Label::new("Ln 3, Col 7"))
                    .push(Button::new("Tool D").on_press(Msg::Tool("d"))),
            )
            .push(ContextMenu::new(
                Panel::boxed(Button::new("Target").on_press(Msg::Tool("target"))).padding(20),
                context,
            ))
            .push(Stretch::new())
            .push(StatusBar::new().push("Ready").push("UTF-8").push_right("Plain text"))
            .into()
    }

    fn update(&mut self, m: Msg, _cx: &mut Context<'_, Msg>) {
        if m == Msg::Wrap {
            self.wrap = !self.wrap;
        }
    }
}

fn settings() -> Settings {
    Settings { size: (400, 200), resizable: true, min_size: (0, 0), over: None, system: false }
}

fn app() -> Harness<Notes> {
    Harness::new(Notes { can_save: false, wrap: true }, settings())
}

/// The middle of the File title and of the Edit title ("File" is 24 + 16 wide, from 4).
const FILE: (i32, i32) = (24, 12);
const EDIT: (i32, i32) = (64, 12);

fn open_file(h: &mut Harness<Notes>) -> WindowId {
    h.click(FILE.0, FILE.1);
    h.popup().expect("a popup")
}

fn lit(h: &Harness<Notes>) -> Vec<String> {
    let p = h.popup().expect("a popup");
    h.describe(p).into_iter().filter(|d| d.focused).map(|d| d.what.label).collect()
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/sandstone")
}

#[test]
fn the_widgets_describe_themselves() {
    let h = app();
    let lines = h.describe_lines(WindowId::MAIN);
    assert!(
        lines.iter().any(|l| l.starts_with("menu bar \"File|Edit\" at 0,0 size 400x24")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("status bar \"Ready|UTF-8|Plain text\" at 0,180 size 400x20")),
        "{lines:?}"
    );
    let a = h.find("Tool A").unwrap().1.rect;
    // The toolbar is 32 high below the bar; its 24-high buttons are centred above its bottom
    // line, 6 pixels in.
    assert_eq!((a.x, a.y, a.height), (6, 24 + 3, 24));
    let b = h.find("Tool B").unwrap().1.rect;
    assert_eq!(b.x, a.x + a.width + 4);
    let c = h.find("Tool C").unwrap().1.rect;
    assert_eq!(c.x, b.x + b.width + 4 + 1 + 4);
    // The right end: the label ends 6 pixels from the edge; the button after it is on the left.
    let d = h.find("Tool D").unwrap().1.rect;
    let ln = h.find("Ln 3, Col 7").unwrap().1.rect;
    assert_eq!(d.x, c.x + c.width + 4);
    assert_eq!(ln.x + ln.width, 400 - 6);
}

#[test]
fn a_press_on_a_title_opens_its_menu_just_under_it() {
    let mut h = app();
    let p = open_file(&mut h);
    let w = h.window(p).unwrap();
    assert_eq!(w.kind, pbr_window::WindowKind::Popup);
    assert_eq!(w.at, (4, 24));
    // Four rows of 22, a separator of 9, and the edge.
    assert_eq!(w.size, (144, 2 + 4 * 22 + 9));
    let rows = h.describe_lines(p);
    assert_eq!(rows.len(), 4, "{rows:?}");
    assert!(rows[0].starts_with("menu item \"New\""), "{rows:?}");
    assert!(rows[0].contains("value \"Ctrl+N\""));
    assert!(rows[2].starts_with("menu item \"Save\"") && rows[2].contains("disabled"), "{rows:?}");
    assert!(rows[3].contains("\"Word wrap\"") && rows[3].ends_with("checked"), "{rows:?}");

    h.click(FILE.0, FILE.1); // closes
    assert!(h.popup().is_none());
    h.advance(500);
    h.click(EDIT.0, EDIT.1);
    assert_eq!(h.window(h.popup().unwrap()).unwrap().at, (44, 24));
}

#[test]
fn clicking_a_row_sends_its_message_and_closes_the_menu() {
    let mut h = app();
    open_file(&mut h);
    h.click_on("Open…");
    assert_eq!(h.messages(), &[Msg::Menu("open")]);
    assert!(h.popup().is_none());
    // A row that is ticked toggles through the app, and the check follows.
    open_file(&mut h);
    assert_eq!(h.find("Word wrap").unwrap().1.what.checked, Some(true));
    h.click_on("Word wrap");
    assert_eq!(h.messages().last(), Some(&Msg::Wrap));
    open_file(&mut h);
    assert_eq!(h.find("Word wrap").unwrap().1.what.checked, Some(false));
}

#[test]
fn press_on_the_title_drag_down_and_let_go_chooses_the_row() {
    let mut h = app();
    h.move_to(FILE.0, FILE.1);
    h.press(1);
    assert!(h.popup().is_some());
    let p = h.popup().unwrap();
    let r = h.find("New").unwrap().1.rect;
    h.move_in(p, r.x + 20, r.y + 10);
    assert_eq!(lit(&h), vec!["New"]);
    h.release(1);
    assert_eq!(h.messages(), &[Msg::Menu("new")]);
    assert!(h.popup().is_none());
}

#[test]
fn letting_go_over_the_separator_or_a_disabled_row_chooses_nothing() {
    let mut h = app();
    open_file(&mut h);
    let p = h.popup().unwrap();
    let save = h.find("Save").unwrap().1.rect;
    h.move_in(p, save.x + 5, save.y + 5);
    assert!(lit(&h).is_empty());
    h.press(1);
    h.release(1);
    // The separator is between Open… and Save.
    let open = h.find("Open…").unwrap().1.rect;
    h.move_in(p, open.x + 5, open.y + open.height + 4);
    h.press(1);
    h.release(1);
    assert!(h.messages().is_empty());
    assert!(h.popup().is_some());
}

#[test]
fn moving_over_another_title_switches_menus_only_soon_after_a_press() {
    let mut h = app();
    open_file(&mut h);
    h.move_to(EDIT.0, EDIT.1);
    let p = h.popup().expect("the Edit menu");
    assert_eq!(h.window(p).unwrap().at, (44, 24));
    assert!(h.find("Undo").is_some() && h.find("New").is_none());
    // Back again, and the bar remembers which is open: a click on its title closes it.
    h.move_to(FILE.0, FILE.1);
    assert!(h.find("New").is_some());
    h.click(FILE.0, FILE.1);
    assert!(h.popup().is_none());

    // Long after the press, a move can't open a popup.
    open_file(&mut h);
    h.advance(2000);
    h.move_to(EDIT.0, EDIT.1);
    assert!(h.find("New").is_some() && h.find("Undo").is_none());
}

#[test]
fn escape_and_a_click_outside_close_the_menu_without_choosing() {
    let mut h = app();
    open_file(&mut h);
    h.key(Key::Escape);
    assert!(h.popup().is_none());
    h.advance(500);
    open_file(&mut h);
    h.click(250, 100);
    assert!(h.popup().is_none());
    assert!(h.messages().is_empty());
    // After either, the bar is ready to open a menu again.
    h.advance(500);
    open_file(&mut h);
    assert!(h.find("New").is_some());
}

#[test]
fn f10_opens_the_first_menu_and_the_arrows_move_the_light() {
    let mut h = app();
    h.key(Key::Function(10));
    assert_eq!(h.window(h.popup().unwrap()).unwrap().at, (4, 24));
    assert_eq!(lit(&h), vec!["New"]);
    h.key(Key::Down);
    assert_eq!(lit(&h), vec!["Open…"]);
    // Past the separator and the disabled Save.
    h.key(Key::Down);
    assert_eq!(lit(&h), vec!["Word wrap"]);
    h.key(Key::Down);
    assert_eq!(lit(&h), vec!["New"]);
    h.key(Key::Up);
    assert_eq!(lit(&h), vec!["Word wrap"]);
    h.key(Key::Home);
    assert_eq!(lit(&h), vec!["New"]);
    h.key(Key::End);
    assert_eq!(lit(&h), vec!["Word wrap"]);
    h.key(Key::Up);
    h.key(Key::Enter);
    assert_eq!(h.messages(), &[Msg::Menu("open")]);
    assert!(h.popup().is_none());
}

#[test]
fn left_and_right_move_between_menus_and_wrap_round() {
    let mut h = app();
    h.key(Key::Function(10));
    h.key(Key::Right);
    assert_eq!(h.window(h.popup().unwrap()).unwrap().at, (44, 24));
    assert_eq!(lit(&h), vec!["Undo"]);
    h.key(Key::Right);
    assert_eq!(h.window(h.popup().unwrap()).unwrap().at, (4, 24));
    h.key(Key::Left);
    assert_eq!(lit(&h), vec!["Undo"]);
    // The bar still knows what is open: Escape then F10 again works.
    h.key(Key::Escape);
    assert!(h.popup().is_none());
    h.key(Key::Function(10));
    assert_eq!(lit(&h), vec!["New"]);
    h.key(Key::Function(10));
    assert!(h.popup().is_none());
    assert!(h.messages().is_empty());
}

#[test]
fn enter_with_nothing_lit_and_other_keys_choose_nothing() {
    let mut h = app();
    h.move_to(FILE.0, FILE.1);
    h.press(1);
    h.release(1);
    assert!(lit(&h).is_empty());
    h.key(Key::Enter);
    h.type_text("x");
    assert!(h.popup().is_some() && h.messages().is_empty());
    h.key(Key::Down);
    assert_eq!(lit(&h), vec!["New"]);
}

#[test]
fn shortcuts_work_with_the_menu_shut_and_disabled_rows_ignore_theirs() {
    let mut h = app();
    h.shortcut(Shortcut::ctrl('n'));
    h.shortcut(Shortcut::ctrl('v'));
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.messages(), &[Msg::Menu("new"), Msg::Menu("paste")]);
    h.app_mut().can_save = true;
    h.refresh();
    h.shortcut(Shortcut::ctrl('s'));
    assert_eq!(h.messages().last(), Some(&Msg::Menu("save")));
    assert_eq!(h.messages().len(), 3);
}

#[test]
fn a_right_click_opens_the_context_menu_at_the_pointer() {
    let mut h = app();
    let t = h.find("Target").unwrap().1.rect;
    h.right_click_in(WindowId::MAIN, t.x + 3, t.y + 4);
    let p = h.popup().expect("the context menu");
    assert_eq!(h.window(p).unwrap().at, (t.x + 3, t.y + 4));
    assert_eq!(h.describe(p).len(), 3);
    assert!(!h.find("Paste").unwrap().1.what.enabled);
    h.click_on("Select all");
    assert_eq!(h.messages(), &[Msg::Ctx("all")]);
    assert!(h.popup().is_none());

    // A right click outside the child opens nothing; a left click does nothing here either.
    h.right_click_in(WindowId::MAIN, 300, 60);
    assert!(h.popup().is_none());
    // The child still works as a button.
    h.click_on("Target");
    assert_eq!(h.messages().last(), Some(&Msg::Tool("target")));
}

#[test]
fn shift_f10_opens_the_context_menu_when_the_focus_is_inside() {
    let mut h = app();
    let shift = Modifiers(Modifiers::SHIFT);
    h.key_with(Key::Function(10), shift);
    assert!(h.popup().is_none(), "nothing focused");
    for _ in 0..5 {
        h.key(Key::Tab);
        if h.focused().is_some_and(|f| f.what.label == "Target") {
            break;
        }
    }
    assert_eq!(h.focused().unwrap().what.label, "Target");
    let t = h.find("Target").unwrap().1.rect;
    h.key_with(Key::Function(10), shift);
    let p = h.popup().expect("the context menu");
    assert_eq!(h.window(p).unwrap().at, (t.x - 20, t.y - 20));
    assert_eq!(lit(&h), vec!["Copy"]);
    h.key(Key::Down);
    assert_eq!(lit(&h), vec!["Select all"]);
    h.key(Key::Right);
    h.key(Key::Enter);
    assert_eq!(h.messages(), &[Msg::Ctx("all")]);
}

#[test]
fn a_menu_follows_the_apps_state_while_open() {
    let mut h = app();
    open_file(&mut h);
    assert!(!h.find("Save").unwrap().1.what.enabled);
    h.app_mut().can_save = true;
    h.refresh();
    assert!(h.find("Save").unwrap().1.what.enabled);
    h.click_on("Save");
    assert_eq!(h.messages(), &[Msg::Menu("save")]);
}

/// Widgets with nothing to show, huge texts, tiny windows and keys with no menu to open.
struct Odd {
    huge: bool,
}

impl App for Odd {
    type Message = u8;
    fn view(&self) -> Element<'_, u8> {
        let long = "m".repeat(if self.huge { 5000 } else { 3 });
        let bar = if self.huge {
            MenuBar::new(vec![
                Menu::new(&long, vec![MenuItem::new(&long, 1).shortcut(Shortcut::ctrl('q'))]),
                Menu::new("Empty", vec![]),
                Menu::new("", vec![MenuItem::separator(), MenuItem::separator()]),
            ])
        } else {
            MenuBar::new(vec![])
        };
        Column::new()
            .push(bar)
            .push(Toolbar::new())
            .push(ContextMenu::new(Label::new("x"), vec![]))
            .push(StatusBar::new().push(&long).push_right(&long))
            .into()
    }
    fn update(&mut self, _m: u8, _cx: &mut Context<'_, u8>) {}
}

#[test]
fn odd_input_does_not_panic() {
    for huge in [false, true] {
        for size in [(1, 1), (30, 20), (300, 100)] {
            let mut h = Harness::new(
                Odd { huge },
                Settings { size, resizable: true, min_size: (0, 0), over: None, system: false },
            );
            for (x, y) in [(0, 0), (5, 5), (20, 10), (100, 12), (-5, -5), (100000, 3)] {
                h.click(x, y);
                h.right_click_in(WindowId::MAIN, x, y);
                h.key(Key::Function(10));
                for k in
                    [Key::Down, Key::Up, Key::Left, Key::Right, Key::Home, Key::End, Key::Enter]
                {
                    h.key(k);
                }
                h.key_with(Key::Function(10), Modifiers(Modifiers::SHIFT));
                h.key(Key::Escape);
            }
            h.shortcut(Shortcut::ctrl('q'));
            h.resize(0, 0);
            h.resize(200, 50);
            h.key(Key::Tab);
        }
    }
}

#[test]
fn pictures_at_1x_and_2x() {
    let dir = dir();
    let mut h = app();
    h.check_picture("menu-window-1x");

    h.move_to(EDIT.0, EDIT.1);
    h.check_picture("menu-title-hover-1x");
    h.move_to(0, 150);

    let p = open_file(&mut h);
    h.check_picture("menu-title-open-1x");
    h.check_window_picture_in(p, &dir, "menu-file-1x");
    let r = h.find("Open…").unwrap().1.rect;
    h.move_in(p, r.x + 20, r.y + 10);
    h.check_window_picture_in(p, &dir, "menu-file-hover-1x");
    let r = h.find("Save").unwrap().1.rect;
    h.move_in(p, r.x + 20, r.y + 10);
    h.key(Key::Escape);

    h.move_to(0, 150);
    h.advance(500);
    h.key(Key::Function(10));
    h.key(Key::Right);
    let p = h.popup().unwrap();
    h.check_window_picture_in(p, &dir, "menu-edit-keys-1x");
    h.key(Key::Escape);

    let t = h.find("Target").unwrap().1.rect;
    h.right_click_in(WindowId::MAIN, t.x + 3, t.y + 4);
    let p = h.popup().unwrap();
    h.check_window_picture_in(p, &dir, "menu-context-1x");
    h.key(Key::Escape);

    let mut h = Harness::with_scale(Notes { can_save: true, wrap: false }, settings(), 2);
    h.check_picture("menu-window-2x");
    h.click(FILE.0, FILE.1);
    let p = h.popup().unwrap();
    h.check_picture("menu-title-open-2x");
    h.check_window_picture_in(p, &dir, "menu-file-2x");
    h.key(Key::Escape);
    h.advance(500);
    let t = h.find("Target").unwrap().1.rect;
    h.right_click_in(WindowId::MAIN, t.x + 3, t.y + 4);
    let p = h.popup().unwrap();
    h.check_window_picture_in(p, &dir, "menu-context-2x");
}
