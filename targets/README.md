<!-- SPDX-License-Identifier: MPL-2.0 -->
# Rust targets for PrettyBigRock programs

`x86_64-unknown-prettybigrock.json` and `aarch64-unknown-prettybigrock.json` describe how
programs for PrettyBigRock are compiled: hardware floating point, position-independent code,
linked as static position-independent executables, with `target_os = "prettybigrock"`. See
[docs/design/0009-programs-and-granite.md](../docs/design/0009-programs-and-granite.md),
section 5.

Building with a target file needs two unstable Cargo features (`-Zjson-target-spec` and
`-Zbuild-std`); the build tool sets them up. The plan is to add PrettyBigRock as an official
Rust target with the standard library work in milestone M9.

These files are under the Mozilla Public License 2.0, like `granite`, since every program is
built with them.
