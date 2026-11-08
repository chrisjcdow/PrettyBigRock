// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! `shell`: the text shell (docs/design/0018-supervision-shell-and-m5-tests.md, section 6).
//!
//! An app you type commands into, on the terminal the console serves. It is *trusted*
//! (note 0017, section 4): its manifest asks for the session's whole tree, and it is given
//! the session's sandbox manager and users service, so it can launch apps, show what they
//! hold and take it back. It has no powers beyond its session's: whatever its file tree holds
//! is all it can reach.
//!
//! The program is glue. Everything that can be wrong with a line (splitting it, checking a
//! command's arguments, resolving paths, the rules for copying and removing, the wording of
//! errors, the shape of every table) is in `lib/shell` (`pbr-shell`) and tested on the host.
//! What is here is the part that needs the system: reading a line from the terminal, calling
//! the services, and printing what came back.
//!
//! What it is started with (a *start-up role* is a number a launcher gives each capability it
//! hands to a new program, so the program can find it; note 0009):
//! - its file tree: the session's tree (`pbr_files::Files::from_start`);
//! - role 1000, `pbr.sandbox` (with this app's own badge: a number the service stamps on a
//!   capability to know who is calling, note 0008); role 1001, `pbr.users`; role 1002,
//!   `pbr.terminal`.
//!
//! If the terminal is missing, or any call on it fails (the console has gone, or it is busy or
//! closed), the shell logs why and ends with 1. Its session starts it again (note 0018,
//! section 3). Calls to the other services have a time limit, so nothing waits for ever.

#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use pbr_files::{Error as FilesError, FileError, Files, READ, READ_WRITE};
use pbr_interfaces::pbr_file as file;
use pbr_interfaces::pbr_sandbox as sandbox;
use pbr_interfaces::pbr_terminal::{self as terminal, TerminalError};
use pbr_interfaces::pbr_users::{self as users, User, UserKind};
use pbr_shell::format::{self, MAX_WRITE};
use pbr_shell::{
    AppRef, Command, Failure, FileShell, Fs, Item, Reason, SandboxOp, Space, Stat, UsersOp, apps,
    file_reason, help, parse_line, sandbox_reason, users_reason,
};

granite::main!(main);

/// The start-up roles of the session's `pbr.sandbox` and `pbr.users` capabilities (the sandbox
/// manager gives trusted apps these; `services/sandbox` uses the same numbers) and of the
/// terminal (the sandbox manager hands it to apps granted `terminal`).
const ROLE_SANDBOX: u32 = 1000;
const ROLE_USERS: u32 = 1001;
const ROLE_TERMINAL: u32 = 1002;

/// How long a call to the users service or the sandbox manager may take before the shell gives
/// up on it.
const SERVICE_TIMEOUT_MS: u64 = 60_000;

/// What the shell says first.
const WELCOME: &str = "PrettyBigRock shell. Type help for the commands.\n";

/// The terminal can't be used any more. The reason has been logged; the shell should end.
struct TerminalGone;

// ---- The file tree, as the library's `Fs` ----

/// The shell's file tree, for the library to work on. (A wrapper of our own, because Rust only
/// lets a crate implement a trait for a type if the crate owns one of the two, and this crate
/// owns neither `Fs` nor `Files`.)
struct Disk(Files);

/// A `pbr_files` error in the library's words.
fn files_reason(e: FilesError) -> Reason {
    match e {
        FilesError::NotFound => Reason::NotFound,
        FilesError::BadPath => Reason::BadPath,
        FilesError::Above => Reason::Virtual,
        FilesError::File(e) => Reason::from(e),
        FilesError::Call(e) => Reason::from(e),
        FilesError::Malformed => Reason::Garbled,
    }
}

impl Disk {
    /// Opens the file at `path` for writing, making it if it isn't there.
    fn open_or_create(&self, path: &str) -> pbr_files::Result<pbr_files::File> {
        match self.0.create(path, false, READ_WRITE) {
            Err(FilesError::File(FileError::AlreadyExists)) => self.0.open(path, READ_WRITE),
            other => other,
        }
    }
}

impl Fs for Disk {
    fn stat(&self, path: &str) -> Result<Stat, Reason> {
        self.0.info(path).map(|info| Stat::from(&info)).map_err(files_reason)
    }

    fn list(&self, path: &str) -> Result<Vec<Item>, Reason> {
        let listing = self.0.list(path).map_err(files_reason)?;
        Ok(listing
            .into_iter()
            .map(|l| Item { name: l.name, folder: l.folder, size: l.size })
            .collect())
    }

    fn read_at(&self, path: &str, offset: u64, max: u32) -> Result<Vec<u8>, Reason> {
        let read = || self.0.open(path, READ)?.read_at(offset, max);
        read().map_err(files_reason)
    }

    fn replace(&self, path: &str, data: &[u8]) -> Result<(), Reason> {
        // `save` empties the file, writes, and makes it durable (note 0014, section 6).
        let save = || self.open_or_create(path)?.save(data);
        save().map_err(files_reason)
    }

    fn append(&self, path: &str, data: &[u8]) -> Result<(), Reason> {
        let append = || {
            let file = self.open_or_create(path)?;
            file.write_at(file.info()?.size, data)?;
            file.sync()
        };
        append().map_err(files_reason)
    }

    fn mkdir(&self, path: &str) -> Result<(), Reason> {
        // The new folder's own capability isn't needed; it is closed when dropped.
        self.0.create(path, true, READ).map(drop).map_err(files_reason)
    }

    fn remove(&self, path: &str) -> Result<(), Reason> {
        self.0.remove(path).map_err(files_reason)
    }

    fn rename(&self, from: &str, to: &str) -> Result<(), Reason> {
        self.0.rename(from, to).map_err(files_reason)
    }

    fn space(&self, path: &str) -> Result<Space, Reason> {
        // `pbr_files` doesn't wrap `space`, so ask the volume directly through a fresh,
        // derived capability for the folder.
        let folder = self.0.open(path, READ).map_err(files_reason)?;
        let mut client = file::Client::new(Endpoint(folder.into_handle()));
        client.set_timeout_ms(SERVICE_TIMEOUT_MS);
        client.space().map(|s| Space::from(&s)).map_err(|e| file_reason(&e))
    }
}

/// Opens `path` to give to an app: with read and write if the shell has both, with read if
/// that is all it has. Choosing a file is granting it (note 0017, section 6), and a grant can
/// never be more than the shell holds.
fn open_to_grant(files: &Files, path: &str) -> Result<pbr_files::File, Reason> {
    match files.open(path, READ_WRITE) {
        Err(FilesError::File(FileError::NoRight | FileError::ReadOnly)) => {
            files.open(path, READ).map_err(files_reason)
        }
        other => other.map_err(files_reason),
    }
}

// ---- The shell ----

struct Shell {
    terminal: terminal::Client,
    files: FileShell<Disk>,
    /// This session's user name, for the prompt.
    user: String,
    /// `None` if this shell wasn't given the capability (it always is, when started by a
    /// session's sandbox manager); commands that need it say so.
    users: Option<users::Client>,
    sandbox: Option<sandbox::Client>,
}

fn main(mut start: granite::Start) -> i32 {
    let files = Files::from_start(&mut start);
    let entries = files.entries();
    let Some(terminal) = start.take(ROLE_TERMINAL) else {
        granite::println!("shell: it was not given a terminal, so there is nowhere to type.");
        return 1;
    };
    let mut users = start.take(ROLE_USERS).map(|h| users::Client::new(Endpoint(h)));
    let mut sandbox = start.take(ROLE_SANDBOX).map(|h| sandbox::Client::new(Endpoint(h)));
    // Nothing the shell asks of these may wait for ever.
    if let Some(c) = &mut users {
        c.set_timeout_ms(SERVICE_TIMEOUT_MS);
    }
    if let Some(c) = &mut sandbox {
        c.set_timeout_ms(SERVICE_TIMEOUT_MS);
    }
    let user = match users.as_ref().map(users::Client::whoami) {
        Some(Ok(u)) => u.name,
        Some(Err(e)) => {
            granite::println!("shell: can't ask who this is: {e}");
            String::from("unknown")
        }
        None => {
            granite::println!("shell: it was not given the users service.");
            String::from("unknown")
        }
    };
    let mut shell = Shell {
        terminal: terminal::Client::new(Endpoint(terminal)),
        files: FileShell::new(Disk(files), &entries),
        user,
        users,
        sandbox,
    };
    match shell.run() {
        Ok(()) => 0,
        Err(TerminalGone) => 1,
    }
}

impl Shell {
    /// Reads and runs lines until the terminal fails.
    fn run(&mut self) -> Result<(), TerminalGone> {
        self.say(WELCOME)?;
        loop {
            let prompt = format::prompt(&self.user, self.files.cwd());
            let line =
                self.terminal.read_line(&prompt).map_err(|e| self.lost("reading a line", e))?;
            match parse_line(&line) {
                Ok(None) => {}
                Ok(Some(command)) => self.execute(command)?,
                Err(e) => self.say(&format!("{e}\n"))?,
            }
        }
    }

    /// Logs why the terminal can't be used, for the person reading the log: the user can't be
    /// told, since the terminal is what shows them anything.
    fn lost(&self, what: &str, e: CallError<TerminalError>) -> TerminalGone {
        granite::println!("shell: the terminal failed while {what}: {e}");
        TerminalGone
    }

    /// Shows `text` (which should end each line with `\n`), in pieces the terminal accepts.
    fn say(&self, text: &str) -> Result<(), TerminalGone> {
        for piece in format::pieces(text, MAX_WRITE) {
            self.terminal.write(piece).map_err(|e| self.lost("writing", e))?;
        }
        Ok(())
    }

    /// Shows the outcome of a command that has a path to blame: its text, or
    /// `<command>: <path>: <reason>`.
    fn show(&self, command: &str, outcome: Result<String, Failure>) -> Result<(), TerminalGone> {
        match outcome {
            Ok(text) => self.say(&text),
            Err(failure) => self.say(&format!("{command}: {failure}\n")),
        }
    }

    /// Shows the outcome of a command with no path to blame: its text, or `<command>: <reason>`.
    fn show_plain(
        &self,
        command: &str,
        outcome: Result<String, Reason>,
    ) -> Result<(), TerminalGone> {
        match outcome {
            Ok(text) => self.say(&text),
            Err(reason) => self.say(&format!("{command}: {reason}\n")),
        }
    }

    fn users_service(&self) -> Result<&users::Client, Reason> {
        self.users
            .as_ref()
            .ok_or_else(|| Reason::text("this shell was not given the users service"))
    }

    fn sandbox_service(&self) -> Result<&sandbox::Client, Reason> {
        self.sandbox
            .as_ref()
            .ok_or_else(|| Reason::text("this shell was not given the sandbox manager"))
    }

    fn execute(&mut self, command: Command) -> Result<(), TerminalGone> {
        let name = command.name();
        match command {
            Command::Help => self.say(&help::help()),
            Command::About => self.say(&help::about()),
            Command::Clear => self.terminal.clear().map_err(|e| self.lost("clearing", e)),
            Command::Echo(text) => self.say(&format!("{text}\n")),
            Command::Potato => self.say(&help::potato()),
            Command::Pwd => self.say(&format!("{}\n", self.files.cwd())),
            Command::Ls { all, path } => self.show(name, self.files.ls(all, path.as_deref())),
            Command::Cd(path) => {
                let outcome = self.files.cd(path.as_deref()).map(|()| String::new());
                self.show(name, outcome)
            }
            Command::Cat(path) => self.show(name, self.files.cat(&path)),
            Command::Info(path) => self.show(name, self.files.info(&path)),
            Command::Mkdir(path) => self.show(name, quiet(self.files.mkdir(&path))),
            Command::Write { file, text } => self.show(name, quiet(self.files.write(&file, &text))),
            Command::Append { file, text } => {
                self.show(name, quiet(self.files.append(&file, &text)))
            }
            Command::Cp { recursive, from, to } => {
                self.show(name, quiet(self.files.cp(recursive, &from, &to)))
            }
            Command::Mv { from, to } => self.show(name, quiet(self.files.mv(&from, &to))),
            Command::Rm { recursive, path } => {
                self.show(name, quiet(self.files.rm(recursive, &path)))
            }
            Command::Df => {
                let (text, failures) = self.files.df();
                self.say(&text)?;
                for failure in failures {
                    self.say(&format!("{name}: {failure}\n"))?;
                }
                Ok(())
            }
            Command::Volumes => self.show_plain(name, self.volumes()),
            Command::Format { volume, label } => {
                self.show_plain(name, self.format_volume(volume, &label))
            }
            Command::Setup { volume, name: user } => {
                self.show_plain(name, self.setup_volume(volume, &user))
            }
            Command::Whoami => self.say(&format!("{}\n", self.user)),
            Command::Users => self.show_plain(name, self.list_users()),
            Command::AddUser { name: user, kind, display } => {
                self.show_plain(name, self.add_user(user, kind, display))
            }
            Command::Run { app, files, args } => {
                let outcome = self.run_app(&app, &files, &args);
                self.show(name, outcome)
            }
            Command::Apps => self.show_plain(name, self.list_apps()),
            Command::Grants(app) => {
                let outcome = self.grants(&app).map_err(|r| Failure::new(app.to_string(), r));
                self.show(name, outcome)
            }
            Command::Revoke { app, path } => {
                let outcome = self.revoke(&app, &path);
                self.show(name, outcome)
            }
            Command::Stop(app) => {
                let outcome = self.stop(&app).map_err(|r| Failure::new(app.to_string(), r));
                self.show(name, outcome)
            }
            Command::Shutdown => self.shutdown(),
        }
    }

    // ---- Disks and users (pbr.users) ----

    fn volumes(&self) -> Result<String, Reason> {
        let list =
            self.users_service()?.volumes().map_err(|e| users_reason(UsersOp::Volumes, &e))?;
        Ok(format::volumes(&list))
    }

    fn format_volume(&self, volume: u32, label: &str) -> Result<String, Reason> {
        self.users_service()?
            .format(volume, label)
            .map_err(|e| users_reason(UsersOp::Format, &e))?;
        Ok(format!(
            "Volume {volume} now has RockFS on it, labelled \"{label}\". \
             Everything that was on it is gone.\n"
        ))
    }

    fn setup_volume(&self, volume: u32, user: &str) -> Result<String, Reason> {
        self.users_service()?.setup(volume, user).map_err(|e| users_reason(UsersOp::Setup, &e))?;
        Ok(format!(
            "Volume {volume} is set up as the system volume, with {user} as its first \
             administrator. It takes effect at the next start.\n"
        ))
    }

    fn list_users(&self) -> Result<String, Reason> {
        let list = self.users_service()?.list().map_err(|e| users_reason(UsersOp::List, &e))?;
        Ok(format::users(&list))
    }

    fn add_user(&self, name: String, kind: UserKind, display: String) -> Result<String, Reason> {
        let what = match kind {
            UserKind::Admin => "an administrator",
            UserKind::Standard => "a standard user",
        };
        let message = format!("Added {name} as {what}. Their session starts at the next start.\n");
        let user = User { name, kind, display };
        self.users_service()?.add(&user).map_err(|e| users_reason(UsersOp::Add, &e))?;
        Ok(message)
    }

    /// `shutdown`: saves everything, then says it is safe. PrettyBigRock can't switch the
    /// computer off yet, so the shell keeps prompting; file commands fail from now on, because
    /// the volumes are closed.
    fn shutdown(&self) -> Result<(), TerminalGone> {
        self.say("Saving everything...\n")?;
        let done = self
            .users_service()
            .and_then(|u| u.shutdown().map_err(|e| users_reason(UsersOp::Shutdown, &e)));
        match done {
            Ok(()) => self.say("It's now safe to turn off your computer.\n"),
            Err(reason) => self.say(&format!("shutdown: {reason}\n")),
        }
    }

    // ---- Apps (pbr.sandbox) ----

    /// `run APP [FILE…] [-- ARGS…]`.
    fn run_app(&self, app: &str, files: &[String], args: &str) -> Result<String, Failure> {
        let sandbox = self.sandbox_service().map_err(|r| Failure::new(app, r))?;
        let disk = &self.files.fs().0;
        // Open every file first. A launch that has been prepared and never started stays in
        // the sandbox manager until the session ends (there is no way to cancel one, and only
        // a few can wait), so what can be checked here is checked before `prepare`.
        let mut chosen: Vec<(&str, String, pbr_files::File)> = Vec::new();
        for typed in files {
            let path = self.files.absolute(typed)?;
            let path = apps::sandbox_path(path).map_err(|r| Failure::new(typed.as_str(), r))?;
            // The same file named twice is one grant.
            if chosen.iter().any(|(_, p, _)| *p == path) {
                continue;
            }
            let file = open_to_grant(disk, &path).map_err(|r| Failure::new(typed.as_str(), r))?;
            chosen.push((typed.as_str(), path, file));
        }
        let run = |e: &CallError<sandbox::SandboxError>| sandbox_reason(SandboxOp::Run, e);
        let launch = sandbox.prepare(app, args).map_err(|e| Failure::new(app, run(&e)))?;
        for (typed, path, file) in chosen {
            // Handing the capability over is the grant. Our copy goes with the call.
            let node = Endpoint(file.into_handle());
            sandbox.choose(launch, &path, node).map_err(|e| Failure::new(typed, run(&e)))?;
        }
        let started = sandbox.start(launch).map_err(|e| Failure::new(app, run(&e)))?;
        // The sandbox manager keeps the app tied to itself; the process capability is only for
        // watching it, which `apps` does.
        drop(started.process);
        Ok(format!("Started {app} as app {}.\n", started.app))
    }

    /// `apps`.
    fn list_apps(&self) -> Result<String, Reason> {
        let list =
            self.sandbox_service()?.list().map_err(|e| sandbox_reason(SandboxOp::List, &e))?;
        Ok(format::apps(&list))
    }

    /// The number of the app `app` means.
    fn app_id(&self, op: SandboxOp, app: &AppRef) -> Result<u32, Reason> {
        match app {
            AppRef::Id(id) => Ok(*id),
            AppRef::Name(_) => {
                let list = self.sandbox_service()?.list().map_err(|e| sandbox_reason(op, &e))?;
                apps::resolve(&list, app)
            }
        }
    }

    /// `grants APP`: every page of what the app holds, one line each.
    fn grants(&self, app: &AppRef) -> Result<String, Reason> {
        let id = self.app_id(SandboxOp::Grants, app)?;
        let sandbox = self.sandbox_service()?;
        let mut out = String::new();
        let mut from = 0;
        loop {
            let page =
                sandbox.grants(id, from).map_err(|e| sandbox_reason(SandboxOp::Grants, &e))?;
            for grant in &page.grants {
                out.push_str(&format::grant(grant));
            }
            // 0 means that was all. A `next` that doesn't move forward would loop for ever.
            if page.next == 0 || page.next <= from {
                return Ok(out);
            }
            from = page.next;
        }
    }

    /// `revoke APP PATH`: takes the path (and everything the app opened through it) back.
    fn revoke(&self, app: &AppRef, typed: &str) -> Result<String, Failure> {
        let path = self.files.absolute(typed)?;
        let go = || -> Result<(), Reason> {
            let path = apps::sandbox_path(path.clone())?;
            let id = self.app_id(SandboxOp::Revoke, app)?;
            self.sandbox_service()?
                .revoke(id, &path)
                .map_err(|e| sandbox_reason(SandboxOp::Revoke, &e))
        };
        go().map_err(|r| Failure::new(typed, r))?;
        Ok(format!("Took {path} back from {app}.\n"))
    }

    /// `stop APP`.
    fn stop(&self, app: &AppRef) -> Result<String, Reason> {
        let id = self.app_id(SandboxOp::Stop, app)?;
        self.sandbox_service()?.stop(id).map_err(|e| sandbox_reason(SandboxOp::Stop, &e))?;
        Ok(format!("Stopped {app}.\n"))
    }
}

/// A command that prints nothing when it works.
fn quiet(outcome: Result<(), Failure>) -> Result<String, Failure> {
    outcome.map(|()| String::new())
}
