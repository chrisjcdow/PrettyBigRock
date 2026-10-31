#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
"""An independent calculator of interface message layouts.

Written from docs/design/0008-messages-and-interfaces.md, sections 7 and 8, without
reference to the interface compiler, so the two can be checked against each other (as
tools/test_rockfs.py checks RockFS). It reads .pbi files and prints, for each:

    interface <name> <version>
    struct <Name> <size> <align>
    op <name> <number> <request fixed> <request buffer> <request caps> <reply fixed> <reply buffer> <reply caps>

which `cargo xtask test` compares with the compiler's `--layout` output. It assumes the file
is valid (the compiler checks that); it only works out sizes.

usage: pbi_check.py FILE.pbi...
"""

import re
import sys

INTS = {"u8": 1, "u16": 2, "u32": 4, "u64": 8, "i8": 1, "i16": 2, "i32": 4, "i64": 8}


def tokens(text):
    # Drop comments (documentation included), then split into words, numbers and symbols.
    text = re.sub(r"//[^\n]*", " ", text)
    return re.findall(r"->|[A-Za-z_][A-Za-z0-9_]*|0x[0-9A-Fa-f]+|[0-9]+|[^\sA-Za-z0-9_]", text)


class Reader:
    def __init__(self, toks):
        self.t = toks
        self.i = 0

    def peek(self):
        return self.t[self.i] if self.i < len(self.t) else None

    def take(self, want=None):
        tok = self.t[self.i]
        if want is not None and tok != want:
            raise SystemExit(f"expected {want!r}, found {tok!r}")
        self.i += 1
        return tok

    def number(self):
        return int(self.take(), 0)

    def type(self):
        """A type as a nested tuple."""
        word = self.take()
        if word in INTS:
            return ("int", INTS[word])
        if word == "bool":
            return ("int", 1)
        if word in ("string", "bytes"):
            self.take("<")
            n = self.number()
            self.take(">")
            return ("bytes", n)
        if word in ("array", "list"):
            self.take("<")
            inner = self.type()
            self.take(",")
            n = self.number()
            self.take(">")
            return (word, inner, n)
        if word == "option":
            self.take("<")
            inner = self.type()
            self.take(">")
            return ("option", inner)
        if word == "handle":
            self.take("<")
            while self.peek() != ">":
                self.take()
            self.take(">")
            return ("handle",)
        return ("named", word)

    def fields(self, close):
        out = []
        while self.peek() != close:
            name = self.take()
            self.take(":")
            out.append((name, self.type()))
            if self.peek() == ",":
                self.take(",")
        self.take(close)
        return out


def parse(text):
    r = Reader(tokens(text))
    r.take("interface")
    name = [r.take()]
    while r.peek() == ".":
        r.take(".")
        name.append(r.take())
    r.take("version")
    version = r.number()
    r.take(";")
    named = {}  # name -> ("struct", fields) | ("enum",) | ("bits", size)
    structs = []
    ops = []
    while r.peek() is not None:
        word = r.take()
        if word == "@":
            number = r.number()
            r.take("fn")
            op = r.take()
            r.take("(")
            params = r.fields(")")
            results = []
            if r.peek() == "->":
                r.take("->")
                r.take("(")
                results = r.fields(")")
            if r.peek() == "error":
                r.take("error")
                r.take()
            r.take(";")
            ops.append((op, number, params, results))
        elif word == "reserved":
            r.take("@")
            r.number()
            r.take(";")
        elif word == "struct":
            n = r.take()
            r.take("{")
            named[n] = ("struct", r.fields("}"))
            structs.append(n)
        elif word in ("enum", "flexible"):
            if word == "flexible":
                r.take("enum")
            n = r.take()
            while r.take() != "}":
                pass
            named[n] = ("enum",)
        elif word == "bits":
            n = r.take()
            r.take(":")
            size = INTS[r.take()]
            while r.take() != "}":
                pass
            named[n] = ("bits", size)
        elif word == "const":
            while r.take() != ";":
                pass
        else:
            raise SystemExit(f"unexpected {word!r}")
    return ".".join(name), version, named, structs, ops


def pad(n, to):
    return (n + to - 1) // to * to


class Layout:
    def __init__(self, named):
        self.named = named

    def size_align(self, t):
        kind = t[0]
        if kind == "int":
            return t[1], t[1]
        if kind in ("bytes", "list"):
            return 8, 4  # a reference: offset and length, two u32s
        if kind == "handle":
            return 4, 4
        if kind == "array":
            s, a = self.size_align(t[1])
            return s * t[2], a
        if kind == "option":
            s, a = self.size_align(t[1])
            # The u8 tag at the start, then the value at its alignment.
            return pad(1, a) + s, a
        d = self.named[t[1]]
        if d[0] == "enum":
            return 4, 4
        if d[0] == "bits":
            return d[1], d[1]
        return self.record(d[1])

    def record(self, fields):
        offset, align = 0, 1
        for _, t in fields:
            s, a = self.size_align(t)
            offset = pad(offset, a) + s
            align = max(align, a)
        return pad(offset, align), align

    def buffer(self, t):
        """The most bytes the type can put in the attached buffer."""
        kind = t[0]
        if kind == "bytes":
            return pad(t[1], 8)
        if kind == "list":
            elem, n = t[1], t[2]
            return pad(self.size_align(elem)[0] * n, 8) + n * self.buffer(elem)
        if kind == "array":
            return t[2] * self.buffer(t[1])
        if kind == "option":
            return self.buffer(t[1])
        if kind == "named" and self.named[t[1]][0] == "struct":
            return sum(self.buffer(ft) for _, ft in self.named[t[1]][1])
        return 0

    def message(self, fields):
        fixed = self.record(fields)[0]
        buf = sum(self.buffer(t) for _, t in fields)
        caps = sum(1 for _, t in fields if t[0] == "handle" or (t[0] == "option" and t[1][0] == "handle"))
        return fixed, buf, caps


def report(text):
    name, version, named, structs, ops = parse(text)
    lay = Layout(named)
    lines = [f"interface {name} {version}"]
    for s in structs:
        size, align = lay.record(named[s][1])
        lines.append(f"struct {s} {size} {align}")
    for op, number, params, results in ops:
        q = lay.message(params)
        r = lay.message(results)
        lines.append(f"op {op} {number} {q[0]} {q[1]} {q[2]} {r[0]} {r[1]} {r[2]}")
    return "\n".join(lines) + "\n"


def main():
    if len(sys.argv) < 2:
        raise SystemExit(__doc__)
    out = ""
    for path in sys.argv[1:]:
        with open(path, encoding="utf-8") as f:
            out += report(f.read())
    sys.stdout.write(out)


if __name__ == "__main__":
    main()
