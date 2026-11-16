// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! `Button::colour` through the harness: it sends its message like any button, greys out
//! without one, and looks right at 1x and 2x, at rest, focused, under the pointer and pressed
//! (note 0023, sections 5 and 6).

use pbr_draw::Colour;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::*;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Msg {
    Orange,
    Pink,
}

struct Keys;

impl App for Keys {
    type Message = Msg;

    fn view(&self) -> Element<'_, Msg> {
        Panel::new(
            Row::new()
                .padding(8)
                .spacing(8)
                .push(Button::new("+").colour(Colour::solid(0xe0913a)).on_press(Msg::Orange))
                .push(
                    Button::new("=")
                        .colour(Colour::solid(0xd2577a))
                        .on_press(Msg::Pink)
                        .width(Length::Fixed(50))
                        .height(Length::Fixed(42)),
                )
                .push(Button::new("C").colour(Colour::solid(0x9aa0ac)))
                .push(Button::new("7").width(Length::Fixed(50))),
        )
        .into()
    }

    fn update(&mut self, _message: Msg, _cx: &mut Context<'_, Msg>) {}
}

fn settings() -> Settings {
    Settings { size: (290, 60), resizable: false, ..Settings::default() }
}

#[test]
fn a_coloured_button_sends_its_message_and_a_greyed_one_does_not() {
    let mut h = Harness::new(Keys, settings());
    h.click_on("+");
    h.click_on("=");
    h.click_on("C");
    assert_eq!(h.take_messages(), [Msg::Orange, Msg::Pink]);
    assert!(!h.find("C").unwrap().1.what.enabled);
}

#[test]
fn the_look_at_1x_and_2x() {
    let mut h = Harness::new(Keys, settings());
    h.check_picture("coloured-buttons-1x");
    h.key(Key::Tab);
    h.check_picture("coloured-buttons-focused-1x");
    // Under the pointer, and pressed.
    let r = h.find("+").unwrap().1.rect;
    h.move_to(r.x + 3, r.y + 3);
    h.check_picture("coloured-buttons-hover-1x");
    h.press(1);
    h.check_picture("coloured-buttons-pressed-1x");
    let h2 = Harness::with_scale(Keys, settings(), 2);
    h2.check_picture("coloured-buttons-2x");
}
