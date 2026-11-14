// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Choosing files (docs/design/0025-picker-clipboard-permissions.md, section 1). Served by the
/// session's file picker, a system app the sandbox manager starts itself. An app reaches it at
/// start-up role 1004 if its manifest asks for `picker`, through a capability whose badge
/// carries the app's number, so the picker always knows which app is asking (and hears when it
/// has gone).
///
/// Choosing is granting: the file or folder the user chooses comes back as a capability for it
/// alone, recorded by the sandbox manager as a `Chosen` grant, which the permissions viewer
/// shows and can take back.
interface pbr.picker version 1;

/// Which files to show: a title and the extensions, separated by spaces ("Pictures",
/// "bmp png"). "All files" is always offered as well.
struct Filter { title: string<64>, extensions: string<128> }

/// Where the picker starts. `Last` is where this app's last pick was.
enum Place { Last @0, Home @1, Documents @2, Pictures @3, Music @4, Videos @5, Downloads @6 }

/// The user chooses an existing file (or a folder, with `folder`). `window` and `serial` are
/// the app's window and the key or button press that asked, delivered to that window in the
/// last second (note 0022, section 4). With `write` the app may change what is chosen, and the
/// picker says so.
@1 fn open(window: u32, serial: u32, title: string<64>, start: Place, filters: list<Filter, 8>,
           folder: bool, write: bool)
        -> (path: string<1024>, node: handle<pbr.file>) error PickerError;
/// The user chooses where to save, and a name (`name` is offered first). Answers with the
/// file, read and write, made empty if it is new; for an existing one the user has already
/// said to replace it.
@2 fn save(window: u32, serial: u32, title: string<64>, start: Place, name: string<255>,
           filters: list<Filter, 8>)
        -> (path: string<1024>, node: handle<pbr.file>) error PickerError;

/// What can go wrong.
flexible enum PickerError {
    /// The user cancelled.
    Cancelled @1,
    /// Not asked from the app's window in answer to the user: `serial` isn't a press delivered
    /// to that window of that app in the last second.
    NotAllowed @2,
    /// This app already has a picker open.
    Busy @3,
    /// The sandbox manager couldn't record the grant (too many, or the app has gone), or the
    /// file couldn't be opened or made.
    Failed @4,
}
