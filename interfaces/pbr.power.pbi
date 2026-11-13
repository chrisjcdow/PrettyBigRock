// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Turning the computer off and restarting it (docs/design/0024-the-session-desktop.md,
/// section 7). Served by the power service to the users service alone (the root program's
/// lookup table gives `power` to nobody else, and the service checks the badge), which saves
/// everything first: these act at once.
interface pbr.power version 1;

/// Turns the computer off. Answers only if it can't.
@1 fn off() -> () error PowerError;

/// Restarts the computer. Answers only if it can't.
@2 fn restart() -> () error PowerError;

/// What can go wrong.
flexible enum PowerError {
    /// This computer can't be turned off (or restarted) this way: on x86-64, no `_S5_` object
    /// was found in the ACPI tables; on ARM64, no PSCI.
    NotSupported @1,
    /// Not called through the users service's capability.
    NotAllowed @2,
}
