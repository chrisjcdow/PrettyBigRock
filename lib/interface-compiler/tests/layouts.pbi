// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow
//
// Awkward layouts, compiled by the interface compiler and by tools/pbi_check.py and
// compared (cargo xtask test). Types are used before they're defined on purpose.
interface pbr.test.layouts version 3;

@1 fn small(a: u8, b: option<u8>, c: bool, d: option<u64>, e: u16) -> ();
@2 fn nested(a: Outer, b: array<Inner, 3>, c: option<Inner>) -> (x: list<Outer, 2>);
@3 fn lists(a: list<list<string<9>, 3>, 2>, b: list<option<bytes<5>>, 4>, c: i8)
    -> (d: list<array<u16, 3>, 5>, e: Wide);
@4 fn handles(a: u8, b: handle<any>, c: option<handle<pbr.test.layouts>>, d: u8)
    -> (e: option<handle<memory>>, f: Flags16, g: Flags64, h: Choice);
@65535 fn empty();
@9 fn tail(a: u64, b: u8) -> (c: array<array<u8, 3>, 5>, d: option<array<u32, 2>>);

struct Outer { i: Inner, o: option<Inner>, s: string<1>, k: Choice, }
struct Inner { x: u8, y: u32, z: u8, }
struct Wide { a: u8, b: u64, c: array<Inner, 2>, d: Empty, e: list<Inner, 1>, }
struct Empty { }
enum Choice { A @0, B @7 }
bits Flags16: u16 { X @15 }
bits Flags64: u64 { Y @63 }
