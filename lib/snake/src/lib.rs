// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The rules of Snake, as 1.3's game had them (`classic/kernel/apps.c`, the `s_*` functions),
//! with no drawing and no clock (docs/design/0026-apps-m6-tests-and-stages.md, section 1).
//!
//! The app (`apps/snake`) draws a [`Game`] and calls [`Game::step`] when its timer goes off, a
//! number of milliseconds apart that the game itself says ([`Game::delay_ms`]). Because the
//! game never reads a clock and gets its random numbers from a seed it is given, a test can
//! play a whole game, move by move, and get the same game every time.
//!
//! The rules:
//! - the board is [`WIDTH`] by [`HEIGHT`] cells; the snake starts [`START_LENGTH`] long, head
//!   at (8, 10), heading right;
//! - each step moves the head one cell the way the snake is heading, and the tail follows;
//! - eating the food (landing the head on it) grows the snake by one cell, scores
//!   [`FOOD_POINTS`], and puts new food on a cell the snake isn't on;
//! - hitting a wall or the snake itself ends the game. Moving into the cell the tail is just
//!   leaving is allowed, since the tail will be gone by the time the head arrives;
//! - turns are asked for with [`Game::turn`] at any time and take effect at the next step. The
//!   latest turn asked for between two steps wins, unless it is straight back the way the
//!   snake came, which is ignored (so Up then Left, while heading Right, leaves it heading
//!   Right, as 1.3's `snext` did);
//! - a game that fills the whole board ends as a win ([`Game::won`]). 1.3 hung in its food
//!   search then.
//!
//! This is `no_std`: it uses only Rust's `core` and `alloc` libraries (`alloc` is the part
//! with growable collections such as `VecDeque`), so it can be linked into a PrettyBigRock
//! program as well as tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::collections::VecDeque;

/// The board's width, in cells.
pub const WIDTH: i32 = 28;
/// The board's height, in cells.
pub const HEIGHT: i32 = 20;
/// How long the snake is at the start of a game.
pub const START_LENGTH: usize = 4;
/// What each piece of food scores.
pub const FOOD_POINTS: u32 = 10;
/// The most cells there are: a snake this long has filled the board.
const CELLS: usize = (WIDTH * HEIGHT) as usize;

/// A cell of the board: `x` from 0 at the left, `y` from 0 at the top.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    /// Column.
    pub x: i32,
    /// Row.
    pub y: i32,
}

impl Point {
    /// The cell at (`x`, `y`).
    pub const fn new(x: i32, y: i32) -> Point {
        Point { x, y }
    }

    /// Whether it is on the board.
    pub const fn on_board(self) -> bool {
        self.x >= 0 && self.y >= 0 && self.x < WIDTH && self.y < HEIGHT
    }

    /// The cell one step on in `heading`.
    pub const fn go(self, heading: Heading) -> Point {
        let (dx, dy) = heading.delta();
        Point { x: self.x + dx, y: self.y + dy }
    }
}

/// The way the snake is going.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Heading {
    /// Towards larger `x`.
    Right,
    /// Towards larger `y` (down the screen).
    Down,
    /// Towards smaller `x`.
    Left,
    /// Towards smaller `y`.
    Up,
}

impl Heading {
    /// All four, in 1.3's order (its `DX` and `DY` tables).
    pub const ALL: [Heading; 4] = [Heading::Right, Heading::Down, Heading::Left, Heading::Up];

    /// How a step in this heading changes (`x`, `y`).
    pub const fn delta(self) -> (i32, i32) {
        match self {
            Heading::Right => (1, 0),
            Heading::Down => (0, 1),
            Heading::Left => (-1, 0),
            Heading::Up => (0, -1),
        }
    }

    /// The heading straight back the way this one came from.
    pub const fn reverse(self) -> Heading {
        match self {
            Heading::Right => Heading::Left,
            Heading::Down => Heading::Up,
            Heading::Left => Heading::Right,
            Heading::Up => Heading::Down,
        }
    }

    /// Its name, for logs: `"right"`, `"down"`, `"left"`, `"up"`.
    pub const fn name(self) -> &'static str {
        match self {
            Heading::Right => "right",
            Heading::Down => "down",
            Heading::Left => "left",
            Heading::Up => "up",
        }
    }
}

/// Where a game is, as 1.3 had it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Not started yet: the snake waits at its start.
    Ready,
    /// Being played (see [`Game::paused`]).
    Playing,
    /// Ended, by a crash or a win ([`Game::won`]).
    Over,
}

/// What ended a game.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Crash {
    /// The head went off the board.
    Wall,
    /// The head ran into the snake's own body.
    Itself,
}

/// What a call to [`Game::step`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Nothing: the game isn't being played, or is paused.
    Idle,
    /// The snake moved one cell.
    Moved,
    /// The snake moved onto the food, grew and scored.
    Ate,
    /// The snake crashed: the game is over.
    Died(Crash),
    /// The snake ate the last food and filled the board: the game is over, won.
    Won,
}

/// A small random number generator of the game's own, so that a seed makes a repeatable game:
/// *xorshift64\**, which is quick, tiny and good enough for choosing where food goes. (Not for
/// anything secret.)
#[derive(Clone, Debug)]
struct Rng(u64);

impl Rng {
    /// A generator for `seed`. The seed is first mixed (the *splitmix64* steps), so that
    /// seeds that differ in only a few bits, such as two readings of a clock, give games that
    /// don't resemble each other; and the state is never 0, which xorshift can't leave.
    fn new(seed: u64) -> Rng {
        let mut z = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^= z >> 31;
        Rng(if z == 0 { 0x9e37_79b9_7f4a_7c15 } else { z })
    }

    /// The next number.
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// A number from 0 up to but not including `n` (which must not be 0). Taking the top
    /// bits of a 64-bit number and scaling them down is fair enough for a game, and has no
    /// division by a variable.
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next() >> 11) * n as u128) >> 53) as usize
    }
}

/// A game of Snake.
#[derive(Clone, Debug)]
pub struct Game {
    /// The snake's cells, head first.
    body: VecDeque<Point>,
    heading: Heading,
    /// The turn asked for since the last step.
    next: Heading,
    food: Option<Point>,
    score: u32,
    best: u32,
    state: State,
    paused: bool,
    won: bool,
    rng: Rng,
}

impl Game {
    /// A new game, ready to start, whose food is placed from `seed`. The best score is 0 until
    /// [`Game::set_best`] says otherwise.
    pub fn new(seed: u64) -> Game {
        let mut game = Game {
            body: VecDeque::with_capacity(CELLS),
            heading: Heading::Right,
            next: Heading::Right,
            food: None,
            score: 0,
            best: 0,
            state: State::Ready,
            paused: false,
            won: false,
            rng: Rng::new(seed),
        };
        game.lay_out();
        game
    }

    /// Puts the snake at its start, clears the score and places food. The best score and the
    /// random numbers carry on.
    fn lay_out(&mut self) {
        self.body.clear();
        for i in 0..START_LENGTH as i32 {
            self.body.push_back(Point::new(8 - i, HEIGHT / 2));
        }
        self.heading = Heading::Right;
        self.next = Heading::Right;
        self.score = 0;
        self.paused = false;
        self.won = false;
        self.place_food();
    }

    /// Puts the food on a cell the snake isn't on, every free cell equally likely, or nowhere
    /// if there is none (the board is full). Counting the free cells and picking the n-th is
    /// what makes this finish: 1.3's "try random cells until one is free" never did on a full
    /// board.
    fn place_food(&mut self) {
        let free = CELLS - self.body.len();
        if free == 0 {
            self.food = None;
            return;
        }
        let mut pick = self.rng.below(free);
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = Point::new(x, y);
                if self.body.contains(&p) {
                    continue;
                }
                if pick == 0 {
                    self.food = Some(p);
                    return;
                }
                pick -= 1;
            }
        }
        // Unreachable while `body` holds only cells of the board; no panic if it ever isn't.
        self.food = None;
    }

    /// Starts a new game from Ready or Over (the best score is kept), and does nothing while
    /// one is being played.
    pub fn start(&mut self) {
        if self.state == State::Playing {
            return;
        }
        self.lay_out();
        self.state = State::Playing;
    }

    /// Pauses or resumes a game being played, as 1.3's P (and Space, while playing). It does
    /// nothing when no game is being played.
    pub fn toggle_pause(&mut self) {
        if self.state == State::Playing {
            self.paused = !self.paused;
        }
    }

    /// Pauses a game being played (and leaves one already paused, or not begun, alone): what
    /// the app does when its window loses the keyboard.
    pub fn pause(&mut self) {
        if self.state == State::Playing {
            self.paused = true;
        }
    }

    /// Asks to go `heading` from the next step on. The latest ask before a step wins; a turn
    /// straight back is ignored when that step comes (see the module's rules).
    pub fn turn(&mut self, heading: Heading) {
        self.next = heading;
    }

    /// Moves the snake one cell and says what happened. Does nothing ([`Step::Idle`]) unless a
    /// game is being played and isn't paused.
    pub fn step(&mut self) -> Step {
        if !self.running() {
            return Step::Idle;
        }
        // 1.3's rule: take the turn asked for unless it is straight back.
        if self.next != self.heading.reverse() {
            self.heading = self.next;
        }
        // `let ... else` takes the value out of the `Some`, or runs the `else` block, which
        // must leave. (`front` is `None` only for an empty body, which can't happen.)
        let Some(&head) = self.body.front() else {
            return Step::Idle;
        };
        let to = head.go(self.heading);
        if !to.on_board() {
            return self.end(Step::Died(Crash::Wall));
        }
        let eats = self.food == Some(to);
        // Without food the tail leaves its cell as the head moves, so the head may enter it.
        let blocking = self.body.len() - usize::from(!eats);
        if self.body.iter().take(blocking).any(|&p| p == to) {
            return self.end(Step::Died(Crash::Itself));
        }
        if !eats {
            self.body.pop_back();
            self.body.push_front(to);
            return Step::Moved;
        }
        self.body.push_front(to);
        self.score += FOOD_POINTS;
        self.best = self.best.max(self.score);
        self.place_food();
        if self.food.is_none() {
            self.won = true;
            return self.end(Step::Won);
        }
        Step::Ate
    }

    /// The game ends: records it and answers `what`, for `step` to return.
    fn end(&mut self, what: Step) -> Step {
        self.state = State::Over;
        self.paused = false;
        what
    }

    /// How long to wait before the next step, in milliseconds: 1.3's, which counted 10 ms
    /// ticks, 11 less one tick for every 40 points (so 110 ms at the start, 20 ms less per
    /// 80 points) down to 5 ticks (50 ms) from 250 points.
    pub fn delay_ms(&self) -> u32 {
        let ticks = if self.score >= 250 { 5 } else { 11 - self.score / 40 };
        ticks * 10
    }

    /// A game is being played and isn't paused: steps should be coming.
    pub fn running(&self) -> bool {
        self.state == State::Playing && !self.paused
    }

    /// Where the game is.
    pub fn state(&self) -> State {
        self.state
    }

    /// A game being played is paused.
    pub fn paused(&self) -> bool {
        self.paused
    }

    /// The game is over because the board is full.
    pub fn won(&self) -> bool {
        self.won
    }

    /// The score of this game.
    pub fn score(&self) -> u32 {
        self.score
    }

    /// The best score: of the games played since the program started, or what
    /// [`Game::set_best`] was told, whichever is more.
    pub fn best(&self) -> u32 {
        self.best
    }

    /// Tells the game a best score kept from before (it never lowers the best).
    pub fn set_best(&mut self, best: u32) {
        self.best = self.best.max(best);
    }

    /// The way the snake is heading now (a turn asked for shows only after the next step).
    pub fn heading(&self) -> Heading {
        self.heading
    }

    /// The head. (`Option` is Rust's "maybe a value": `Some(x)` or `None`.)
    pub fn head(&self) -> Point {
        self.body.front().copied().unwrap_or(Point::new(0, 0))
    }

    /// The food, or `None` when the board is full.
    pub fn food(&self) -> Option<Point> {
        self.food
    }

    /// How many cells long the snake is.
    pub fn len(&self) -> usize {
        self.body.len()
    }

    /// The snake never has no cells; this is here because a `len` wants an `is_empty`.
    pub fn is_empty(&self) -> bool {
        self.body.is_empty()
    }

    /// The snake's cells, from the head to the tail. `impl Iterator` says "something that
    /// hands out `Point`s one at a time" without naming its type; the `'_` ties it to the
    /// borrow of the game, so the game can't change while the cells are being read.
    pub fn cells(&self) -> impl Iterator<Item = Point> + '_ {
        self.body.iter().copied()
    }
}

#[cfg(test)]
mod tests;
