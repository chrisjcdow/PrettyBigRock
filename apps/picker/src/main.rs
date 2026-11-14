// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The file picker as a program (docs/design/0025-picker-clipboard-permissions.md, section 1):
//! a service and a window. The session's sandbox manager starts it, and starts it again if it
//! ends.
//!
//! **The service.** Apps whose manifest asks for `picker` hold a capability to the picker's
//! endpoint, at their start-up role 1004. Each carries a *badge* (a number the kernel attaches
//! to every request made through it, note 0008), which the sandbox manager set to the app's
//! number, so the picker always knows which app is asking, and is told when every copy of it
//! has gone (the app has ended). A thread of its own receives those requests and hands them to
//! the main thread, waking it through the window connection's notification (bits 8 to 63 are
//! the app's own, note 0023, section 4), so that the window never waits for a request nor the
//! requests for the window. A request's *reply capability* is kept until the user has chosen:
//! an answer can be given long after the question (note 0008).
//!
//! **The rules** (note 0025, section 1):
//! - a request is answered only if `serial` is a key or button press delivered to that window
//!   of that app in the last second (the display server checks it): an app in the background
//!   can't throw pickers at the user;
//! - one picker at a time per app (`Busy`); other apps' wait their turn;
//! - the window is a `System` window over the app's own, which an app can't imitate.
//!
//! **What the app gets.** The picker opens (or, saving a new name, creates) what was chosen in
//! its own tree, with `Read`, and `Write` if asked or saving; hands it to the sandbox manager
//! (`pbr.sandbox.grant`), which keeps it, records the grant as `Chosen`, and answers with the
//! app's own capability, derived from it, so the permissions viewer shows it and can take it
//! back; and answers the app with that and the path.
//!
//! In test boots (`describe` among its arguments) it writes what its window shows to the log
//! as the gallery does, so that the desktop boot's runner can find its widgets.

#![cfg_attr(target_os = "prettybigrock", no_std, no_main)]

#[cfg(not(target_os = "prettybigrock"))]
fn main() {}

#[cfg(target_os = "prettybigrock")]
mod program {
    extern crate alloc;

    use alloc::collections::VecDeque;
    use alloc::format;
    use alloc::string::String;
    use alloc::sync::Arc;
    use alloc::vec::Vec;
    use core::mem;
    use granite::endpoint::{Endpoint, Event as Incoming};
    use granite::lock::Lock;
    use granite::notification::Notification;
    use granite::time;
    use pbr_browse::{Filter, PlaceKind, path};
    use pbr_files::{FileRights, Files};
    use pbr_interfaces::pbr_picker::{self as picker, PickerError, Place};
    use pbr_interfaces::pbr_sandbox::{self as sandbox, FileRights as GrantRights};
    use pbr_picker::{Ask, Choice, Outcome, Picker, Request, Start};
    use pbr_sandstone::Runtime;
    use pbr_window::{Connection, Event};

    granite::main!(main);

    /// The start-up roles: the picker's endpoint, to receive on; its `pbr.sandbox`, which may
    /// only `grant` and ask `details`.
    const ENDPOINT: u32 = 1004;
    const SANDBOX: u32 = 1000;
    /// The notification bit the receiving thread signals when it has handed something over.
    const JOBS: u64 = 1 << 8;
    /// How long the sandbox manager has to answer (a grant asks a filesystem server, for up to
    /// two seconds).
    const SANDBOX_MS: u64 = 10_000;
    /// Where it keeps each app's last folder, one `APP<tab>PATH` line each.
    const LAST: &str = "/Home/Apps/picker/last.txt";

    /// Logs a line with the program's prefix.
    macro_rules! say {
        ($($arg:tt)*) => {
            granite::println!("picker: {}", format_args!($($arg)*))
        };
    }

    /// What the receiving thread hands over.
    enum Job {
        /// A request: the app's number (its badge), its window and the press that asked, and
        /// what it asks for, with the reply to answer.
        Ask {
            app: u32,
            window: u32,
            serial: u32,
            what: String,
            start: Place,
            filters: Vec<Filter>,
            ask: Ask,
            reply: Reply,
        },
        /// Every capability with this app's badge has gone: the app has ended.
        Gone(u32),
    }

    /// The reply to an open or a save: either kind is answered the same way.
    enum Reply {
        Open(picker::open::Reply),
        Save(picker::save::Reply),
    }

    impl Reply {
        fn ok(self, path: &str, node: Endpoint) {
            let r = match self {
                Reply::Open(r) => r.ok(path, node),
                Reply::Save(r) => r.ok(path, node),
            };
            if let Err(e) = r {
                say!("an answer failed: {e}");
            }
        }

        fn err(self, e: PickerError) {
            // (An app that has gone can't be answered, which is no fault of anyone's.)
            let _ = match self {
                Reply::Open(r) => r.err(e),
                Reply::Save(r) => r.err(e),
            };
        }
    }

    /// The jobs handed over, and how to wake the main thread.
    struct Mailbox {
        jobs: Lock<VecDeque<Job>>,
        wake: Notification,
    }

    impl Mailbox {
        fn post(&self, job: Job) {
            self.jobs.lock().push_back(job);
            let _ = self.wake.signal(JOBS);
        }
    }

    /// `pbr.picker`, on the receiving thread: each request is handed over as it is.
    struct Receiver<'a>(&'a Mailbox, u32);

    impl picker::Server for Receiver<'_> {
        fn open(
            &mut self,
            window: u32,
            serial: u32,
            title: String,
            start: Place,
            filters: Vec<picker::Filter>,
            folder: bool,
            write: bool,
            reply: picker::open::Reply,
        ) {
            self.0.post(Job::Ask {
                app: self.1,
                window,
                serial,
                what: title,
                start,
                filters: filters_of(&filters),
                ask: Ask::Open { folder, write },
                reply: Reply::Open(reply),
            });
        }

        fn save(
            &mut self,
            window: u32,
            serial: u32,
            title: String,
            start: Place,
            name: String,
            filters: Vec<picker::Filter>,
            reply: picker::save::Reply,
        ) {
            self.0.post(Job::Ask {
                app: self.1,
                window,
                serial,
                what: title,
                start,
                filters: filters_of(&filters),
                ask: Ask::Save { name },
                reply: Reply::Save(reply),
            });
        }
    }

    fn filters_of(f: &[picker::Filter]) -> Vec<Filter> {
        f.iter().map(|f| Filter::new(&f.title, &f.extensions)).collect()
    }

    /// Receives requests for ever, on its own thread.
    fn receive(endpoint: Endpoint, mail: Arc<Mailbox>) {
        let mut buffer = alloc::vec![0u8; picker::REQUEST_BUFFER];
        loop {
            match endpoint.receive(&mut buffer, time::FOREVER) {
                Ok(Incoming::Call(msg, reply)) => {
                    // The badge is the app's number (never 0: the sandbox manager numbers
                    // apps from 1).
                    let app = msg.badge as u32;
                    if let Some((_, r)) =
                        picker::dispatch(&mut Receiver(&mail, app), msg, &buffer, reply)
                    {
                        granite::interface::unknown(r);
                    }
                }
                Ok(Incoming::Gone(badge)) => mail.post(Job::Gone(badge as u32)),
                Ok(_) => {}
                Err(e) => {
                    say!("receive failed: {e}");
                    time::sleep_ms(10);
                }
            }
        }
    }

    /// A request waiting its turn, its serial already checked.
    struct Waiting {
        app: u32,
        window: u32,
        what: String,
        start: Place,
        filters: Vec<Filter>,
        ask: Ask,
        reply: Reply,
    }

    /// The picker open now.
    struct Open {
        app: u32,
        /// The app's name, for remembering its last folder.
        name: String,
        rt: Runtime<Picker<Files>>,
        reply: Reply,
    }

    struct State {
        files: Files,
        sandbox: sandbox::Client,
        waiting: VecDeque<Waiting>,
        open: Option<Open>,
        /// Each app's last folder, by the app's name.
        last: Vec<(String, String)>,
        describe: bool,
        /// What was last written to the log, so that only changes are.
        described: Vec<String>,
        views: u32,
    }

    fn main(mut start: granite::Start) -> i32 {
        let files = Files::from_start(&mut start);
        let describe = start.args().split_whitespace().any(|a| a == "describe");
        let (Some(endpoint), Some(sb)) = (start.take(ENDPOINT), start.take(SANDBOX)) else {
            say!("started without its endpoint or pbr.sandbox");
            return 1;
        };
        let mut sb = sandbox::Client::new(Endpoint(sb));
        sb.set_timeout_ms(SANDBOX_MS);
        let mut conn = match Connection::from_start(&mut start) {
            Ok(c) => c,
            Err(e) => {
                say!("can't reach the display server: {e:?}");
                return 2;
            }
        };
        let Ok(wake) = conn.notifier() else {
            say!("no notification to be woken through");
            return 3;
        };
        let mail = Arc::new(Mailbox { jobs: Lock::new(VecDeque::new()), wake });
        let theirs = mail.clone();
        if granite::thread::spawn(move || receive(Endpoint(endpoint), theirs)).is_err() {
            say!("can't start its receiving thread");
            return 4;
        }
        let mut s = State {
            last: read_last(&files),
            files,
            sandbox: sb,
            waiting: VecDeque::new(),
            open: None,
            describe,
            described: Vec::new(),
            views: 0,
        };
        say!("ready.");
        loop {
            let Some(event) = conn.next_event(time::FOREVER) else { continue };
            match event {
                Event::Notified(bits) if bits & JOBS != 0 => {
                    let jobs: Vec<Job> = mail.jobs.lock().drain(..).collect();
                    for job in jobs {
                        s.take(&mut conn, job);
                    }
                }
                Event::Disconnected => {
                    say!("the display server has gone.");
                    return 0;
                }
                event => {
                    if let Some(o) = &mut s.open {
                        o.rt.handle(&mut conn, event);
                        if o.rt.finished() {
                            s.finish(&mut conn);
                        }
                    }
                }
            }
            if s.open.is_none() {
                s.next(&mut conn);
            }
            s.log_view();
        }
    }

    impl State {
        /// A job from the receiving thread.
        fn take(&mut self, conn: &mut Connection, job: Job) {
            match job {
                Job::Gone(app) => {
                    // The app has ended: its waiting requests go, and its open picker closes.
                    self.waiting.retain(|w| w.app != app);
                    if self.open.as_ref().is_some_and(|o| o.app == app) {
                        say!("app {app} has gone; its picker closes.");
                        if let Some(o) = self.open.take() {
                            self.close(conn, o);
                        }
                    }
                }
                Job::Ask { app, window, serial, what, start, filters, ask, reply } => {
                    let kind = if matches!(ask, Ask::Save { .. }) { "save" } else { "open" };
                    if self.open.as_ref().is_some_and(|o| o.app == app)
                        || self.waiting.iter().any(|w| w.app == app)
                    {
                        say!("app {app} asked to {kind} while its picker is open: busy.");
                        return reply.err(PickerError::Busy);
                    }
                    // Checked at once, while the press is less than a second old.
                    if !conn.check_serial(app, window, serial).unwrap_or(false) {
                        say!("app {app} asked to {kind} without a press in its window: refused.");
                        return reply.err(PickerError::NotAllowed);
                    }
                    say!("app {app} asks to {kind}.");
                    self.waiting.push_back(Waiting {
                        app,
                        window,
                        what,
                        start,
                        filters,
                        ask,
                        reply,
                    });
                }
            }
        }

        /// Opens the window for the next request waiting, if any.
        fn next(&mut self, conn: &mut Connection) {
            while let Some(w) = self.waiting.pop_front() {
                // The app's title, from the sandbox manager: what the window says is asking.
                let details = match self.sandbox.details(w.app) {
                    Ok(d) => d,
                    Err(e) => {
                        say!("app {} isn't known to the sandbox manager: {e}", w.app);
                        w.reply.err(PickerError::Failed);
                        continue;
                    }
                };
                let last =
                    self.last.iter().find(|(a, _)| *a == details.app).map(|(_, p)| p.clone());
                let request = Request {
                    app: details.title,
                    what: w.what,
                    ask: w.ask,
                    start: start_of(w.start),
                    filters: w.filters,
                    last,
                    offset_minutes: 0,
                };
                // The tree is lent to the window while it is open (`mem::replace` swaps an
                // empty one in), and taken back when it closes.
                let tree = mem::replace(&mut self.files, Files::empty());
                let mut rt = Runtime::new(
                    Picker::new(tree, request),
                    pbr_picker::settings((w.app, w.window)),
                );
                if !rt.open(conn) {
                    say!("the display server refused a window over app {}'s.", w.app);
                    self.files = mem::replace(rt.app_mut().tree_mut(), Files::empty());
                    w.reply.err(PickerError::Failed);
                    continue;
                }
                say!("open for app {} ({}).", w.app, details.app);
                self.open = Some(Open { app: w.app, name: details.app, rt, reply: w.reply });
                return;
            }
        }

        /// Destroys the window and takes the tree back.
        fn close(
            &mut self,
            conn: &mut Connection,
            mut o: Open,
        ) -> (Option<Outcome>, Reply, String, u32) {
            for (_, win) in o.rt.windows() {
                let _ = conn.destroy(win);
            }
            self.files = mem::replace(o.rt.app_mut().tree_mut(), Files::empty());
            (o.rt.app().outcome().cloned(), o.reply, o.name, o.app)
        }

        /// The window has closed: answers the app.
        fn finish(&mut self, conn: &mut Connection) {
            let Some(o) = self.open.take() else { return };
            let write = o.rt.app().request().write();
            let (outcome, reply, name, app) = self.close(conn, o);
            match outcome {
                Some(Outcome::Chosen(c)) => match self.give(app, &c, write) {
                    Ok(node) => {
                        say!("app {app} ({name}) was given {}.", c.path);
                        reply.ok(&c.path, node);
                        let folder = if c.folder {
                            c.path.clone()
                        } else {
                            String::from(path::parent(&c.path).unwrap_or("/Home"))
                        };
                        self.remember(&name, &folder);
                    }
                    Err(why) => {
                        say!("app {app} ({name}) couldn't be given {}: {why}", c.path);
                        reply.err(PickerError::Failed);
                    }
                },
                _ => {
                    say!("app {app} ({name}): cancelled.");
                    reply.err(PickerError::Cancelled);
                }
            }
        }

        /// Opens (or makes) what was chosen and has the sandbox manager grant it: the app's
        /// capability for it.
        fn give(&mut self, app: u32, c: &Choice, write: bool) -> Result<Endpoint, String> {
            let mut rights = FileRights::READ.0;
            if write {
                rights |= FileRights::WRITE.0;
            }
            let opened = if c.new {
                self.files.create(&c.path, false, FileRights(rights))
            } else {
                self.files.open(&c.path, FileRights(rights))
            }
            .map_err(pbr_files::describe)?;
            match self.sandbox.grant(
                app,
                &c.path,
                Endpoint(opened.into_handle()),
                GrantRights(rights),
            ) {
                Ok(granted) => Ok(granted),
                Err(e) => {
                    // A file made for this save that the app can't have is taken away again.
                    if c.new {
                        let _ = self.files.remove(&c.path);
                    }
                    Err(format!("the sandbox manager said {e}"))
                }
            }
        }

        /// Remembers `app`'s last folder, in memory and in the picker's own folder.
        fn remember(&mut self, app: &str, folder: &str) {
            match self.last.iter_mut().find(|(a, _)| a == app) {
                Some(entry) => entry.1 = String::from(folder),
                None => self.last.push((String::from(app), String::from(folder))),
            }
            let text: String = self.last.iter().map(|(a, p)| format!("{a}\t{p}\n")).collect();
            let rw = FileRights(FileRights::READ.0 | FileRights::WRITE.0);
            let saved = match self.files.open(LAST, rw) {
                Ok(f) => f.save(text.as_bytes()),
                Err(_) => self.files.create(LAST, false, rw).and_then(|f| f.save(text.as_bytes())),
            };
            if let Err(e) = saved {
                say!("can't keep the last folders: {}", pbr_files::describe(e));
            }
        }

        /// In test boots: what the window shows, when it has changed.
        fn log_view(&mut self) {
            if !self.describe {
                return;
            }
            let mut lines = Vec::new();
            if let Some(o) = &self.open {
                // What the lists hold: their descriptions say only which row is picked.
                let p = o.rt.app();
                lines.push(format!("places {}", p.place_titles().join("|")));
                lines.push(format!("folder {}: {}", p.folder(), p.names().join("|")));
                for (id, win) in o.rt.windows() {
                    lines.push(format!("window {} is {}", id.0, win.number()));
                    for d in o.rt.describe(id) {
                        lines.push(format!("widget {} {}", id.0, d.line()));
                    }
                }
            }
            if lines != self.described {
                self.views += 1;
                say!("view {}", self.views);
                for l in &lines {
                    say!("{l}");
                }
                say!("view {} done", self.views);
                self.described = lines;
            }
        }
    }

    /// `pbr.picker`'s place as the window's.
    fn start_of(p: Place) -> Start {
        match p {
            Place::Last => Start::Last,
            Place::Home => Start::Place(PlaceKind::Home),
            Place::Documents => Start::Place(PlaceKind::Documents),
            Place::Pictures => Start::Place(PlaceKind::Pictures),
            Place::Music => Start::Place(PlaceKind::Music),
            Place::Videos => Start::Place(PlaceKind::Videos),
            Place::Downloads => Start::Place(PlaceKind::Downloads),
        }
    }

    /// Each app's last folder, as kept in [`LAST`].
    fn read_last(files: &Files) -> Vec<(String, String)> {
        let Ok(text) = files.open(LAST, FileRights::READ).and_then(|f| f.read_all()) else {
            return Vec::new();
        };
        let text = String::from_utf8_lossy(&text);
        text.lines()
            .filter_map(|l| l.split_once('\t'))
            .filter(|(_, p)| path::tidy(p).is_some())
            .map(|(a, p)| (String::from(a), String::from(p)))
            .take(64)
            .collect()
    }
}
