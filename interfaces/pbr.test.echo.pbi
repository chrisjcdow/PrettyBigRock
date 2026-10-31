// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// A test interface, served by the test program `echo` and called by `probe` in the program
/// self-test. It uses every kind of type the interface language has, so the generated code
/// is tested end to end on PrettyBigRock.
interface pbr.test.echo version 1;

/// Sends `text` back reversed (by characters).
@1 fn reverse(text: string<200>) -> (text: string<200>);

/// Adds two numbers, failing if the sum doesn't fit.
@2 fn add(a: u32, b: u32) -> (sum: u32) error EchoError;

/// Turns a pair inside out: `left` becomes the length of `right` (or -1 if it's absent), and
/// `right` becomes `left` written in decimal. The flags come back as they were, with the
/// pair's shape.
@3 fn swap(pair: Pair, flags: Flags) -> (pair: Pair, flags: Flags, shape: Shape);

/// Sends a notification capability back, signalling it with `bits` first; and a memory
/// capability's size, if one came.
@4 fn pass(bits: u64, note: handle<notification>, memory: option<handle<memory>>)
    -> (note: handle<notification>, size: option<u64>);

/// Totals a list of numbers (wrapping), and lists the names of the items longer than `min`
/// bytes. `grid` counts up from the number of values.
@5 fn total(values: list<i64, 64>, items: list<Item, 16>, min: u16)
    -> (sum: i64, long_names: list<string<32>, 16>, grid: array<array<u8, 3>, 2>);

/// Answers with a failure of the service itself.
@6 fn fail();

/// Operation 7 belonged to an earlier draft.
reserved @7;

/// Two sides, one of which may be missing.
struct Pair {
    left: i16,
    /// Absent means "none".
    right: option<string<32>>,
}

/// An item with a name.
struct Item {
    name: string<32>,
    weight: u8,
    tags: list<u16, 4>,
}

/// What went wrong.
flexible enum EchoError {
    /// The sum didn't fit in 32 bits.
    Overflow @1,
}

/// Options for `swap`.
bits Flags: u8 {
    Loud @0,
    Twice @3,
}

/// How a pair looked.
enum Shape {
    Whole @0,
    LeftOnly @1,
}

/// The longest name an item can have.
const MAX_NAME: u32 = 32;
