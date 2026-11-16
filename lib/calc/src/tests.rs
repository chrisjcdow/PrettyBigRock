// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Host tests of `pbr-calc`: expressions (1.3's own examples, each operator, errors, hostile
//! input), `format` against what 1.3's C code printed for the same numbers, and the keypad's
//! key sequences.

use super::*;
use std::time::{Duration, Instant};

fn sum(expr: &str) -> f64 {
    eval(expr).unwrap_or_else(|e| panic!("{expr:?} failed: {e}"))
}

#[test]
fn the_examples_1_3_gave() {
    assert_eq!(sum("(2+3)*4^2"), 80.0);
    assert_eq!(sum("12*(3+4)/2"), 42.0);
}

#[test]
fn each_operator() {
    assert_eq!(sum("7+5"), 12.0);
    assert_eq!(sum("7-5"), 2.0);
    assert_eq!(sum("7*5"), 35.0);
    assert_eq!(sum("7x5"), 35.0);
    assert_eq!(sum("7/2"), 3.5);
    assert_eq!(sum("7%4"), 3.0);
    assert_eq!(sum("2^10"), 1024.0);
    assert_eq!(sum("1.5+2.25"), 3.75);
    assert_eq!(sum(".5+.5"), 1.0);
}

#[test]
fn precedence_and_grouping() {
    assert_eq!(sum("2+3*4"), 14.0);
    assert_eq!(sum("(2+3)*4"), 20.0);
    assert_eq!(sum("10-4-3"), 3.0);
    assert_eq!(sum("100/10/5"), 2.0);
    assert_eq!(sum("2*3%4"), 2.0);
    assert_eq!(sum("2*3^2"), 18.0);
    // Powers group to the right: 2^(3^2), not (2^3)^2.
    assert_eq!(sum("2^3^2"), 512.0);
    assert_eq!(sum("((((7))))"), 7.0);
    assert_eq!(sum("  1 +  2 *  ( 3 + 4 ) "), 15.0);
}

#[test]
fn unary_signs() {
    assert_eq!(sum("-3"), -3.0);
    assert_eq!(sum("+3"), 3.0);
    assert_eq!(sum("--3"), 3.0);
    assert_eq!(sum("- -3"), 3.0);
    assert_eq!(sum("2*-3"), -6.0);
    assert_eq!(sum("2--3"), 5.0);
    assert_eq!(sum("-(1+2)"), -3.0);
    // As in 1.3, the sign belongs to the number, so it is raised to the power.
    assert_eq!(sum("-2^2"), 4.0);
}

#[test]
fn remainder_takes_the_sign_of_the_left_side() {
    assert_eq!(sum("10%3"), 1.0);
    assert_eq!(sum("-10%3"), -1.0);
    assert_eq!(sum("10%-3"), 1.0);
    assert_eq!(sum("5.5%2"), 1.5);
}

#[test]
fn powers_with_whole_exponents() {
    assert_eq!(sum("2^0"), 1.0);
    assert_eq!(sum("0^0"), 1.0);
    assert_eq!(sum("2^-2"), 0.25);
    assert_eq!(sum("2^(1+2)"), 8.0);
    assert_eq!(sum("(-2)^3"), -8.0);
    assert_eq!(sum("2^1"), 2.0);
    assert_eq!(sum("10^15"), 1e15);
    // Too big for a float: infinity, which `format` shows as Error.
    assert_eq!(sum("10^400"), f64::INFINITY);
    assert_eq!(format(sum("10^400")), "Error");
    assert_eq!(sum("10^-400"), 0.0);
}

#[test]
fn numbers_read_as_1_3_read_them() {
    assert_eq!(sum("1.2.3"), 1.2);
    assert_eq!(sum("."), 0.0);
    assert_eq!(sum("007"), 7.0);
    assert_eq!(sum("0.1+0.2"), 0.1 + 0.2);
    assert_eq!(parse("12.5"), 12.5);
    assert_eq!(parse("  -3.25xyz"), -3.25);
    assert_eq!(parse("1e5"), 1.0);
    assert_eq!(parse(""), 0.0);
    assert_eq!(parse("abc"), 0.0);
    assert_eq!(parse("1.2.3"), 1.2);
    assert_eq!(parse("-"), 0.0);
    assert_eq!(parse("1.000000e+15"), 1.0);
}

#[test]
fn errors_say_what_was_wrong() {
    assert_eq!(eval(""), Err(CalcError::Expected));
    assert_eq!(eval("   "), Err(CalcError::Expected));
    assert_eq!(eval("2+"), Err(CalcError::Expected));
    assert_eq!(eval("2+*3"), Err(CalcError::Expected));
    assert_eq!(eval("abc"), Err(CalcError::Expected));
    assert_eq!(eval("()"), Err(CalcError::Expected));
    assert_eq!(eval("(1"), Err(CalcError::Unclosed));
    assert_eq!(eval("(1+(2)"), Err(CalcError::Unclosed));
    assert_eq!(eval("1)"), Err(CalcError::Extra));
    assert_eq!(eval("1 2"), Err(CalcError::Extra));
    assert_eq!(eval("1/0"), Err(CalcError::DivideByZero));
    assert_eq!(eval("5%0"), Err(CalcError::DivideByZero));
    assert_eq!(eval("1/(2-2)"), Err(CalcError::DivideByZero));
    assert_eq!(eval("2^0.5"), Err(CalcError::BadExponent));
    assert_eq!(eval("2^(1/2)"), Err(CalcError::BadExponent));
    assert_eq!(eval("2^3000000000"), Err(CalcError::BadExponent));
    assert_eq!(eval("2^-3000000000"), Err(CalcError::BadExponent));
    // Every error has words for a user.
    for e in [
        CalcError::Expected,
        CalcError::Unclosed,
        CalcError::Extra,
        CalcError::DivideByZero,
        CalcError::BadExponent,
        CalcError::TooDeep,
    ] {
        assert!(!e.to_string().is_empty());
    }
}

/// Runs `f`, which must finish within a second (they take microseconds; the limit is for a
/// hang, not a slow machine), and answers what it did.
fn quickly<T>(f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let out = f();
    assert!(start.elapsed() < Duration::from_secs(1), "took {:?}", start.elapsed());
    out
}

#[test]
fn hostile_input_finishes_quickly() {
    // 1.3 looped once for each unit of the exponent, so these hung it.
    assert_eq!(quickly(|| eval("2^2000000000")), Ok(f64::INFINITY));
    assert_eq!(quickly(|| eval("1^2000000000")), Ok(1.0));
    assert_eq!(quickly(|| eval("2^-2147483648")), Ok(0.0));
    assert_eq!(quickly(|| eval("2^2147483647")), Ok(f64::INFINITY));
    // Brackets nest to MAX_DEPTH and no further; a million of them don't overflow the stack.
    let ok = format!("{}1{}", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
    assert_eq!(quickly(|| eval(&ok)), Ok(1.0));
    let deep = format!("{}1{}", "(".repeat(MAX_DEPTH + 1), ")".repeat(MAX_DEPTH + 1));
    assert_eq!(quickly(|| eval(&deep)), Err(CalcError::TooDeep));
    assert_eq!(quickly(|| eval(&"(".repeat(1_000_000))), Err(CalcError::TooDeep));
    // The depth is back to what it was after a bracket closes, so many in a row are fine.
    let many = vec!["(1)"; 100_000].join("+");
    assert_eq!(quickly(|| eval(&many)), Ok(100_000.0));
    // Long runs of signs, powers and operators are loops, not recursion.
    assert_eq!(quickly(|| eval(&format!("{}1", "-".repeat(1_000_001)))), Ok(-1.0));
    assert_eq!(quickly(|| eval(&format!("1{}", "^1".repeat(500_000)))), Ok(1.0));
    assert_eq!(quickly(|| eval(&format!("2{}", "^1".repeat(500_000)))), Ok(2.0));
    assert!(quickly(|| eval(&"1+".repeat(500_000))).is_err());
    // A very long number, and a very long input that is not an expression.
    assert!(quickly(|| eval(&"9".repeat(1_000_000))).is_ok());
    assert!(quickly(|| eval(&"1.".repeat(500_000))).is_ok());
    assert!(quickly(|| eval(&"\u{1F980}".repeat(100_000))).is_err());
    assert!(quickly(|| eval(&" ".repeat(1_000_000))).is_err());
}

#[test]
fn input_that_is_not_text_a_calculator_expects() {
    // Multi-byte characters are never split, and are just something unexpected.
    assert_eq!(eval("2\u{d7}3"), Err(CalcError::Extra));
    assert_eq!(eval("\u{e9}"), Err(CalcError::Expected));
    assert_eq!(eval("1\u{0}"), Err(CalcError::Extra));
}

/// What 1.3's C `fmt_double` printed for each number (worked out by compiling `classic/kernel/
/// lib.c`'s own functions on the host, with the x87 truncation replaced by a cast).
#[test]
fn format_as_1_3_did() {
    let table: &[(f64, &str)] = &[
        (0.0, "0"),
        (-0.0, "0"),
        (0.1, "0.1"),
        (0.1 + 0.2, "0.3"),
        (-0.5, "-0.5"),
        (1.0 / 3.0, "0.333333333"),
        (2.0 / 3.0, "0.666666667"),
        (12.0, "12"),
        (-12.0, "-12"),
        (2.5, "2.5"),
        (1234567.125, "1234567.125"),
        (100.0, "100"),
        (1e9, "1000000000"),
        (1e10, "10000000000"),
        (4294967296.0, "4294967296"),
        (1e14, "100000000000000"),
        (123456789012345.0, "123456789012345"),
        (999999999999999.0, "999999999999999"),
        (999999999999999.9, "999999999999999.875"),
        (123456789012345.67, "123456789012345.671875"),
        (1e15, "1.000000e+15"),
        (-1e15, "-1.000000e+15"),
        (9999999999999999.0, "1.000000e+16"),
        (1e22, "1.000000e+22"),
        (1.23456789e25, "1.234568e+25"),
        // The mantissa rounds up to 10.000000 and is printed as it comes, as in 1.3.
        (9.9999995e15, "10.000000e+15"),
        (1e300, "1.000000e+300"),
        (-1e300, "-1.000000e+300"),
        (1.5e300, "Error"),
        (-1.5e300, "Error"),
        (f64::NAN, "Error"),
        (f64::INFINITY, "Error"),
        (f64::NEG_INFINITY, "Error"),
        // Small numbers: the ninth decimal place rounds half up, a nothing is not -0, and a
        // fraction that rounds up to a whole one carries.
        (0.000000001, "0.000000001"),
        (0.0000000004, "0"),
        (0.0000000005, "0.000000001"),
        (-1e-12, "0"),
        (0.9999999996, "1"),
        (99999999.9999999999, "100000000"),
    ];
    for &(v, want) in table {
        assert_eq!(format(v), want, "format({v:e})");
    }
}

#[test]
fn format_round_trips_through_eval() {
    for text in ["80", "42", "-7.25", "0.125", "123456789"] {
        assert_eq!(format(sum(text)), text);
    }
}

// ---- The keypad. ----

/// Presses keys written as 1.3's keyboard would type them (`c` for clear, `<` for back).
fn keys(pad: &mut Keypad, text: &str) {
    for c in text.chars() {
        let key = if c == '<' {
            Key::Back
        } else if c == '~' {
            Key::Negate
        } else {
            Key::from_char(c).unwrap_or_else(|| panic!("no key for {c:?}"))
        };
        pad.press(key);
    }
}

/// A new keypad after `text`.
fn typed(text: &str) -> Keypad {
    let mut pad = Keypad::new();
    keys(&mut pad, text);
    pad
}

#[test]
fn a_new_keypad_shows_zero() {
    let pad = Keypad::new();
    assert_eq!(pad.display(), "0");
    assert_eq!(pad.pending(), None);
    assert!(!pad.is_error());
}

#[test]
fn sums_with_the_keys() {
    assert_eq!(typed("12+7=").display(), "19");
    assert_eq!(typed("9*3-4=").display(), "23");
    assert_eq!(typed("2+3+4=").display(), "9");
    assert_eq!(typed("8/4=").display(), "2");
    assert_eq!(typed("1/3=").display(), "0.333333333");
    assert_eq!(typed("0.1+0.2=").display(), "0.3");
    assert_eq!(typed("5-8=").display(), "-3");
}

#[test]
fn chained_operators_work_out_as_they_go() {
    let mut pad = typed("2+3");
    assert_eq!(pad.pending().as_deref(), Some("2 +"));
    keys(&mut pad, "+");
    // The second plus worked out 2 + 3.
    assert_eq!(pad.display(), "5");
    assert_eq!(pad.pending().as_deref(), Some("5 +"));
    keys(&mut pad, "4=");
    assert_eq!(pad.display(), "9");
    assert_eq!(pad.pending(), None);
    // Left to right, with no precedence: this is a pocket calculator.
    assert_eq!(typed("2+3*4=").display(), "20");
}

#[test]
fn an_operator_pressed_twice_only_changes_the_operator() {
    let mut pad = typed("6+");
    keys(&mut pad, "*");
    assert_eq!(pad.display(), "6");
    assert_eq!(pad.pending().as_deref(), Some("6 x"));
    keys(&mut pad, "7=");
    assert_eq!(pad.display(), "42");
}

#[test]
fn the_pending_line_writes_x_for_times() {
    assert_eq!(typed("3*").pending().as_deref(), Some("3 x"));
    assert_eq!(typed("3x").pending().as_deref(), Some("3 x"));
    assert_eq!(typed("3/").pending().as_deref(), Some("3 /"));
    assert_eq!(typed("3-").pending().as_deref(), Some("3 -"));
    assert_eq!(typed("0.5+").pending().as_deref(), Some("0.5 +"));
}

#[test]
fn an_error_sticks_until_clear() {
    let mut pad = typed("5/0=");
    assert_eq!(pad.display(), "Error");
    assert!(pad.is_error());
    // Nothing but C works.
    keys(&mut pad, "123+-*/%=<.~");
    pad.press(Key::Digit(4));
    assert_eq!(pad.display(), "Error");
    assert!(!pad.paste("5"));
    keys(&mut pad, "c");
    assert_eq!(pad.display(), "0");
    assert_eq!(pad.pending(), None);
    keys(&mut pad, "2+2=");
    assert_eq!(pad.display(), "4");
    // An error in the middle of a chain, too.
    let pad = typed("5/0+");
    assert_eq!(pad.display(), "Error");
}

#[test]
fn digits_and_the_point() {
    assert_eq!(typed("007").display(), "7");
    assert_eq!(typed("0").display(), "0");
    assert_eq!(typed(".").display(), "0.");
    assert_eq!(typed(".5").display(), "0.5");
    assert_eq!(typed("1.5.5").display(), "1.55");
    assert_eq!(typed("1..").display(), "1.");
    assert_eq!(typed("0.0").display(), "0.0");
    // Digits past 9 are no key.
    let mut pad = typed("1");
    pad.press(Key::Digit(12));
    assert_eq!(pad.display(), "1");
}

#[test]
fn fifteen_characters_fit_and_no_more() {
    let pad = typed("1234567890123456789");
    assert_eq!(pad.display(), "123456789012345");
    // The point and a minus sign count: 1.3 counted characters, not digits.
    let pad = typed("1.2345678901234567");
    assert_eq!(pad.display(), "1.2345678901234");
    assert_eq!(pad.display().len(), 15);
    // Not fresh after a result either: the result is replaced by the first digit.
    assert_eq!(typed("1+1=9").display(), "9");
}

#[test]
fn clear_forgets_everything() {
    let pad = typed("12+7c");
    assert_eq!(pad.display(), "0");
    assert_eq!(pad.pending(), None);
    assert_eq!(pad, Keypad::new());
    // The old accumulator is gone: `=` with no operator changes nothing.
    assert_eq!(typed("12+7c=").display(), "0");
    assert_eq!(typed("12+7c5+1=").display(), "6");
}

#[test]
fn plus_minus_changes_the_sign() {
    assert_eq!(typed("5~").display(), "-5");
    assert_eq!(typed("5~~").display(), "5");
    assert_eq!(typed("0~").display(), "0");
    assert_eq!(typed("2.5~").display(), "-2.5");
    assert_eq!(typed("5~+3=").display(), "-2");
    // After a result it changes the result, and the next digit starts afresh.
    assert_eq!(typed("2+3=~").display(), "-5");
    assert_eq!(typed("2+3=~7").display(), "7");
    // Typing goes on after the minus, as 1.3's did.
    assert_eq!(typed("5~4").display(), "-54");
}

#[test]
fn percent_divides_by_a_hundred() {
    assert_eq!(typed("50%").display(), "0.5");
    assert_eq!(typed("5%").display(), "0.05");
    assert_eq!(typed("200+50%=").display(), "200.5");
    // The result is a finished number: the next digit starts afresh.
    assert_eq!(typed("50%7").display(), "7");
}

#[test]
fn back_takes_off_the_last_character() {
    assert_eq!(typed("123<").display(), "12");
    assert_eq!(typed("12<<").display(), "0");
    assert_eq!(typed("1<").display(), "0");
    assert_eq!(typed("0<").display(), "0");
    assert_eq!(typed("5~<").display(), "0");
    assert_eq!(typed("25~<").display(), "-2");
    assert_eq!(typed("1.5<").display(), "1.");
    // Nothing to take back from a result or at the start of a number.
    assert_eq!(typed("12+7=<").display(), "19");
    assert_eq!(typed("12+<").display(), "12");
}

#[test]
fn equals_with_no_operator_changes_nothing() {
    assert_eq!(typed("5=").display(), "5");
    assert_eq!(typed("=").display(), "0");
    // And the next digit starts afresh.
    assert_eq!(typed("5=6").display(), "6");
    // With an operator but no second number, the number is used twice, as 1.3 did.
    assert_eq!(typed("5+=").display(), "10");
}

#[test]
fn a_result_too_long_for_the_display_is_cut_as_in_1_3() {
    // 12345678901234.5678 is 12345678901234.568359375 as a float: 24 characters in all, one
    // more than 1.3's 24-byte display buffer (with its end mark) held.
    let full = format(12345678901234.0 + 0.5678);
    assert_eq!(full, "12345678901234.568359375");
    let mut pad = typed("12345678901234+0.5678=");
    assert_eq!(pad.display(), "12345678901234.56835937");
    assert_eq!(pad.display().len(), DISPLAY_MAX);
    keys(&mut pad, "c");
    assert_eq!(pad.display(), "0");
}

#[test]
fn keys_from_characters() {
    assert_eq!(Key::from_char('7'), Some(Key::Digit(7)));
    assert_eq!(Key::from_char('.'), Some(Key::Point));
    assert_eq!(Key::from_char('*'), Some(Key::Op(Op::Multiply)));
    assert_eq!(Key::from_char('x'), Some(Key::Op(Op::Multiply)));
    assert_eq!(Key::from_char('/'), Some(Key::Op(Op::Divide)));
    assert_eq!(Key::from_char('-'), Some(Key::Op(Op::Subtract)));
    assert_eq!(Key::from_char('%'), Some(Key::Percent));
    assert_eq!(Key::from_char('='), Some(Key::Equals));
    assert_eq!(Key::from_char('c'), Some(Key::Clear));
    assert_eq!(Key::from_char('C'), Some(Key::Clear));
    assert_eq!(Key::from_char('\u{d7}'), Some(Key::Op(Op::Multiply)));
    assert_eq!(Key::from_char('\u{f7}'), Some(Key::Op(Op::Divide)));
    assert_eq!(Key::from_char('\u{2212}'), Some(Key::Op(Op::Subtract)));
    assert_eq!(Key::from_char('a'), None);
    assert_eq!(Key::from_char(' '), None);
    assert_eq!(Key::from_char('('), None);
}

#[test]
fn pasting_a_number() {
    let mut pad = typed("5+");
    assert!(pad.paste("12.5"));
    assert_eq!(pad.display(), "12.5");
    keys(&mut pad, "=");
    assert_eq!(pad.display(), "17.5");
    // It is typed in: more digits follow, and Back works.
    let mut pad = Keypad::new();
    assert!(pad.paste("  42 \n"));
    keys(&mut pad, "7");
    assert_eq!(pad.display(), "427");
    keys(&mut pad, "<");
    assert_eq!(pad.display(), "42");
    // Tidied as typing would have: no leading zeros, no -0.
    for (text, shown) in [
        (".5", "0.5"),
        ("007", "7"),
        ("0", "0"),
        ("-0", "0"),
        ("-0.0", "0.0"),
        ("-3", "-3"),
        ("5.", "5."),
    ] {
        let mut pad = Keypad::new();
        assert!(pad.paste(text), "{text:?}");
        assert_eq!(pad.display(), shown, "{text:?}");
    }
}

#[test]
fn pasting_what_is_not_a_number_does_nothing() {
    for text in [
        "",
        " ",
        "abc",
        "1,5",
        "1.2.3",
        ".",
        "-",
        "--1",
        "+5",
        "1e5",
        "5 5",
        "Error",
        "1234567890123456",
        "(1)",
    ] {
        let mut pad = typed("12+3");
        let before = pad.clone();
        assert!(!pad.paste(text), "{text:?}");
        assert_eq!(pad, before, "{text:?}");
    }
    // Fifteen characters are the most that can be typed, so the most that can be pasted.
    assert!(Keypad::new().paste("123456789012345"));
    assert!(Keypad::new().paste("-12345678901234"));
    assert!(!Keypad::new().paste("-123456789012345"));
}
