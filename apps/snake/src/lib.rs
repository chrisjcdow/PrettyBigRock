// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Snake as a sandstone app (docs/design/0026-apps-m6-tests-and-stages.md, section 1): 1.3's
//! game, with the rules in `lib/snake` and everything here being the window around them.
//!
//! The whole window is one sandstone [`Canvas`], a rectangle the app draws on itself with
//! `lib/draw`. Sandstone can't see what the drawing function will draw, so the app gives the
//! canvas a *version* number and changes it whenever the picture may have changed; sandstone
//! draws again only then.
//!
//! The game moves on a timer: while a game is running the app asks sandstone for a message
//! after [`Game::delay_ms`] milliseconds, steps the game when it comes, and asks again.
//!
//! **The best score is kept** in the app's own folder (`/Home/Apps/snake/best.txt`, note 0024,
//! section 4), which the sandbox manager makes for an app whose manifest says `data`. The app
//! reaches it through the [`Store`] trait, so the host tests give it a pretend folder and the
//! program (`main.rs`) the real one.
//!
//! **Pausing.** Space or Enter starts a game and, while one is being played, pauses it (so
//! does P). A game also pauses when the window loses the keyboard, as in 1.3, and stays paused
//! until the player carries on: unlike 1.3 it does not start again by itself, since a snake
//! that moves the instant the window is clicked is hard to steer.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use pbr_draw::{Canvas as DrawCanvas, Colour, Rect, TextStyle};
use pbr_font::ABOVE;
// `Canvas` here is sandstone's widget; what it draws on is `pbr_draw::Canvas`, imported above
// as `DrawCanvas` so that the two names don't clash.
use pbr_sandstone::{
    App, Canvas, Context, Element, Identify, Key, KeyPress, Settings, TimerId, WindowId,
};
use pbr_snake::{Crash, Game, HEIGHT, Heading, Point, State, Step, WIDTH};

/// How big a cell of the board is, in logical pixels.
const CELL: i32 = 15;
/// How tall the bar of scores above the board is.
const BAR: i32 = 30;
/// The window's content: 28 cells of 15 pixels across, and the bar and 20 cells down.
const SIZE: (i32, i32) = (WIDTH * CELL, BAR + HEIGHT * CELL);

/// The file the best score is kept in, in the app's own folder.
pub const BEST_FILE: &str = "best.txt";

/// The window's size and kind: 1.3's, which can't be resized.
pub fn settings() -> Settings {
    Settings {
        size: (SIZE.0 as u32, SIZE.1 as u32),
        resizable: false,
        min_size: (0, 0),
        over: None,
        system: false,
    }
}

/// Where the best score is kept. A *trait* is Rust's word for a set of methods a type
/// promises to have; the app asks for "any type with these", so a test can give it one that
/// keeps the text in memory.
pub trait Store {
    /// The text kept, or `None` if there is none (the first time) or it can't be read.
    fn read(&mut self) -> Option<String>;
    /// Keeps `text`, replacing what was there. The error is in words for the log.
    fn write(&mut self, text: &str) -> Result<(), String>;
}

/// The best score in the text of the file: a whole number, perhaps with a line end after it.
/// Anything else (a damaged file) is `None`.
pub fn parse_best(text: &str) -> Option<u32> {
    text.trim().parse().ok()
}

/// What the player can ask for with a key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Steer.
    Turn(Heading),
    /// Space or Enter: start a game, or pause the one being played.
    Go,
    /// P: pause, or carry on.
    Pause,
}

/// What a key means: the arrows and W, A, S, D steer; Space and Enter start and pause; P
/// pauses. A key held down repeats, which is fine for steering but would flick Space and P
/// on and off, so those ignore repeats. Keys with Ctrl, Alt or the Windows key held are the
/// window's, not the game's.
pub fn command(k: &KeyPress) -> Option<Command> {
    let m = k.modifiers.for_shortcuts();
    if m.ctrl() || m.alt() || m.meta() {
        return None;
    }
    let command = match k.key {
        Key::Right => Command::Turn(Heading::Right),
        Key::Down => Command::Turn(Heading::Down),
        Key::Left => Command::Turn(Heading::Left),
        Key::Up => Command::Turn(Heading::Up),
        Key::Space | Key::Enter => Command::Go,
        // `?` returns `None` from the whole function right away if there is no character.
        _ => match k.ch?.to_ascii_lowercase() {
            'd' => Command::Turn(Heading::Right),
            's' => Command::Turn(Heading::Down),
            'a' => Command::Turn(Heading::Left),
            'w' => Command::Turn(Heading::Up),
            'p' => Command::Pause,
            _ => return None,
        },
    };
    let steering = matches!(command, Command::Turn(_));
    (steering || !k.repeat).then_some(command)
}

/// What the window sends the app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Message {
    /// A key that means something to the game.
    Do(Command),
    /// The step timer went off.
    Tick,
    /// The window lost the keyboard.
    KeyboardLost,
}

/// Snake, the app: a game, and where its best score is kept. `Snake<S>` is a *generic* type:
/// one definition that works for any `S` that is a [`Store`] (the real folder, or a test's).
pub struct Snake<S: Store> {
    game: Game,
    store: S,
    /// The best score as last kept (or read), to know when a game has beaten it.
    kept: u32,
    /// Changes whenever the picture may have: the canvas is drawn again then.
    version: u64,
    /// The step timer, while one is set.
    timer: Option<TimerId>,
    /// Lines for the log ("kept the best score: 120").
    log: Vec<String>,
}

impl<S: Store> Snake<S> {
    /// A new game, its food placed from `seed` (the program gives a reading of the clock, a
    /// test a fixed number), with the best score read from `store`.
    pub fn new(mut store: S, seed: u64) -> Snake<S> {
        let mut game = Game::new(seed);
        let mut kept = 0;
        let mut log = Vec::new();
        // `if let` runs its block only when the value matches the pattern: here, when the
        // store had something to read.
        if let Some(text) = store.read() {
            match parse_best(&text) {
                Some(best) => {
                    kept = best;
                    game.set_best(best);
                }
                None => log.push(String::from("the kept best score is damaged; ignoring it")),
            }
        }
        Snake { game, store, kept, version: 1, timer: None, log }
    }

    /// The game, for drawing and for tests.
    pub fn game(&self) -> &Game {
        &self.game
    }

    /// The store.
    pub fn store(&self) -> &S {
        &self.store
    }

    /// The lines for the log made since the last call.
    pub fn take_log(&mut self) -> Vec<String> {
        core::mem::take(&mut self.log)
    }

    /// Where the game is, in one line for the log, which the desktop boot's runner steers by:
    /// `ready score 0 best 0 head 8,10 food 3,4 heading right length 4`. The state is
    /// `ready`, `playing`, `paused` or `over`.
    pub fn status_line(&self) -> String {
        let g = &self.game;
        let state = match (g.state(), g.paused()) {
            (State::Ready, _) => "ready",
            (State::Playing, false) => "playing",
            (State::Playing, true) => "paused",
            (State::Over, _) => "over",
        };
        let food = match g.food() {
            Some(f) => format!("{},{}", f.x, f.y),
            None => String::from("none"),
        };
        format!(
            "{state} score {} best {} head {},{} food {food} heading {} length {}",
            g.score(),
            g.best(),
            g.head().x,
            g.head().y,
            g.heading().name(),
            g.len()
        )
    }

    /// Acts on a key's meaning.
    fn command(&mut self, command: Command) {
        match command {
            Command::Turn(heading) => self.game.turn(heading),
            Command::Go if self.game.state() == State::Playing => self.game.toggle_pause(),
            Command::Go => self.game.start(),
            Command::Pause => self.game.toggle_pause(),
        }
    }

    /// Takes one step, and keeps the best score if the game ended on a better one.
    fn step(&mut self) {
        match self.game.step() {
            Step::Died(why) => {
                let how = match why {
                    Crash::Wall => "hit the wall",
                    Crash::Itself => "ran into itself",
                };
                self.log.push(format!("game over, {how}, score {}", self.game.score()));
                self.keep_best();
            }
            Step::Won => {
                self.log.push(format!("won, the board is full, score {}", self.game.score()));
                self.keep_best();
            }
            Step::Idle | Step::Moved | Step::Ate => {}
        }
    }

    /// Writes the best score to the store if it is better than what is kept there. If that
    /// fails the old value stays "kept", so the next game that ends tries again.
    fn keep_best(&mut self) {
        let best = self.game.best();
        if best <= self.kept {
            return;
        }
        match self.store.write(&format!("{best}\n")) {
            Ok(()) => {
                self.kept = best;
                self.log.push(format!("kept the best score: {best}"));
            }
            Err(why) => self.log.push(format!("can't keep the best score: {why}")),
        }
    }

    /// Makes the timer match the game: one set while the game is running, none otherwise.
    fn sync_timer(&mut self, cx: &mut Context<'_, Message>) {
        match (self.game.running(), self.timer) {
            (true, None) => {
                self.timer = Some(cx.after(u64::from(self.game.delay_ms()), Message::Tick));
            }
            (false, Some(timer)) => {
                cx.cancel(timer);
                self.timer = None;
            }
            _ => {}
        }
    }
}

impl<S: Store> App for Snake<S> {
    type Message = Message;

    fn view(&self) -> Element<'_, Message> {
        let game = &self.game;
        Canvas::new(SIZE.0, SIZE.1, move |canvas, area| draw_board(canvas, area, game))
            .version(self.version)
            .label("Snake board")
            // Every key the game has no use for (Tab, Escape) is left unused, so the window
            // still treats it as it would anywhere else.
            .on_key(|k, down| if down { command(&k).map(Message::Do) } else { None })
            .id("board")
    }

    fn update(&mut self, message: Message, cx: &mut Context<'_, Message>) {
        match message {
            Message::Do(command) => self.command(command),
            Message::Tick => {
                // The timer that sent this has gone; `sync_timer` sets the next.
                self.timer = None;
                self.step();
            }
            Message::KeyboardLost => self.game.pause(),
        }
        self.version += 1;
        self.sync_timer(cx);
    }

    fn start(&mut self, cx: &mut Context<'_, Message>) {
        // The board takes the keys from the start, without a click.
        cx.focus("board");
    }

    fn focus_changed(&self, window: WindowId, focused: bool) -> Option<Message> {
        (window == WindowId::MAIN && !focused).then_some(Message::KeyboardLost)
    }
}

// ---------------------------------------------------------------------------------------------
// Drawing: 1.3's `snake_draw`.
// ---------------------------------------------------------------------------------------------

/// The bar's colour, and the colours of the scores on it.
const BAR_COLOUR: Colour = Colour::solid(0x1d2a22);
const SCORE_COLOUR: Colour = Colour::WHITE;
const BEST_COLOUR: Colour = Colour::solid(0xf5d04a);
/// The two greens of the checked board.
const BOARD_LIGHT: Colour = Colour::solid(0x304434);
const BOARD_DARK: Colour = Colour::solid(0x2b3d2f);
const FOOD: Colour = Colour::solid(0xe05252);
const STALK: Colour = Colour::solid(0x5cc46a);
const HEAD: Colour = Colour::solid(0x9be89f);
/// The body shades from `NECK` just behind the head to `TAIL` at its end.
const NECK: Colour = Colour::solid(0x5cc46a);
const TAIL: Colour = Colour::solid(0x2f7d43);
const EYE: Colour = Colour::solid(0x10140f);
const SUB_COLOUR: Colour = Colour::solid(0xd8dce6);

/// Draws the window on `canvas`, with its top left corner at `area`'s. The canvas counts in
/// logical pixels and scales them to the screen itself, so the same code draws 1x and 2x.
pub fn draw_board(canvas: &mut DrawCanvas<'_>, area: Rect, game: &Game) {
    let bold = TextStyle::BOLD;
    // 1.3 put the top of a capital 11 pixels below the top of the bar; our font has `ABOVE`
    // rows of room over its capitals.
    let text_y = area.y + 11 - ABOVE as i32;
    canvas.fill_rect(Rect::new(area.x, area.y, SIZE.0, BAR), BAR_COLOUR);
    canvas.text(area.x + 10, text_y, &format!("Score {}", game.score()), SCORE_COLOUR, bold);
    let best = format!("Best {}", game.best());
    canvas.text(area.x + SIZE.0 - 10 - bold.width(&best), text_y, &best, BEST_COLOUR, bold);

    let top = area.y + BAR;
    let cell = |p: Point| Rect::new(area.x + p.x * CELL, top + p.y * CELL, CELL, CELL);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let colour = if (x + y) & 1 == 1 { BOARD_DARK } else { BOARD_LIGHT };
            canvas.fill_rect(cell(Point::new(x, y)), colour);
        }
    }
    if let Some(food) = game.food() {
        let c = cell(food);
        canvas.fill_ellipse(Rect::new(c.x + 1, c.y + 1, CELL - 2, CELL - 2), FOOD);
        canvas.fill_rect(Rect::new(c.x + CELL / 2, c.y, 2, 3), STALK);
    }
    // Tail first, so that the head is drawn over the cells it overlaps.
    let length = game.len() as i32;
    let cells: Vec<Point> = game.cells().collect();
    for (i, p) in cells.iter().enumerate().rev() {
        let colour = if i == 0 { HEAD } else { Colour::between(NECK, TAIL, i as i32, length) };
        let c = cell(*p);
        canvas.fill_round_rect(Rect::new(c.x + 1, c.y + 1, CELL - 2, CELL - 2), 4, colour);
    }
    // The eyes face the way it is going: side by side across the heading.
    let head = cell(game.head());
    let (dx, dy) = game.heading().delta();
    let (ex, ey) = if dx != 0 { (0, 3) } else { (3, 0) };
    for sign in [-1, 1] {
        canvas.fill_rect(
            Rect::new(
                head.x + CELL / 2 - 1 + sign * ex + dx * 3,
                head.y + CELL / 2 - 1 + sign * ey + dy * 3,
                2,
                2,
            ),
            EYE,
        );
    }

    let words = match (game.state(), game.paused()) {
        (State::Ready, _) => Some(("SNAKE", "Press Space to start - arrows or WASD to steer")),
        (State::Over, _) if game.won() => {
            Some(("YOU WIN", "The board is full - press Space to play again"))
        }
        (State::Over, _) => Some(("GAME OVER", "Press Space to play again")),
        (State::Playing, true) => Some(("PAUSED", "Press P to carry on")),
        (State::Playing, false) => None,
    };
    if let Some((big, small)) = words {
        canvas.fill_rect(Rect::new(area.x, top, SIZE.0, HEIGHT * CELL), Colour::argb(140, 0, 0, 0));
        let large = TextStyle::PLAIN.sized(3);
        canvas.text(
            area.x + (SIZE.0 - large.width(big)) / 2,
            top + 110 - 3 * ABOVE as i32,
            big,
            Colour::WHITE,
            large,
        );
        let plain = TextStyle::PLAIN;
        canvas.text(
            area.x + (SIZE.0 - plain.width(small)) / 2,
            top + 152 - ABOVE as i32,
            small,
            SUB_COLOUR,
            plain,
        );
    }
}

#[cfg(test)]
mod tests;
