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

/// Signals `bits` on `note` each time the power button is pressed (docs/design/0028-acpi.md,
/// section 7). Replaces any earlier watch. `NotSupported` where the button isn't seen (ARM64,
/// or a PC whose button is handled by the firmware's methods).
@3 fn watch_button(note: handle<notification>, bits: u64) -> () error PowerError;

/// What can go wrong.
flexible enum PowerError {
    /// This computer can't be turned off (or restarted, or its button watched) this way: on
    /// x86-64, the ACPI service couldn't; on ARM64, no PSCI.
    NotSupported @1,
    /// Not called through the users service's capability.
    NotAllowed @2,
}
