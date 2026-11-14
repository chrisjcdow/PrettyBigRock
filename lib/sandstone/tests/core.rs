// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Sandstone's core, through the harness: messages, the focus and Tab, Enter and Escape,
//! shortcuts, state kept between views, drawing only what changed, dialogs, and the look of
//! labels, buttons and panels at 1x and 2x (note 0023, sections 5 and 6).

use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Add,
    Reset,
    Save,
    Ask,
    Answer(&'static str),
    Close,
}

#[derive(Default)]
struct Counter {
    n: i32,
    answers: Vec<&'static str>,
    extra: bool,
    confirm_close: bool,
}

impl App for Counter {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        let mut col = Column::new()
            .padding(8)
            .spacing(6)
            .push(Label::new("Counter").heading())
            .push(Label::new(&format!("Count: {}", self.n)).id("count"));
        if self.extra {
            col = col.push(Label::new("An extra line"));
        }
        col.push(
            Row::new()
                .spacing(6)
                .push(Button::new("Add").on_press(Msg::Add).default().id("add"))
                .push(Button::new("Reset").on_press_maybe((self.n != 0).then_some(Msg::Reset)))
                .push(Button::new("Ask").on_press(Msg::Ask))
                .push(Button::new("Close").on_press(Msg::Close).cancel()),
        )
        .push(Panel::boxed(Label::new("In a box")).padding(6))
        .into()
    }

    fn update(&mut self, m: Msg, cx: &mut Context<'_, Msg>) {
        match m {
            Msg::Add => self.n += 1,
            Msg::Reset => self.n = 0,
            Msg::Save => {}
            Msg::Ask => {
                cx.show(
                    Dialog::question("Question", "Keep the count?")
                        .heading("Keep it?")
                        .cancel_button("No", Msg::Answer("no"))
                        .default_button("Yes", Msg::Answer("yes")),
                );
            }
            Msg::Answer(a) => self.answers.push(a),
            Msg::Close => cx.quit(),
        }
    }

    fn title(&self) -> String {
        format!("{} counted", self.n)
    }

    fn shortcuts(&self) -> Vec<(Shortcut, Msg)> {
        vec![(Shortcut::ctrl('s'), Msg::Save)]
    }

    fn close_requested(&self, _w: WindowId) -> Option<Msg> {
        self.confirm_close.then_some(Msg::Ask)
    }
}

fn settings() -> Settings {
    Settings { size: (320, 140), resizable: false, min_size: (0, 0), over: None }
}

#[test]
fn a_click_sends_the_message_and_redraws_only_the_label() {
    let mut h = Harness::new(Counter::default(), settings());
    let before = h.window(WindowId::MAIN).unwrap().draws;
    h.click_on("Add");
    assert_eq!(h.messages(), &[Msg::Add]);
    assert_eq!(h.app().n, 1);
    let w = h.window(WindowId::MAIN).unwrap();
    assert!(w.draws > before);
    // The count's label changed, and Reset became usable: not the whole window.
    let area: i32 = w.damage.iter().map(|r| r.width * r.height).sum();
    assert!(area < 320 * 140 / 2, "drew {area} pixels: {:?}", w.damage);
    assert_eq!(w.title, "1 counted");
}

#[test]
fn a_greyed_out_button_does_nothing() {
    let mut h = Harness::new(Counter::default(), settings());
    h.click_on("Reset");
    assert!(h.messages().is_empty());
    let reset = h.find("Reset").unwrap().1;
    assert!(!reset.what.enabled);
}

#[test]
fn pressing_then_leaving_a_button_does_not_click_it() {
    let mut h = Harness::new(Counter::default(), settings());
    let r = h.find("Add").unwrap().1.rect;
    h.move_to(r.x + 5, r.y + 5);
    h.press(1);
    h.move_to(r.x + r.width + 40, r.y + 5);
    h.release(1);
    assert!(h.messages().is_empty());
}

#[test]
fn tab_moves_the_focus_and_space_presses() {
    let mut h = Harness::new(Counter::default(), settings());
    assert!(h.focused().is_none());
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Add");
    h.key(Key::Tab);
    // Reset is greyed out, so it is skipped.
    assert_eq!(h.focused().unwrap().what.label, "Ask");
    h.key_with(Key::Tab, Modifiers(Modifiers::SHIFT));
    assert_eq!(h.focused().unwrap().what.label, "Add");
    h.key(Key::Space);
    assert_eq!(h.messages(), &[Msg::Add]);
    // Now Reset can be used, and is in the order.
    h.key(Key::Tab);
    assert_eq!(h.focused().unwrap().what.label, "Reset");
}

#[test]
fn enter_and_escape_press_the_default_and_cancel_buttons() {
    let mut h = Harness::new(Counter::default(), settings());
    h.key(Key::Enter);
    h.key(Key::Enter);
    assert_eq!(h.app().n, 2);
    h.key(Key::Escape);
    assert!(h.finished());
}

#[test]
fn shortcuts_match_the_character() {
    let mut h = Harness::new(Counter::default(), settings());
    h.shortcut(Shortcut::ctrl('s'));
    h.shortcut(Shortcut::ctrl('S'));
    h.type_text("s");
    assert_eq!(h.messages(), &[Msg::Save, Msg::Save]);
}

#[test]
fn the_focus_follows_a_widget_with_an_id_when_others_come_before_it() {
    let mut h = Harness::new(Counter::default(), settings());
    h.key(Key::Tab);
    let add = h.focused().unwrap();
    assert_eq!(add.key.as_deref(), Some("add"));
    h.app_mut().extra = true;
    h.refresh();
    let now = h.focused().unwrap();
    assert_eq!(now.id, add.id);
    assert!(now.rect.y > add.rect.y);
}

#[test]
fn a_question_dialog_answers_with_the_button_chosen() {
    let mut h = Harness::new(Counter::default(), settings());
    h.click_on("Ask");
    let d = h.dialog().expect("a dialog");
    assert_eq!(h.window(d).unwrap().title, "Question");
    assert!(h.window(d).unwrap().flags.contains(pbr_window::WindowFlags::MODAL));
    h.click_on("Yes");
    assert!(h.dialog().is_none());
    assert_eq!(h.app().answers, vec!["yes"]);

    // The default button has the focus when it opens.
    h.click_on("Ask");
    assert_eq!(h.focused().unwrap().what.label, "Yes");
    // Enter and Escape in a dialog.
    h.key(Key::Enter);
    h.click_on("Ask");
    h.key(Key::Escape);
    // Its close button is its cancel button.
    h.click_on("Ask");
    h.close(h.dialog().unwrap());
    assert_eq!(h.app().answers, vec!["yes", "yes", "no", "no"]);
    assert!(h.dialog().is_none());
}

#[test]
fn closing_the_window_can_be_asked_about() {
    let mut h = Harness::new(Counter { confirm_close: true, ..Counter::default() }, settings());
    h.close(WindowId::MAIN);
    assert!(!h.finished());
    assert!(h.dialog().is_some());
    let mut h = Harness::new(Counter::default(), settings());
    h.close(WindowId::MAIN);
    assert!(h.finished());
}

#[test]
fn hovering_redraws_only_the_button() {
    let mut h = Harness::new(Counter::default(), settings());
    let r = h.find("Ask").unwrap().1.rect;
    h.move_to(r.x + 3, r.y + 3);
    let w = h.window(WindowId::MAIN).unwrap();
    assert_eq!(w.damage, vec![r]);
}

#[test]
fn the_look_at_1x_and_2x() {
    let mut h = Harness::new(Counter { n: 3, ..Counter::default() }, settings());
    h.check_picture("core-1x");
    h.key(Key::Tab);
    h.check_picture("core-focused-1x");
    let h2 = Harness::with_scale(Counter { n: 3, ..Counter::default() }, settings(), 2);
    h2.check_picture("core-2x");
}

#[test]
fn a_dialogs_look() {
    let mut h = Harness::new(Counter::default(), settings());
    h.click_on("Ask");
    let d = h.dialog().unwrap();
    let dir =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/sandstone");
    h.check_window_picture_in(d, &dir, "dialog-question-1x");
}

#[test]
fn rows_and_columns_share_room() {
    struct Lay;
    impl App for Lay {
        type Message = ();
        fn view(&self) -> Element<'_, ()> {
            Column::new()
                .push(
                    Row::new()
                        .width(Length::Fill)
                        .push(Button::new("A").on_press(()).width(Length::Fill))
                        .push(Button::new("B").on_press(()).width(Length::Portion(3)))
                        .push(Space::new(20, 4)),
                )
                .push(Stretch::new())
                .push(
                    Grid::new(3)
                        .spacing(2)
                        .width(Length::Fill)
                        .push(Button::new("1").on_press(()))
                        .push(Button::new("2").on_press(()).width(Length::Fill))
                        .push(Button::new("3").on_press(()))
                        .push(Button::new("4").on_press(())),
                )
                .into()
        }
        fn update(&mut self, _m: (), _cx: &mut Context<'_, ()>) {}
    }
    let h = Harness::new(Lay, Settings { size: (220, 120), ..Settings::default() });
    let a = h.find("A").unwrap().1.rect;
    let b = h.find("B").unwrap().1.rect;
    assert_eq!((a.x, a.width, b.x, b.width), (0, 50, 50, 150));
    let four = h.find("4").unwrap().1.rect;
    assert_eq!(four.y + four.height, 120);
    let two = h.find("2").unwrap().1.rect;
    assert_eq!((two.x, two.width), (74, 72));
}
