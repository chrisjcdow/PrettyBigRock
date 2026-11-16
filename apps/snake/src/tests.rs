// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Snake on the host (note 0026, section 1): a seeded game played through the window with key
//! presses and the passing of time; the pause keys and losing the keyboard; the best score
//! kept in a pretend folder; and its look (`tests/pictures/snake/`).

use super::*;
use pbr_sandstone::testing::Harness;
use pbr_sandstone::{Modifiers, Shortcut};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

/// A pretend folder: the text of `best.txt` (if there is one), every write made to it, and
/// whether writing fails. `Rc<RefCell<…>>` lets the test keep a handle on what the app
/// changes (`Rc` shares, `RefCell` lets the shared thing be changed).
#[derive(Default)]
struct Folder {
    text: Option<String>,
    writes: Vec<String>,
    full: bool,
}

#[derive(Clone, Default)]
struct Fake(Rc<RefCell<Folder>>);

impl Fake {
    fn with(text: &str) -> Fake {
        let f = Fake::default();
        f.0.borrow_mut().text = Some(text.to_string());
        f
    }
}

impl Store for Fake {
    fn read(&mut self) -> Option<String> {
        self.0.borrow().text.clone()
    }

    fn write(&mut self, text: &str) -> Result<(), String> {
        let mut f = self.0.borrow_mut();
        if f.full {
            return Err("the disk is full".to_string());
        }
        f.writes.push(text.to_string());
        f.text = Some(text.to_string());
        Ok(())
    }
}

const SEED: u64 = 42;

fn open(store: &Fake) -> Harness<Snake<Fake>> {
    Harness::new(Snake::new(store.clone(), SEED), settings())
}

fn game(h: &Harness<Snake<Fake>>) -> &Game {
    h.app().game()
}

/// The time to the next step, as the game says.
fn delay(h: &Harness<Snake<Fake>>) -> u64 {
    u64::from(game(h).delay_ms())
}

/// The key that steers `heading`.
fn arrow(heading: Heading) -> Key {
    match heading {
        Heading::Right => Key::Right,
        Heading::Down => Key::Down,
        Heading::Left => Key::Left,
        Heading::Up => Key::Up,
    }
}

/// Where a player would steer to get on towards the food without crashing: of the headings
/// that aren't straight back, the safe ones, the one that ends up nearest the food.
fn steer(g: &Game) -> Heading {
    let food = g.food().unwrap();
    Heading::ALL
        .into_iter()
        .filter(|h| *h != g.heading().reverse())
        .filter(|&h| {
            let to = g.head().go(h);
            let blocked = g.len() - usize::from(Some(to) != g.food());
            to.on_board() && !g.cells().take(blocked).any(|p| p == to)
        })
        .min_by_key(|&h| {
            let to = g.head().go(h);
            (to.x - food.x).abs() + (to.y - food.y).abs()
        })
        .unwrap_or(g.heading())
}

/// Plays on, steering with the arrow keys, until the score is `score`.
fn play_to(h: &mut Harness<Snake<Fake>>, score: u32) {
    for _ in 0..2_000 {
        if game(h).score() >= score {
            return;
        }
        let heading = steer(game(h));
        h.key(arrow(heading));
        let wait = delay(h);
        h.advance(wait);
        assert_eq!(game(h).state(), State::Playing, "{}", h.app().status_line());
    }
    panic!("never got to {score}: {}", h.app().status_line());
}

fn status(h: &Harness<Snake<Fake>>) -> String {
    h.app().status_line()
}

/// The bar of scores, as the pixels the window shows.
fn bar(h: &Harness<Snake<Fake>>) -> Vec<u32> {
    let p = h.picture(WindowId::MAIN);
    p.pixels[..(p.width * BAR as u32) as usize].to_vec()
}

#[test]
fn it_opens_ready_with_the_snake_where_1_3_put_it() {
    let h = open(&Fake::default());
    assert!(status(&h).starts_with("ready score 0 best 0 head 8,10 food "), "{}", status(&h));
    assert!(status(&h).ends_with(" heading right length 4"));
    // The board has the keyboard from the start and says what it is.
    let board = h.focused().expect("the board has the focus");
    assert_eq!(board.what.role, "canvas");
    assert_eq!(board.what.label, "Snake board");
    assert_eq!((board.rect.width, board.rect.height), (420, 330));
    // The window is the board's size and can't be resized.
    let w = h.window(WindowId::MAIN).unwrap();
    assert_eq!(w.size, (420, 330));
    assert!(!settings().resizable);
}

#[test]
fn a_game_played_with_the_keys_starts_turns_eats_and_ends() {
    let mut h = open(&Fake::default());
    // Nothing happens before Space, however long the wait.
    h.advance(5_000);
    assert_eq!(game(&h).head(), Point::new(8, 10));
    h.key(Key::Space);
    assert_eq!(game(&h).state(), State::Playing);
    // The first step comes after the game's wait, not before.
    h.advance(delay(&h) - 1);
    assert_eq!(game(&h).head(), Point::new(8, 10));
    h.advance(1);
    assert_eq!(game(&h).head(), Point::new(9, 10));

    // Turning: a key between steps takes effect at the next one. (The seed's food is at
    // (8, 11), which these turns keep clear of.)
    h.key(Key::Up);
    h.advance(delay(&h));
    assert_eq!((game(&h).head(), game(&h).heading()), (Point::new(9, 9), Heading::Up));
    // WASD too, capitals included.
    h.type_text("a");
    h.advance(delay(&h));
    assert_eq!((game(&h).head(), game(&h).heading()), (Point::new(8, 9), Heading::Left));
    h.type_text("S");
    h.advance(delay(&h));
    assert_eq!((game(&h).head(), game(&h).heading()), (Point::new(8, 10), Heading::Down));
    assert_eq!(game(&h).state(), State::Playing);

    // Eating shows in the bar, and the snake grows.
    let before = bar(&h);
    play_to(&mut h, 10);
    assert_eq!((game(&h).score(), game(&h).len()), (10, 5));
    assert!(status(&h).contains("score 10 best 10"));
    assert_ne!(bar(&h), before, "the bar should show the new score");
    play_to(&mut h, 30);
    assert_eq!(game(&h).len(), 7);

    // Steer into the wall: the game ends and stops its steps.
    for _ in 0..40 {
        if game(&h).state() == State::Over {
            break;
        }
        let toward = if game(&h).heading() == Heading::Up { Heading::Left } else { Heading::Up };
        h.key(arrow(toward));
        h.advance(delay(&h));
    }
    for _ in 0..60 {
        if game(&h).state() == State::Over {
            break;
        }
        h.advance(delay(&h));
    }
    assert_eq!(game(&h).state(), State::Over);
    assert!(status(&h).starts_with("over score 30 best 30 "), "{}", status(&h));
    let head = game(&h).head();
    h.advance(5_000);
    assert_eq!(game(&h).head(), head);
    // Space plays again, with the best score kept.
    h.key(Key::Space);
    assert!(status(&h).starts_with("playing score 0 best 30 head 8,10 "), "{}", status(&h));
}

#[test]
fn going_straight_back_is_ignored_and_the_last_of_two_turns_wins() {
    let mut h = open(&Fake::default());
    h.key(Key::Space);
    h.key(Key::Left);
    h.advance(delay(&h));
    assert_eq!((game(&h).head(), game(&h).heading()), (Point::new(9, 10), Heading::Right));
    // Up and then Down between two steps (Down is not back from Right): Down wins.
    h.key(Key::Up);
    h.key(Key::Down);
    h.advance(delay(&h));
    assert_eq!((game(&h).head(), game(&h).heading()), (Point::new(9, 11), Heading::Down));
    // Left then Up: Up is straight back from Down, so it is ignored, and so is the Left it
    // replaced: the snake carries on Down.
    h.key(Key::Left);
    h.key(Key::Up);
    h.advance(delay(&h));
    assert_eq!(game(&h).heading(), Heading::Down);
}

#[test]
fn p_and_space_pause_and_carry_on() {
    let mut h = open(&Fake::default());
    h.type_text("p");
    // Nothing to pause before a game.
    assert!(status(&h).starts_with("ready"));
    h.key(Key::Enter);
    assert!(status(&h).starts_with("playing"));
    h.type_text("p");
    assert!(status(&h).starts_with("paused"), "{}", status(&h));
    h.advance(5_000);
    assert_eq!(game(&h).head(), Point::new(8, 10));
    h.type_text("P");
    assert!(status(&h).starts_with("playing"));
    h.advance(delay(&h));
    assert_eq!(game(&h).head(), Point::new(9, 10));
    // Space pauses and carries on too, while playing.
    h.key(Key::Space);
    assert!(status(&h).starts_with("paused"));
    h.key(Key::Space);
    assert!(status(&h).starts_with("playing"));
    // Pausing and carrying on at once doesn't leave two timers going: a step takes a whole
    // wait, not half of one.
    let head = game(&h).head();
    h.advance(delay(&h) - 1);
    assert_eq!(game(&h).head(), head);
    h.advance(1);
    assert_ne!(game(&h).head(), head);
}

#[test]
fn losing_the_keyboard_pauses_the_game_and_it_stays_paused() {
    let mut h = open(&Fake::default());
    h.key(Key::Space);
    h.advance(delay(&h));
    h.keyboard_away(true);
    assert!(status(&h).starts_with("paused"), "{}", status(&h));
    let head = game(&h).head();
    h.advance(10_000);
    assert_eq!(game(&h).head(), head);
    h.keyboard_away(false);
    assert!(status(&h).starts_with("paused"));
    h.advance(1_000);
    assert_eq!(game(&h).head(), head);
    h.key(Key::Space);
    h.advance(delay(&h));
    assert_ne!(game(&h).head(), head);
    // Losing it before a game starts does nothing.
    let mut h = open(&Fake::default());
    h.keyboard_away(true);
    assert!(status(&h).starts_with("ready"));
}

#[test]
fn the_wait_between_steps_shortens_as_the_score_grows() {
    let mut h = open(&Fake::default());
    h.key(Key::Space);
    assert_eq!(delay(&h), 110);
    play_to(&mut h, 40);
    assert_eq!(delay(&h), 100);
    // The step timer follows: the next step comes after the shorter wait.
    let head = game(&h).head();
    h.advance(delay(&h) - 1);
    assert_eq!(game(&h).head(), head);
}

#[test]
fn keys_the_game_does_not_use_are_left_alone() {
    let mut h = open(&Fake::default());
    h.key(Key::Space);
    h.shortcut(Shortcut::ctrl('p'));
    h.shortcut(Shortcut::ctrl('w'));
    assert!(status(&h).starts_with("playing"));
    // A key held down doesn't flick the pause on and off.
    let k = |key, ch, repeat| KeyPress {
        key,
        code: 0,
        ch,
        typed: false,
        modifiers: Modifiers::NONE,
        repeat,
    };
    assert_eq!(command(&k(Key::Space, None, false)), Some(Command::Go));
    assert_eq!(command(&k(Key::Space, None, true)), None);
    assert_eq!(command(&k(Key::Character, Some('p'), true)), None);
    assert_eq!(command(&k(Key::Up, None, true)), Some(Command::Turn(Heading::Up)));
    assert_eq!(command(&k(Key::Tab, None, false)), None);
    assert_eq!(command(&k(Key::Character, Some('x'), false)), None);
    assert_eq!(command(&k(Key::Character, None, false)), None);
}

#[test]
fn the_best_score_is_read_and_kept() {
    // Nothing kept yet: the first game's best is kept when it ends.
    let store = Fake::default();
    let mut h = open(&store);
    h.key(Key::Space);
    play_to(&mut h, 20);
    assert!(store.0.borrow().writes.is_empty(), "nothing is written while playing");
    while game(&h).state() == State::Playing {
        h.key(Key::Up);
        h.advance(delay(&h));
    }
    assert_eq!(store.0.borrow().writes, ["20\n"]);
    assert!(h.app_mut().take_log().iter().any(|l| l == "kept the best score: 20"));

    // The next run of the program starts with it.
    let h2 = open(&store);
    assert_eq!(game(&h2).best(), 20);
    assert!(status(&h2).contains("best 20"));

    // A game that doesn't beat it writes nothing.
    let mut h = open(&store);
    h.key(Key::Space);
    while game(&h).state() == State::Playing {
        h.key(Key::Up);
        h.advance(delay(&h));
    }
    assert_eq!(store.0.borrow().writes.len(), 1);
    assert_eq!(game(&h).best(), 20);
}

#[test]
fn a_best_score_that_cannot_be_kept_is_tried_again_and_a_damaged_file_ignored() {
    let store = Fake::default();
    store.0.borrow_mut().full = true;
    let mut h = open(&store);
    h.key(Key::Space);
    play_to(&mut h, 10);
    while game(&h).state() == State::Playing {
        h.key(Key::Up);
        h.advance(delay(&h));
    }
    let log = h.app_mut().take_log();
    assert!(log.iter().any(|l| l == "can't keep the best score: the disk is full"), "{log:?}");
    // Room again: the next game that ends tries once more, even without a better score.
    store.0.borrow_mut().full = false;
    h.key(Key::Space);
    while game(&h).state() == State::Playing {
        h.key(Key::Up);
        h.advance(delay(&h));
    }
    assert_eq!(store.0.borrow().writes, ["10\n"]);

    for damaged in ["", "lots", "-5", "99999999999", "1 2"] {
        let mut h = open(&Fake::with(damaged));
        assert_eq!(game(&h).best(), 0, "{damaged:?}");
        assert_eq!(h.app_mut().take_log().len(), 1, "{damaged:?}");
    }
    assert_eq!(parse_best("120\n"), Some(120));
    assert_eq!(parse_best(" 7 "), Some(7));
}

#[test]
fn the_same_seed_plays_the_same_game() {
    let first = |seed| {
        let h = Harness::new(Snake::new(Fake::default(), seed), settings());
        status(&h)
    };
    assert_eq!(first(SEED), first(SEED));
    assert_ne!(first(SEED), first(SEED + 1));
}

#[test]
fn the_window_is_drawn_again_only_when_the_game_changes() {
    let mut h = open(&Fake::default());
    let draws = h.window(WindowId::MAIN).unwrap().draws;
    // Nothing changed with time passing in a game that hasn't begun.
    h.advance(1_000);
    assert_eq!(h.window(WindowId::MAIN).unwrap().draws, draws);
    h.key(Key::Space);
    h.advance(delay(&h));
    assert!(h.window(WindowId::MAIN).unwrap().draws > draws);
}

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/pictures/snake")
}

#[test]
fn how_it_looks() {
    for (factor, name) in [(1, "snake-1x"), (2, "snake-2x")] {
        let h = Harness::with_scale(Snake::new(Fake::default(), SEED), settings(), factor);
        h.check_window_picture_in(WindowId::MAIN, &dir(), name);
    }
    // Part-way through a game: the snake grown, the score and the best in the bar.
    let mut h = open(&Fake::with("120\n"));
    h.key(Key::Space);
    play_to(&mut h, 30);
    h.check_window_picture_in(WindowId::MAIN, &dir(), "snake-playing-1x");
}

#[test]
fn how_the_other_messages_look() {
    let mut h = open(&Fake::default());
    h.key(Key::Space);
    h.type_text("p");
    h.check_window_picture_in(WindowId::MAIN, &dir(), "snake-paused-1x");
    h.type_text("p");
    while game(&h).state() == State::Playing {
        h.key(Key::Up);
        h.advance(delay(&h));
    }
    h.check_window_picture_in(WindowId::MAIN, &dir(), "snake-over-1x");
}
