// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The sums of PrettyBigRock's Calculator and of Terminal's `calc` command
//! (docs/design/0026-apps-m6-tests-and-stages.md, section 1), ported from 1.3
//! (`classic/kernel/lib.c` and `apps.c`).
//!
//! Three parts, all giving the answers 1.3 gave:
//! - [`eval`] works out an expression such as `(2+3)*4^2`, and [`format`] writes a number the
//!   way 1.3 showed it (so a shell command is `format(eval(text)?)`);
//! - [`parse`] reads a number as 1.3's `parse_double` did;
//! - [`Keypad`] is the calculator's keys: what each key does to the display.
//!
//! `#![no_std]` means this library uses only Rust's `core` and `alloc` libraries, not the
//! standard library (which needs an operating system underneath), so programs and the host can
//! both use it. `alloc` is the part of Rust's library that has `String` and `Vec`; it needs
//! only somewhere to get memory from.
//!
//! The numbers are `f64`, a 64-bit floating-point number: it keeps about 15 to 16 digits and
//! can't hold every decimal exactly (0.1 + 0.2 is 0.30000000000000004), which is why
//! [`format`] rounds to 9 places.
//!
//! Hostile input: Terminal's `calc` is given whatever the user types or pastes, so [`eval`]
//! must not be made to hang or overflow its stack. Powers are worked out by repeated squaring
//! (a handful of steps whatever the exponent), and brackets nest at most [`MAX_DEPTH`] deep.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

use alloc::string::{String, ToString};
use alloc::{format, vec};
use core::fmt;

/// How many brackets deep an expression may nest. The evaluator calls itself once (a few
/// functions, really) for each, and every call uses some of the program's stack, so without a
/// limit a line of a million `(` would run the stack out and stop the program.
pub const MAX_DEPTH: usize = 64;

/// What [`format`] writes for anything that isn't a number it can show (a division by zero
/// in the calculator, or beyond plus or minus 1e300).
pub const ERROR_TEXT: &str = "Error";

/// What went wrong working out an expression.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CalcError {
    /// A number or a bracket was expected, and something else (or nothing) was there: an empty
    /// expression, `2+`, `2+*3`, a letter.
    Expected,
    /// A bracket was opened and never closed: `(1`.
    Unclosed,
    /// Something was left over after a complete sum: `1)`, `1 2`.
    Extra,
    /// A division or remainder by zero.
    DivideByZero,
    /// An exponent that isn't a whole number, or is too big to be one: `2^0.5`.
    BadExponent,
    /// Brackets nested more than [`MAX_DEPTH`] deep.
    TooDeep,
}

impl fmt::Display for CalcError {
    /// The words a user can be shown (note 0002, rule 10: plain English).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            CalcError::Expected => "a number or a bracket was expected",
            CalcError::Unclosed => "a bracket was never closed",
            CalcError::Extra => "there is something left over after the sum",
            CalcError::DivideByZero => "you can't divide by zero",
            CalcError::BadExponent => "a power must be a whole number",
            CalcError::TooDeep => "the brackets are nested too deeply",
        })
    }
}

impl core::error::Error for CalcError {}

// ---------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------

/// `v` with its fractional part cut off, as 1.3's `dtrunc`: numbers beyond 9e15 are whole
/// already (a 64-bit float can't hold fractions that large), so they are returned as they are.
///
/// Converting with `as i64` cuts towards zero. In Rust `as` between numbers never fails: it
/// saturates at the end of the range, and a NaN (not-a-number) becomes 0, which is why the
/// range is checked first (which a NaN is never in).
fn trunc(v: f64) -> f64 {
    if (-9e15..=9e15).contains(&v) { v as i64 as f64 } else { v }
}

/// Reads a number from the start of `s` as 1.3's `parse_double` did: leading spaces, an
/// optional `-`, then digits with at most one `.`; it stops at anything else and answers what
/// it has so far (0 for nothing). It never fails, and has no exponents: `1e5` is 1.
pub fn parse(s: &str) -> f64 {
    let mut value = 0.0f64;
    let mut scale = 1.0f64;
    let mut negative = false;
    let mut after_point = false;
    let mut chars = s.trim_start_matches(' ').chars().peekable();
    if chars.peek() == Some(&'-') {
        negative = true;
        chars.next();
    }
    for c in chars {
        if c == '.' {
            if after_point {
                break;
            }
            after_point = true;
            continue;
        }
        let Some(digit) = c.to_digit(10) else { break };
        // 1.3 added each fractional digit as digit * (1/10)^place, and so do we, so that the
        // same text gives the same number to the last bit.
        if after_point {
            scale /= 10.0;
            value += f64::from(digit) * scale;
        } else {
            value = value * 10.0 + f64::from(digit);
        }
    }
    if negative { -value } else { value }
}

/// Writes `v` as 1.3's `fmt_double` did, character for character:
/// - [`ERROR_TEXT`] for NaN and anything beyond plus or minus 1e300 (infinity too);
/// - from 1e15 up, scientific style with six decimals: `1.000000e+15`;
/// - otherwise the whole part, then the fraction rounded to 9 places with the trailing zeros
///   dropped (and the point, if nothing is left): `0.3` for 0.1 + 0.2, `-12`, `0.333333333`;
/// - never `-0`.
pub fn format(v: f64) -> String {
    if v.is_nan() || !(-1e300..=1e300).contains(&v) {
        return ERROR_TEXT.to_string();
    }
    let sign = if v < 0.0 { "-" } else { "" };
    let mut v = if v < 0.0 { -v } else { v };
    if v >= 1e15 {
        let mut exponent = 0;
        while v >= 10.0 {
            v /= 10.0;
            exponent += 1;
        }
        // The mantissa (1.0 up to 9.9999995) times a million, rounded. Rounding can carry it
        // to 10.000000, which 1.3 printed as it came.
        let m = (v * 1e6 + 0.5) as u32;
        return format!("{sign}{}.{:06}e+{exponent}", m / 1_000_000, m % 1_000_000);
    }
    let mut whole = trunc(v);
    let fraction = v - whole;
    let mut nanos = (fraction * 1e9 + 0.5) as u32;
    // A fraction that rounds up to a whole one carries into the whole part.
    if nanos >= 1_000_000_000 {
        nanos -= 1_000_000_000;
        whole += 1.0;
    }
    // The whole part is under 1e15 plus one, so in two pieces of nine digits at most each.
    let high = (whole / 1e9) as u32;
    let low = (whole - f64::from(high) * 1e9) as u32;
    let whole_text = if high != 0 { format!("{high}{low:09}") } else { low.to_string() };
    if nanos != 0 {
        let digits = format!("{nanos:09}");
        format!("{sign}{whole_text}.{}", digits.trim_end_matches('0'))
    } else if high != 0 || low != 0 {
        format!("{sign}{whole_text}")
    } else {
        // Something like -0.0000000001: it shows as 0, not -0.
        whole_text
    }
}

// ---------------------------------------------------------------------------------------
// Expressions
// ---------------------------------------------------------------------------------------

/// Works out an expression, with 1.3's grammar:
///
/// - numbers (digits with an optional point), spaces anywhere between parts;
/// - `+ -` (lowest precedence), then `* x / %`, then `^` (highest, and right to left:
///   `2^3^2` is `2^9`), with brackets, and a unary `-` or `+` in front of a number or bracket
///   (so `-2^2` is `(-2)^2`, 4, as in 1.3);
/// - `x` means times as well as `*`;
/// - `%` is the remainder (`v - r * trunc(v / r)`, so it takes the sign of the left side);
/// - the exponent of `^` must be a whole number, which may be negative (`2^-2` is 0.25).
///
/// A result too big for a float is infinity, which [`format`] shows as `Error`.
pub fn eval(expr: &str) -> Result<f64, CalcError> {
    let mut parser = Parser { text: expr.as_bytes(), at: 0, depth: 0 };
    let value = parser.sum()?;
    parser.spaces();
    if parser.at < parser.text.len() {
        return Err(CalcError::Extra);
    }
    Ok(value)
}

/// `base` to the whole power `n`, by repeated squaring: 31 steps at most, where 1.3 multiplied
/// `n` times (and `2^2000000000` kept it busy for minutes).
fn power(base: f64, n: i32) -> f64 {
    let mut result = 1.0;
    let mut square = base;
    let mut left = n.unsigned_abs();
    while left > 0 {
        if left & 1 == 1 {
            result *= square;
        }
        square *= square;
        left >>= 1;
    }
    if n < 0 { 1.0 / result } else { result }
}

/// The whole number `x` is, if it is one that 1.3's 32-bit `int` could hold.
fn exponent(x: f64) -> Option<i32> {
    // Both ends are exactly representable in an f64, so the comparison is exact.
    if x >= f64::from(i32::MIN) && x <= f64::from(i32::MAX) && x == trunc(x) {
        Some(x as i32)
    } else {
        None
    }
}

/// A recursive-descent parser: one method for each level of the grammar, each calling the one
/// above it in precedence (`sum` calls `product`, which calls `powers`, which calls `atom`,
/// which calls `sum` again inside brackets). It works out the value as it reads.
struct Parser<'t> {
    text: &'t [u8],
    at: usize,
    /// How many brackets are open now.
    depth: usize,
}

impl Parser<'_> {
    /// The byte at the reader's place, or 0 at the end (which no part of the grammar uses).
    fn peek(&self) -> u8 {
        self.text.get(self.at).copied().unwrap_or(0)
    }

    fn spaces(&mut self) {
        while self.peek() == b' ' {
            self.at += 1;
        }
    }

    /// A number, a bracket, or either with signs in front.
    fn atom(&mut self) -> Result<f64, CalcError> {
        // Any number of signs: each `-` flips the result, a `+` changes nothing. A loop, not
        // a call to itself, so `-------…` can't run the stack out.
        let mut negative = false;
        loop {
            self.spaces();
            match self.peek() {
                b'-' => negative = !negative,
                b'+' => {}
                _ => break,
            }
            self.at += 1;
        }
        let value = match self.peek() {
            b'(' => {
                if self.depth >= MAX_DEPTH {
                    return Err(CalcError::TooDeep);
                }
                self.at += 1;
                self.depth += 1;
                let inner = self.sum()?;
                self.depth -= 1;
                self.spaces();
                if self.peek() != b')' {
                    return Err(CalcError::Unclosed);
                }
                self.at += 1;
                inner
            }
            b'0'..=b'9' | b'.' => {
                // 1.3 took the whole run of digits and points, and read a number from its
                // start: `1.2.3` is 1.2.
                let start = self.at;
                while matches!(self.peek(), b'0'..=b'9' | b'.') {
                    self.at += 1;
                }
                // The run is ASCII, so it is valid text.
                parse(core::str::from_utf8(&self.text[start..self.at]).unwrap_or(""))
            }
            _ => return Err(CalcError::Expected),
        };
        Ok(if negative { -value } else { value })
    }

    /// Atoms joined by `^`, which groups to the right: read all the operands first, then
    /// work from the last, so that `2^2^2^…` is a loop and not a deep chain of calls.
    fn powers(&mut self) -> Result<f64, CalcError> {
        let mut operands = vec![self.atom()?];
        loop {
            self.spaces();
            if self.peek() != b'^' {
                break;
            }
            self.at += 1;
            operands.push(self.atom()?);
        }
        let mut result = operands.pop().unwrap_or(0.0);
        while let Some(base) = operands.pop() {
            result = power(base, exponent(result).ok_or(CalcError::BadExponent)?);
        }
        Ok(result)
    }

    /// Powers joined by `*`, `x`, `/` and `%`.
    fn product(&mut self) -> Result<f64, CalcError> {
        let mut value = self.powers()?;
        loop {
            self.spaces();
            let op = self.peek();
            if !matches!(op, b'*' | b'x' | b'/' | b'%') {
                return Ok(value);
            }
            self.at += 1;
            let right = self.powers()?;
            value = match op {
                b'/' | b'%' if right == 0.0 => return Err(CalcError::DivideByZero),
                b'/' => value / right,
                b'%' => value - right * trunc(value / right),
                _ => value * right,
            };
        }
    }

    /// Products joined by `+` and `-`.
    fn sum(&mut self) -> Result<f64, CalcError> {
        let mut value = self.product()?;
        loop {
            self.spaces();
            let op = self.peek();
            if op != b'+' && op != b'-' {
                return Ok(value);
            }
            self.at += 1;
            let right = self.product()?;
            value = if op == b'+' { value + right } else { value - right };
        }
    }
}

// ---------------------------------------------------------------------------------------
// The calculator's keys
// ---------------------------------------------------------------------------------------

/// How many characters the display holds: 1.3 kept it in a 24-byte buffer, one of which is
/// the end mark. A result longer than this (a number with 15 digits before the point and
/// nine after) is cut short, as it was in 1.3.
const DISPLAY_MAX: usize = 23;

/// How many characters may be typed into the display (digits, the point and a minus sign).
const ENTRY_MAX: usize = 15;

/// An operator key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    /// `+`
    Add,
    /// `-`
    Subtract,
    /// Times: `*` on 1.3's key, `x` on its small line.
    Multiply,
    /// `/`
    Divide,
}

impl Op {
    /// The character 1.3 showed for it on the small line above the display.
    pub fn symbol(self) -> char {
        match self {
            Op::Add => '+',
            Op::Subtract => '-',
            Op::Multiply => 'x',
            Op::Divide => '/',
        }
    }

    /// `a` and `b` combined by it. A division by zero is NaN, which [`format`] shows as
    /// `Error`.
    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            Op::Add => a + b,
            Op::Subtract => a - b,
            Op::Multiply => a * b,
            Op::Divide if b == 0.0 => f64::NAN,
            Op::Divide => a / b,
        }
    }
}

/// A key of the calculator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A digit, 0 to 9 (anything higher is ignored).
    Digit(u8),
    /// The decimal point.
    Point,
    /// `C`: back to 0 with nothing pending.
    Clear,
    /// `+/-`: changes the sign of what is shown.
    Negate,
    /// `%`: what is shown, divided by 100.
    Percent,
    /// `<`: takes off the last character typed.
    Back,
    /// `=`
    Equals,
    /// `+`, `-`, times and divide.
    Op(Op),
}

impl Key {
    /// The key a typed character stands for, as 1.3's keyboard handling had it: digits, `.`,
    /// `+ - * / %`, `x` for times, `=`, and `c` or `C` for clear. (Enter, Backspace and Escape
    /// are not characters; the program maps them to [`Key::Equals`], [`Key::Back`] and
    /// [`Key::Clear`]. The signs `× ÷ −` are accepted too, for text pasted from elsewhere.)
    pub fn from_char(c: char) -> Option<Key> {
        Some(match c {
            '0'..='9' => Key::Digit(c as u8 - b'0'),
            '.' => Key::Point,
            '+' => Key::Op(Op::Add),
            '-' | '\u{2212}' => Key::Op(Op::Subtract),
            '*' | 'x' | '\u{d7}' => Key::Op(Op::Multiply),
            '/' | '\u{f7}' => Key::Op(Op::Divide),
            '%' => Key::Percent,
            '=' => Key::Equals,
            'c' | 'C' => Key::Clear,
            _ => return None,
        })
    }
}

/// The calculator's state, and what each key does to it, exactly as 1.3's `cpress` did.
///
/// It works like a pocket calculator: digits build the number on the display; an operator key
/// keeps that number as the *accumulator* and waits for the next; the next operator (or `=`)
/// combines the two. `2 + 3 + 4 =` shows 5 at the second `+` and 9 at the end. An error
/// (`5 / 0 =`) shows `Error` and ignores every key but `C`.
#[derive(Clone, Debug, PartialEq)]
pub struct Keypad {
    /// What the display shows.
    display: String,
    /// The number kept when an operator was pressed.
    acc: f64,
    /// The operator waiting for its second number.
    op: Option<Op>,
    /// The next digit starts a new number, instead of adding to the one shown (after an
    /// operator, `=` or `%`, and at the start).
    fresh: bool,
}

impl Default for Keypad {
    fn default() -> Keypad {
        Keypad::new()
    }
}

impl Keypad {
    /// A calculator showing 0.
    pub fn new() -> Keypad {
        Keypad { display: "0".to_string(), acc: 0.0, op: None, fresh: true }
    }

    /// What the display shows.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Whether the display shows `Error`.
    pub fn is_error(&self) -> bool {
        self.display == ERROR_TEXT
    }

    /// What the small line above the display shows while an operator is waiting: the
    /// accumulator and the operator, `12 +` or `3 x`. `None` when nothing is pending.
    pub fn pending(&self) -> Option<String> {
        self.op.map(|op| format!("{} {}", format(self.acc), op.symbol()))
    }

    /// Shows `v`, cut to what the display holds.
    fn show(&mut self, v: f64) {
        self.display = format(v);
        self.display.truncate(DISPLAY_MAX);
    }

    /// Presses a key.
    pub fn press(&mut self, key: Key) {
        // An error stays until `C`.
        if self.is_error() && key != Key::Clear {
            return;
        }
        match key {
            Key::Digit(d) if d <= 9 => self.enter(char::from(b'0' + d)),
            Key::Digit(_) => {}
            Key::Point => self.enter('.'),
            Key::Clear => *self = Keypad::new(),
            Key::Negate => self.show(-parse(&self.display)),
            Key::Percent => {
                self.show(parse(&self.display) / 100.0);
                self.fresh = true;
            }
            Key::Back => {
                if self.fresh {
                    return;
                }
                let length = self.display.chars().count();
                if length <= 1 || (length == 2 && self.display.starts_with('-')) {
                    self.display = "0".to_string();
                } else {
                    self.display.pop();
                }
            }
            Key::Equals => {
                if let Some(op) = self.op.take() {
                    self.show(op.apply(self.acc, parse(&self.display)));
                }
                self.fresh = true;
            }
            Key::Op(op) => {
                let shown = parse(&self.display);
                match self.op {
                    // One waiting and a number typed since: work it out first (`2 + 3 +`
                    // shows 5).
                    Some(waiting) if !self.fresh => {
                        self.acc = waiting.apply(self.acc, shown);
                        self.show(self.acc);
                    }
                    // A second operator in a row only changes which one is waiting.
                    Some(_) => {}
                    None => self.acc = shown,
                }
                self.op = Some(op);
                self.fresh = true;
            }
        }
    }

    /// Types a digit or the point into the display.
    fn enter(&mut self, c: char) {
        if self.fresh {
            self.display = if c == '.' { "0.".to_string() } else { c.to_string() };
            self.fresh = false;
            return;
        }
        if c == '.' && self.display.contains('.') {
            return;
        }
        if self.display.chars().count() >= ENTRY_MAX {
            return;
        }
        // A lone 0 is replaced by the digit, not followed by it: no `007`.
        if self.display == "0" && c != '.' {
            self.display.clear();
        }
        self.display.push(c);
    }

    /// Takes a pasted number as if it had been typed: an optional `-`, digits and at most one
    /// point, at most 15 characters, with spaces round it ignored. Anything else is refused
    /// (`false`) and changes nothing, as does pasting while the display shows `Error`.
    pub fn paste(&mut self, text: &str) -> bool {
        let text = text.trim();
        let (negative, digits) = match text.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let is_number = digits.bytes().any(|b| b.is_ascii_digit())
            && digits.bytes().all(|b| b.is_ascii_digit() || b == b'.')
            && digits.bytes().filter(|&b| b == b'.').count() <= 1;
        if self.is_error() || !is_number || text.chars().count() > ENTRY_MAX {
            return false;
        }
        // Tidy it as typing would have: `.5` is `0.5`, `007` is `7`, `-0` is `0`.
        let tidy = digits.trim_start_matches('0');
        let tidy = if tidy.is_empty() || tidy.starts_with('.') {
            format!("0{tidy}")
        } else {
            tidy.to_string()
        };
        let zero = tidy.bytes().all(|b| b == b'0' || b == b'.');
        self.display = if negative && !zero { format!("-{tidy}") } else { tidy };
        self.fresh = false;
        true
    }
}

#[cfg(test)]
mod tests;
