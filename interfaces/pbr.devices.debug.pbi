// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Testing the device manager (docs/design/0012-driver-restart-and-m4-tests.md, section 6).
/// Served on its public endpoint only when it was started with `selftest`. The operation
/// numbers are far from `pbr.devices`' so that the two don't meet.
interface pbr.devices.debug version 1;

/// Stops a device's driver from outside, through the device manager's STOP right.
@100 fn stop(id: u32) -> () error DebugError;

/// Forgets a device's recent crashes, so that a test may crash it again and again without
/// reaching the limit (note 0012, section 4).
@101 fn forgive(id: u32) -> () error DebugError;

flexible enum DebugError {
    /// There is no such device, or it has no driver running.
    NoSuchDevice @1,
}
