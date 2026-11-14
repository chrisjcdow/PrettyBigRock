// SPDX-License-Identifier: MPL-2.0
// Copyright (C) 2026 Christopher Cameron-Dow

//! Asking the file picker for a file (docs/design/0025-picker-clipboard-permissions.md, section
//! 1; note 0023, Changes).
//!
//! A sandboxed app can't see the user's folders: its own tree has only what it was given. To
//! open or save anywhere else it asks the **file picker**, a system app that shows the user
//! every folder in a window of its own over the app's. What the user chooses comes back as a
//! *capability* (the right to use that one file, held by the kernel for the app; note 0006),
//! which [`FilePicker::mount`] puts in the app's tree at the same path the user saw. Choosing is
//! granting: the app gets exactly what the user picked.
//!
//! The picker answers only when the user has chosen, which may be minutes later, so the call
//! runs on a helper thread of the app's: the app's event loop goes on drawing and answering the
//! display server meanwhile. When the answer comes, the thread signals one of the event loop's
//! own notification bits (8 to 63, note 0023, section 4), the app's [`crate::App::notified`]
//! hears of it, and [`FilePicker::take`] gives the answer.
//!
//! ```ignore
//! // At start: the picker at start-up role 1004, answering through bit 9.
//! let picker = FilePicker::from_start(&mut start, conn.notifier()?, 9);
//! // In `update`, for Ctrl+O (a key press, so `cx` has its serial):
//! self.picker.open(cx.display_window(), cx.serial(), &Ask::open("open a picture"));
//! // `notified(bits)` with bit 9: `self.picker.take()` is the answer.
//! ```

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::handle::Handle;
use granite::interface::CallError;
use granite::lock::Lock;
use granite::notification::Notification;
pub use pbr_interfaces::pbr_picker::PickerError;
use pbr_interfaces::pbr_picker::{self as picker, Filter, Place};
use pbr_window::Win;

/// The start-up role apps find the picker at (note 0024, section 10).
pub const ROLE: u32 = 1004;

/// What to ask the picker for.
#[derive(Clone, Debug)]
pub struct Ask {
    /// The app's own words for what it wants, after "Notepad wants to": "open a picture".
    /// Empty for the picker's usual words.
    pub what: String,
    /// Where the picker starts.
    pub start: Place,
    /// Which files to show: (title, extensions separated by spaces), at most 8. "All files" is
    /// added by the picker.
    pub filters: Vec<(String, String)>,
    /// What it is for.
    pub kind: AskKind,
}

/// Opening or saving.
#[derive(Clone, Debug)]
pub enum AskKind {
    /// An existing file (or a folder), to read, and to change too with `write`.
    Open {
        /// A folder, not a file.
        folder: bool,
        /// The app may change what is chosen (the picker says so).
        write: bool,
    },
    /// Where to save, and a name, offered first.
    Save {
        /// The name offered.
        name: String,
    },
}

impl Ask {
    /// A file to open, read only, starting where this app last picked.
    pub fn open(what: &str) -> Ask {
        Ask {
            what: String::from(what),
            start: Place::Last,
            filters: Vec::new(),
            kind: AskKind::Open { folder: false, write: false },
        }
    }

    /// Where to save, offering `name`, starting where this app last picked.
    pub fn save(what: &str, name: &str) -> Ask {
        Ask {
            what: String::from(what),
            start: Place::Last,
            filters: Vec::new(),
            kind: AskKind::Save { name: String::from(name) },
        }
    }

    /// With a filter ("Text", "txt md").
    pub fn filter(mut self, title: &str, extensions: &str) -> Ask {
        self.filters.push((String::from(title), String::from(extensions)));
        self
    }

    /// Starting at `place`.
    pub fn start(mut self, place: Place) -> Ask {
        self.start = place;
        self
    }

    /// To change what is opened, too.
    pub fn write(mut self) -> Ask {
        if let AskKind::Open { write, .. } = &mut self.kind {
            *write = true;
        }
        self
    }
}

/// What the user chose: where it is (the path the user saw, which is also where
/// [`FilePicker::mount`] puts it in the app's tree), and the capability for it.
#[derive(Debug)]
pub struct Chosen {
    /// Its path.
    pub path: String,
    /// The capability: a `pbr.file` capability for it alone.
    pub node: Handle,
}

/// Why there is no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickError {
    /// The picker said no (the user cancelled; the press was too old; one is open already).
    Picker(PickerError),
    /// The call couldn't be made (the picker has gone).
    Call(granite::Error),
    /// A picker is already open for this app ([`FilePicker::open`] while one runs).
    Busy,
    /// The app has no picker (its manifest doesn't ask for `picker`), or no window yet.
    NoPicker,
    /// A helper thread couldn't be started.
    NoThread,
}

/// The answer a helper thread hands back, with the capability to the picker, which it took
/// with it.
type Answer = (Endpoint, Result<Chosen, PickError>);

/// The app's way to the file picker.
pub struct FilePicker {
    /// The picker's endpoint, while no call is under way (the helper thread has it then).
    endpoint: Option<Endpoint>,
    /// Signalled with `bit` when an answer is ready.
    notifier: Notification,
    bit: u64,
    answer: Arc<Lock<Option<Answer>>>,
}

impl FilePicker {
    /// The picker this app was given (start-up role 1004), answering through bit `bit` (8 to
    /// 63) of `notifier`: the event loop's own (`Connection::notifier`). `None` if the app has
    /// no picker or the bit isn't the app's to use.
    #[cfg(target_os = "prettybigrock")]
    pub fn from_start(
        start: &mut granite::Start,
        notifier: Notification,
        bit: u32,
    ) -> Option<FilePicker> {
        let endpoint = Endpoint(start.take(ROLE)?);
        FilePicker::new(endpoint, notifier, bit)
    }

    /// The same, with the picker's capability already taken.
    pub fn new(endpoint: Endpoint, notifier: Notification, bit: u32) -> Option<FilePicker> {
        if !(8..64).contains(&bit) {
            return None;
        }
        Some(FilePicker {
            endpoint: Some(endpoint),
            notifier,
            bit: 1 << bit,
            answer: Arc::new(Lock::new(None)),
        })
    }

    /// The notification bit answers come through: in [`crate::App::notified`], `bits &
    /// picker.bit() != 0` means [`FilePicker::take`] has an answer.
    pub fn bit(&self) -> u64 {
        self.bit
    }

    /// Whether a picker is open for this app now.
    pub fn busy(&self) -> bool {
        self.endpoint.is_none()
    }

    /// Asks the picker, from the app's window `window` in answer to the press `serial` (from
    /// [`crate::Context::display_window`] and [`crate::Context::serial`]): the picker opens
    /// only for a press in the last second. Returns at once; the answer comes later.
    pub fn open(&mut self, window: Option<Win>, serial: u32, ask: &Ask) -> Result<(), PickError> {
        let Some(window) = window else { return Err(PickError::NoPicker) };
        let Some(endpoint) = self.endpoint.take() else { return Err(PickError::Busy) };
        let ask = ask.clone();
        let (answer, notifier) = (self.answer.clone(), &self.notifier);
        let notifier = match notifier.0.duplicate(granite::sys::rights::ALL) {
            Ok(h) => Notification(h),
            Err(_) => {
                self.endpoint = Some(endpoint);
                return Err(PickError::NoThread);
            }
        };
        let bit = self.bit;
        // The thread owns what it uses (`move`): the picker's capability, a copy of the
        // notification, and the shared place for the answer. `Arc` is a pointer shared between
        // threads, counted so that what it points to lives while either has it; `Lock` lets
        // one thread at a time at it.
        let spawned = granite::thread::spawn(move || {
            let client = picker::Client::new(endpoint);
            let result = ask_picker(&client, window.number(), serial, &ask);
            *answer.lock() = Some((client.into_endpoint(), result));
            let _ = notifier.signal(bit);
        });
        match spawned {
            // The thread runs to its end by itself; nothing waits for it.
            Ok(_) => Ok(()),
            Err(_) => {
                // The endpoint went with the closure that never ran: there's no picker now.
                Err(PickError::NoThread)
            }
        }
    }

    /// The answer, once the picker's bit has been signalled (`None` before).
    pub fn take(&mut self) -> Option<Result<Chosen, PickError>> {
        let (endpoint, result) = self.answer.lock().take()?;
        self.endpoint = Some(endpoint);
        Some(result)
    }

    /// Puts what was chosen in the app's tree, at the path the user saw, so the app opens it
    /// by that path like any other file.
    #[cfg(target_os = "prettybigrock")]
    pub fn mount(files: &mut pbr_files::Files, chosen: Chosen) -> Result<String, pbr_files::Error> {
        // Picked twice: the first is there already, and the new capability can go.
        if files.entries().iter().any(|(p, _)| *p == chosen.path) {
            return Ok(chosen.path);
        }
        files.mount(&chosen.path, chosen.node)?;
        Ok(chosen.path)
    }
}

/// The call itself, on the helper thread: it waits as long as the user takes.
fn ask_picker(
    client: &picker::Client,
    window: u32,
    serial: u32,
    ask: &Ask,
) -> Result<Chosen, PickError> {
    let filters: Vec<Filter> = ask
        .filters
        .iter()
        .take(8)
        .map(|(t, e)| Filter { title: t.clone(), extensions: e.clone() })
        .collect();
    let error = |e: CallError<PickerError>| match e {
        CallError::Service(p) => PickError::Picker(p),
        CallError::Call(e) => PickError::Call(e),
        // The picker answered something this app can't read: as good as no picker.
        _ => PickError::Call(granite::Error::InvalidArgument),
    };
    match &ask.kind {
        AskKind::Open { folder, write } => client
            .open(window, serial, &ask.what, ask.start, &filters, *folder, *write)
            .map(|r| Chosen { path: r.path, node: r.node.0 })
            .map_err(error),
        AskKind::Save { name } => client
            .save(window, serial, &ask.what, ask.start, name, &filters)
            .map(|r| Chosen { path: r.path, node: r.node.0 })
            .map_err(error),
    }
}
