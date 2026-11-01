// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// What a driver says to its device manager (docs/design/0011-device-manager-and-drivers.md).
interface pbr.driver.host version 1;

/// The driver has set its device up and is serving: here is the endpoint clients should use.
/// Sent again after a restart, with the new endpoint.
@1 fn ready(service: handle<any>) -> () error HostError;

/// What can go wrong.
flexible enum HostError {
    /// The driver said it was ready when it was not expected to (it was already running, or
    /// was being stopped).
    BadState @1,
}
