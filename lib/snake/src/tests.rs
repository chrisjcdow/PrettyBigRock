// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Playing Snake with seeds: starting, turning, eating, growing, scoring, crashing, pausing,
//! the best score, and a full board.

use super::*;

/// A game started with `seed`.
fn started(seed: u64) -> Game {
    let mut g = Game::new(seed);
    g.start();
    g
}

/// Puts the food on the cell just ahead of the head.
fn food_ahead(g: &mut Game) {
    g.food = Some(g.head().go(g.heading));
}

/// A heading that leads on towards the food without crashing: of the three that aren't
/// straight back, the safe ones, the one that gets closest. `None` if every way crashes.
fn bot(g: &Game) -> Option<Heading> {
    let food = g.food()?;
    Heading::ALL
        .iter()
        .copied()
        .filter(|h| *h != g.heading().reverse())
        .filter(|&h| {
            let to = g.head().go(h);
            // The tail's cell counts as free unless the move eats (it doesn't grow then).
            let blocked = g.len() - usize::from(Some(to) != g.food());
            to.on_board() && !g.cells().take(blocked).any(|p| p == to)
        })
        .min_by_key(|&h| {
            let to = g.head().go(h);
            (to.x - food.x).abs() + (to.y - food.y).abs()
        })
}

/// Plays with the bot until `score` is reached, within `moves` steps. Answers the moves used.
fn play_to(g: &mut Game, score: u32, moves: u32) -> Option<u32> {
    for n in 0..moves {
        if g.score() >= score {
            return Some(n);
        }
        let h = bot(g)?;
        g.turn(h);
        match g.step() {
            Step::Moved | Step::Ate => {}
            Step::Idle | Step::Died(_) | Step::Won => return None,
        }
    }
    None
}

#[test]
fn a_new_game_waits_with_the_snake_where_1_3_put_it() {
    let g = Game::new(1);
    assert_eq!(g.state(), State::Ready);
    assert_eq!(g.len(), 4);
    let cells: Vec<_> = g.cells().collect();
    assert_eq!(cells, [Point::new(8, 10), Point::new(7, 10), Point::new(6, 10), Point::new(5, 10)]);
    assert_eq!(g.heading(), Heading::Right);
    assert_eq!((g.score(), g.best()), (0, 0));
    assert!(g.food().is_some());
    assert!(!g.won());
}

#[test]
fn nothing_moves_until_it_is_started() {
    let mut g = Game::new(1);
    assert_eq!(g.step(), Step::Idle);
    assert_eq!(g.head(), Point::new(8, 10));
    // Pausing a game that hasn't begun does nothing either.
    g.toggle_pause();
    g.pause();
    assert!(!g.paused());
    g.start();
    assert_eq!(g.state(), State::Playing);
    assert_eq!(g.step(), Step::Moved);
    assert_eq!(g.head(), Point::new(9, 10));
    assert_eq!(g.len(), 4);
}

#[test]
fn the_snake_follows_its_head() {
    let mut g = started(2);
    g.food = Some(Point::new(0, 0));
    g.step();
    let cells: Vec<_> = g.cells().collect();
    assert_eq!(cells, [Point::new(9, 10), Point::new(8, 10), Point::new(7, 10), Point::new(6, 10)]);
}

#[test]
fn turning_changes_the_way_it_goes() {
    let mut g = started(3);
    g.food = Some(Point::new(0, 0));
    g.turn(Heading::Down);
    // The turn shows when the step is made.
    assert_eq!(g.heading(), Heading::Right);
    g.step();
    assert_eq!(g.heading(), Heading::Down);
    assert_eq!(g.head(), Point::new(8, 11));
    g.turn(Heading::Left);
    g.step();
    assert_eq!(g.head(), Point::new(7, 11));
}

#[test]
fn going_straight_back_is_ignored() {
    let mut g = started(4);
    g.food = Some(Point::new(0, 0));
    g.turn(Heading::Left);
    assert_eq!(g.step(), Step::Moved);
    assert_eq!(g.heading(), Heading::Right);
    assert_eq!(g.head(), Point::new(9, 10));
    // Also from another heading.
    g.turn(Heading::Up);
    g.step();
    g.turn(Heading::Down);
    g.step();
    assert_eq!(g.heading(), Heading::Up);
    assert_eq!(g.head(), Point::new(9, 8));
}

#[test]
fn of_two_turns_between_steps_the_latest_wins_unless_it_is_back() {
    let mut g = started(5);
    g.food = Some(Point::new(0, 0));
    g.turn(Heading::Up);
    g.turn(Heading::Down);
    g.step();
    assert_eq!(g.heading(), Heading::Down);
    // 1.3's snext: Up and then Left while going Down. Left wins.
    g.turn(Heading::Up);
    g.turn(Heading::Left);
    g.step();
    assert_eq!(g.heading(), Heading::Left);
    // Left and then Right (back) while going Left: the Right is ignored, not the Left kept.
    g.turn(Heading::Down);
    g.turn(Heading::Right);
    g.step();
    assert_eq!(g.heading(), Heading::Left);
}

#[test]
fn eating_grows_scores_and_moves_the_food() {
    let mut g = started(6);
    food_ahead(&mut g);
    let eaten = g.food().unwrap();
    assert_eq!(g.step(), Step::Ate);
    assert_eq!(g.len(), 5);
    assert_eq!(g.score(), 10);
    assert_eq!(g.best(), 10);
    assert_eq!(g.head(), eaten);
    let food = g.food().unwrap();
    assert!(g.cells().all(|p| p != food));
    // The tail didn't move on that step: the snake got longer.
    let cells: Vec<_> = g.cells().collect();
    assert_eq!(cells[4], Point::new(5, 10));
}

#[test]
fn steering_to_the_food_with_a_seed_eats_and_grows() {
    for seed in 0..20 {
        let mut g = started(seed);
        let used = play_to(&mut g, 50, 2_000);
        assert!(used.is_some(), "seed {seed} didn't get to 50");
        assert_eq!(g.len(), 4 + 5);
        assert_eq!(g.score(), 50);
        assert_eq!(g.best(), 50);
    }
}

#[test]
fn the_same_seed_gives_the_same_game() {
    let play = |seed| {
        let mut g = started(seed);
        let mut food = Vec::new();
        for _ in 0..200 {
            let Some(h) = bot(&g) else { break };
            g.turn(h);
            if g.step() == Step::Ate {
                food.push(g.food());
            }
        }
        food
    };
    assert_eq!(play(7), play(7));
    assert_ne!(play(7), play(8));
}

#[test]
fn food_is_never_on_the_snake() {
    for seed in 0..200 {
        let mut g = Game::new(seed);
        for k in 0..30 {
            let food = g.food().unwrap();
            assert!(food.on_board());
            assert!(g.cells().all(|p| p != food), "seed {seed}, food {k}");
            // Make the snake longer, wherever the food would land next.
            g.body.push_back(Point::new(20 + k % 8, k / 8));
            g.place_food();
        }
    }
}

#[test]
fn food_is_found_when_one_cell_is_free() {
    let mut g = Game::new(9);
    g.body.clear();
    let free = Point::new(13, 7);
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            if Point::new(x, y) != free {
                g.body.push_back(Point::new(x, y));
            }
        }
    }
    for _ in 0..10 {
        g.place_food();
        assert_eq!(g.food(), Some(free));
    }
}

#[test]
fn hitting_the_wall_ends_the_game() {
    let mut g = started(10);
    g.food = Some(Point::new(0, 0));
    // From x = 8 the head reaches x = 27 in 19 steps; the 20th leaves the board.
    for _ in 0..19 {
        assert_eq!(g.step(), Step::Moved);
    }
    assert_eq!(g.head(), Point::new(27, 10));
    assert_eq!(g.step(), Step::Died(Crash::Wall));
    assert_eq!(g.state(), State::Over);
    assert!(!g.won());
    // The snake stays where it was, and steps do nothing now.
    assert_eq!(g.head(), Point::new(27, 10));
    assert_eq!(g.step(), Step::Idle);
}

#[test]
fn each_wall_ends_it() {
    for (heading, steps) in [(Heading::Up, 11), (Heading::Down, 10)] {
        let mut g = started(11);
        g.food = Some(Point::new(0, 0));
        g.turn(heading);
        for _ in 0..steps - 1 {
            assert_eq!(g.step(), Step::Moved);
        }
        assert_eq!(g.step(), Step::Died(Crash::Wall), "{heading:?}");
    }
    // Left needs a turn first, since going Left from Right is going back: go Up, then Left.
    let mut g = started(11);
    g.food = Some(Point::new(27, 19));
    g.turn(Heading::Up);
    g.step();
    g.turn(Heading::Left);
    for _ in 0..8 {
        assert_eq!(g.step(), Step::Moved);
    }
    assert_eq!(g.head(), Point::new(0, 9));
    assert_eq!(g.step(), Step::Died(Crash::Wall));
}

/// Makes the snake `length` long, in a line going left from the head at (10, 10).
fn long_snake(length: usize) -> Game {
    let mut g = started(12);
    g.body.clear();
    for i in 0..length as i32 {
        g.body.push_back(Point::new(10 - i, 10));
    }
    g.food = Some(Point::new(27, 19));
    g
}

#[test]
fn running_into_itself_ends_the_game() {
    let mut g = long_snake(6);
    // Down, Left, Up: a loop the head closes onto the body's third cell from the tail end.
    g.turn(Heading::Down);
    assert_eq!(g.step(), Step::Moved);
    g.turn(Heading::Left);
    assert_eq!(g.step(), Step::Moved);
    g.turn(Heading::Up);
    // The head is at (9, 11); the cell above it, (9, 10), is the body, and not the tail.
    assert_eq!(g.step(), Step::Died(Crash::Itself));
    assert_eq!(g.state(), State::Over);
}

#[test]
fn moving_into_the_cell_the_tail_is_leaving_is_allowed() {
    // A snake of 4 turning in a 2 by 2 square: the head enters the tail's cell each time.
    let mut g = started(13);
    g.food = Some(Point::new(0, 0));
    g.turn(Heading::Down);
    assert_eq!(g.step(), Step::Moved);
    g.turn(Heading::Left);
    assert_eq!(g.step(), Step::Moved);
    g.turn(Heading::Up);
    assert_eq!(g.step(), Step::Moved);
    assert_eq!(g.head(), Point::new(7, 10));
    g.turn(Heading::Right);
    assert_eq!(g.step(), Step::Moved);
    assert_eq!(g.state(), State::Playing);
}

#[test]
fn but_not_into_the_tail_when_eating() {
    // The same square, with food where the tail is: the snake grows, so the tail stays and
    // the head hits it.
    let mut g = started(14);
    g.food = Some(Point::new(0, 0));
    g.turn(Heading::Down);
    g.step();
    g.turn(Heading::Left);
    g.step();
    g.turn(Heading::Up);
    g.food = Some(Point::new(7, 10));
    assert_eq!(g.step(), Step::Died(Crash::Itself));
}

#[test]
fn pausing_stops_the_steps() {
    let mut g = started(15);
    assert!(g.running());
    g.toggle_pause();
    assert!(g.paused());
    assert!(!g.running());
    assert_eq!(g.step(), Step::Idle);
    assert_eq!(g.head(), Point::new(8, 10));
    g.toggle_pause();
    assert_eq!(g.step(), Step::Moved);
    // `pause` only ever pauses.
    g.pause();
    g.pause();
    assert!(g.paused());
    // A new game isn't paused, and starting one while playing does nothing.
    g.start();
    assert!(g.paused());
    assert_eq!(g.head(), Point::new(9, 10));
}

#[test]
fn a_new_game_after_one_ends_keeps_the_best_score() {
    let mut g = started(16);
    play_to(&mut g, 30, 2_000).unwrap();
    assert_eq!((g.score(), g.best()), (30, 30));
    // Crash it.
    g.turn(Heading::Up);
    while g.step() != Step::Died(Crash::Wall) {}
    assert_eq!(g.state(), State::Over);
    g.start();
    assert_eq!(g.state(), State::Playing);
    assert_eq!((g.score(), g.best()), (0, 30));
    assert_eq!(g.len(), 4);
    assert_eq!(g.head(), Point::new(8, 10));
    assert_eq!(g.heading(), Heading::Right);
    // A worse game doesn't lower it; a better one raises it.
    food_ahead(&mut g);
    g.step();
    assert_eq!((g.score(), g.best()), (10, 30));
    // The best a program kept from before.
    let mut h = Game::new(1);
    h.set_best(120);
    h.set_best(40);
    assert_eq!(h.best(), 120);
}

#[test]
fn the_wait_between_steps_is_1_3s() {
    let mut g = Game::new(17);
    let at = |g: &mut Game, score| {
        g.score = score;
        g.delay_ms()
    };
    assert_eq!(at(&mut g, 0), 110);
    assert_eq!(at(&mut g, 30), 110);
    assert_eq!(at(&mut g, 40), 100);
    assert_eq!(at(&mut g, 120), 80);
    assert_eq!(at(&mut g, 240), 50);
    assert_eq!(at(&mut g, 249), 50);
    assert_eq!(at(&mut g, 250), 50);
    assert_eq!(at(&mut g, 10_000), 50);
}

/// A game one move from filling the board: the snake runs along each row in turn (to the right
/// on even rows, to the left on odd ones), ending on the cell next to the one free cell, which
/// has the food on it.
fn nearly_full() -> Game {
    let mut order = Vec::new();
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            order.push(Point::new(if y % 2 == 0 { x } else { WIDTH - 1 - x }, y));
        }
    }
    let last = order.pop().unwrap();
    let mut g = started(18);
    g.body.clear();
    // The head is the last cell walked; the tail the first.
    for p in order.iter().rev() {
        g.body.push_back(*p);
    }
    g.food = Some(last);
    // The last row is 19, odd: it runs right to left, so the free cell is (0, 19) and the
    // head is at (1, 19), heading Left.
    g.heading = Heading::Left;
    g.next = Heading::Left;
    g
}

#[test]
fn filling_the_board_ends_the_game_as_a_win() {
    let mut g = nearly_full();
    assert_eq!(g.len(), CELLS - 1);
    assert_eq!(g.head(), Point::new(1, 19));
    assert_eq!(g.step(), Step::Won);
    assert_eq!(g.len(), CELLS);
    assert_eq!(g.state(), State::Over);
    assert!(g.won());
    assert_eq!(g.food(), None);
    assert_eq!(g.score(), 10);
    assert_eq!(g.best(), 10);
    // The next game is a fresh one.
    g.start();
    assert!(!g.won());
    assert_eq!(g.len(), 4);
    assert!(g.food().is_some());
}

#[test]
fn a_crash_is_not_a_win() {
    let mut g = started(19);
    g.turn(Heading::Up);
    while g.step() == Step::Moved {}
    assert!(!g.won());
}

#[test]
fn any_seed_gives_a_working_generator() {
    // Seed 0 and seeds that differ in one bit must all work, and give different games.
    let mut foods = Vec::new();
    for seed in [0, 1, 2, 3, u64::MAX] {
        let g = Game::new(seed);
        foods.push(g.food().unwrap());
    }
    foods.sort_by_key(|p| (p.x, p.y));
    foods.dedup();
    assert!(foods.len() >= 4, "{foods:?}");
}
