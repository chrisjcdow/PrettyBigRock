#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2026 Christopher Cameron-Dow
#
# Boots every configuration again and again on your own machine, to catch problems that only
# show up now and then. The same thing as the Stress workflow on GitHub, with one advantage:
# when a boot fails here, everything needed to find out why stays on disk (the serial logs,
# every core's registers and stack, and the very kernel that was running).
#
#   tools/stress.sh [RUNS]          (20 runs if you don't say; each run is four boots)
#
# Needs Linux (the test runner talks to QEMU through Unix sockets): a Linux machine, or WSL 2
# on Windows. Set it up once as the Stress workflow does:
#   sudo apt-get install qemu-system-x86 qemu-system-arm ovmf qemu-efi-aarch64 dosfstools mtools
#   and Rust through rustup (https://rustup.rs); the right version installs itself.
# PBR_QEMU_ONLY picks configurations, as for `cargo xtask qemu-test` (PBR_QEMU_ONLY=ARM64).
#
# It stops at the first failure and copies what's needed into stress-failures/<date and time>/.
# Don't run two at once in the same folder.

set -u
runs="${1:-20}"
cd "$(dirname "$0")/.." || exit 1

echo "Building once..."
cargo xtask build || exit 1

for i in $(seq 1 "$runs"); do
    echo "=== Run $i of $runs ($(date '+%H:%M:%S')) ==="
    if ! cargo xtask qemu-test >build/stress-last-run.txt 2>&1; then
        keep="stress-failures/$(date '+%Y-%m-%d-%H%M%S')"
        mkdir -p "$keep"
        cp build/stress-last-run.txt "$keep/runner-output.txt"
        cp build/qemu/serial-* build/qemu/desktop-* "$keep/" 2>/dev/null
        for arch in x86_64 aarch64; do
            [ -f "build/$arch/debug/kernel.elf" ] && cp "build/$arch/debug/kernel.elf" "$keep/kernel-$arch.elf"
        done
        git rev-parse HEAD >"$keep/commit.txt" 2>/dev/null
        echo "Run $i FAILED. Everything is in $keep/ (start with runner-output.txt)."
        tail -n 5 "$keep/runner-output.txt"
        exit 1
    fi
    grep -E '^boot test|^ok$' build/stress-last-run.txt | paste - - | sed 's/ \.\.\..*\t/: /'
done
echo "All $runs runs passed."
