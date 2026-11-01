// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Crashing a driver on purpose (docs/design/0012-driver-restart-and-m4-tests.md, section 6).
/// A driver serves this on its own endpoint, beside its class interface, only when it was
/// started with `selftest`. The operation number is far from the class interfaces' so that
/// the two don't meet.
interface pbr.driver.debug version 1;

/// The ways a process ends.
enum CrashKind {
    /// Calls `process_exit`.
    Exit @1,
    /// Writes to address 0.
    NullWrite @2,
    /// Executes an undefined instruction.
    IllegalInstruction @3,
    /// Divides by zero (on ARM, where that doesn't fault, executes a breakpoint instead).
    DivideByZero @4,
}

/// Crashes the driver. It never answers: the caller sees `PEER_GONE`.
@100 fn crash(kind: CrashKind) -> ();
