// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Finding devices (docs/design/0011-device-manager-and-drivers.md). Served by the device
/// manager.
interface pbr.devices version 1;

/// The class of interface a device's driver offers.
enum DeviceClass {
    /// `pbr.block`
    Block @1,
    /// `pbr.input`
    Input @2,
    /// `pbr.serial`
    Serial @3,
    /// `pbr.display`
    Display @4,
}

/// Where a device's driver is.
enum DeviceState {
    /// Being started for the first time.
    Starting @1,
    /// Serving.
    Running @2,
    /// Crashed or stopped, and being started again.
    Restarting @3,
    /// Failed too often (or could not be started): left alone.
    Disabled @4,
}

/// A device that has a driver.
struct DeviceInfo {
    /// The number to give to `open`. Stable until the device manager itself restarts.
    id: u32,
    /// Which interface its endpoint speaks.
    kind: DeviceClass,
    /// Where its driver is.
    state: DeviceState,
    /// The driver's name, for people.
    name: string<48>,
    /// How many times the driver has been restarted.
    restarts: u32,
}

/// The devices that have a driver, or had one.
@1 fn list() -> (devices: list<DeviceInfo, 64>) error DevicesError;

/// An endpoint for the driver of the device with this id, to be used through the interface for
/// its class. `NotReady` while the driver is starting or restarting: ask again.
@2 fn open(id: u32) -> (service: handle<any>) error DevicesError;

/// What can go wrong.
flexible enum DevicesError {
    /// There is no device with that id.
    NoSuchDevice @1,
    /// The driver is starting or restarting: ask again shortly.
    NotReady @2,
    /// The driver failed too often and was given up on.
    Disabled @3,
}
