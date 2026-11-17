// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The shell itself, over any terminal (docs/design/0018-supervision-shell-and-m5-tests.md,
//! section 6; docs/design/0026-apps-m6-tests-and-stages.md, section 1).
//!
//! The rest of this library is plain computation, tested on the host. This module is the part
//! that needs the system: it reads a line from a [`Console`], runs the command through the
//! services the program was given, and prints what came back. It is built only for
//! PrettyBigRock, and the two programs that run it are only glue: `apps/shell` supplies the
//! console's terminal (`pbr.terminal`), `apps/terminal` a window.
//!
//! The shell is *trusted* (note 0017, section 4): its manifest asks for the session's whole
//! tree, and it is given the session's sandbox manager and users service, so it can launch apps,
//! show what they hold and take it back. It has no powers beyond its session's: whatever its
//! file tree holds is all it can reach. With `sysinfo` in its manifest it is also given
//! `pbr.sysinfo`, which answers `mem`, `fetch` and `disk` (and `about`'s version).
//!
//! What it is started with (a *start-up role* is a number a launcher gives each capability it
//! hands to a new program, so the program can find it; note 0009):
//! - its file tree: the session's tree (`pbr_files::Files::from_start`);
//! - role 1000, `pbr.sandbox` (with this app's own badge: a number the service stamps on a
//!   capability to know who is calling, note 0008); role 1001, `pbr.users`; role 1005,
//!   `pbr.sysinfo`.
//!
//! A service the program wasn't given is `None`, and a command that needs it says so. Calls to
//! the services have a time limit, so nothing waits for ever.

use crate::command::AppRef;
use crate::console::{Console, Gone};
use crate::format::{self, MAX_WRITE};
use crate::{
    Command, Failure, FileShell, Fs, Item, Reason, SandboxOp, Space, Stat, UsersOp, apps, clock,
    facts, file_reason, help, parse_line, sandbox_reason, sysinfo_reason, users_reason,
};
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use granite::endpoint::Endpoint;
use granite::interface::CallError;
use granite::time;
use pbr_files::{Error as FilesError, FileError, Files, READ, READ_WRITE};
use pbr_interfaces::pbr_file as file;
use pbr_interfaces::pbr_sandbox as sandbox;
use pbr_interfaces::pbr_sysinfo as sysinfo;
use pbr_interfaces::pbr_users::{self as users, User, UserKind};

/// The start-up roles of the session's `pbr.sandbox`, `pbr.users` and `pbr.sysinfo`
/// capabilities (the sandbox manager gives trusted apps the first two, and the third to apps
/// whose manifest asks for `sysinfo`; `services/sandbox` uses the same numbers).
const ROLE_SANDBOX: u32 = 1000;
const ROLE_USERS: u32 = 1001;
const ROLE_SYSINFO: u32 = 1005;

/// How long a call to the users service or the sandbox manager may take before the shell gives
/// up on it. (Checking a volume is one of them.)
const SERVICE_TIMEOUT_MS: u64 = 60_000;
/// And for `pbr.sysinfo`, which only reports what the system already knows.
const SYSINFO_TIMEOUT_MS: u64 = 5_000;

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

/// The shell: a terminal to talk to, the file tree, and the services (`None` for one the
/// program wasn't given).
pub struct Shell<C: Console> {
    console: C,
    files: FileShell<Disk>,
    /// This session's user name, for the prompt.
    user: String,
    /// `None` if this shell wasn't given the capability (it always is, when started by a
    /// session's sandbox manager); commands that need it say so.
    users: Option<users::Client>,
    sandbox: Option<sandbox::Client>,
    sysinfo: Option<sysinfo::Client>,
}

impl<C: Console> Shell<C> {
    /// A shell over `console`, with `files` (the program's tree, `Files::from_start`) and the
    /// services in `start`'s roles. Asks the users service who this is, for the prompt.
    pub fn from_start(start: &mut granite::Start, files: Files, console: C) -> Shell<C> {
        let entries = files.entries();
        let mut users = start.take(ROLE_USERS).map(|h| users::Client::new(Endpoint(h)));
        let mut sandbox = start.take(ROLE_SANDBOX).map(|h| sandbox::Client::new(Endpoint(h)));
        let mut sysinfo = start.take(ROLE_SYSINFO).map(|h| sysinfo::Client::new(Endpoint(h)));
        // Nothing the shell asks of these may wait for ever.
        if let Some(c) = &mut users {
            c.set_timeout_ms(SERVICE_TIMEOUT_MS);
        }
        if let Some(c) = &mut sandbox {
            c.set_timeout_ms(SERVICE_TIMEOUT_MS);
        }
        if let Some(c) = &mut sysinfo {
            c.set_timeout_ms(SYSINFO_TIMEOUT_MS);
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
        Shell {
            console,
            files: FileShell::new(Disk(files), &entries),
            user,
            users,
            sandbox,
            sysinfo,
        }
    }

    /// The system's name and version (`PrettyBigRock`, `2.0.0-alpha.1`), if the shell was given
    /// `pbr.sysinfo` and it answers.
    pub fn version(&self) -> Option<(String, String)> {
        let v = self.sysinfo.as_ref()?.version().ok()?;
        Some((v.name, v.version))
    }

    /// Reads and runs lines until the terminal is gone.
    pub fn run(&mut self) -> Result<(), Gone> {
        loop {
            let prompt = format::prompt(&self.user, self.files.cwd());
            let line = self.console.read_line(&prompt)?;
            match parse_line(&line) {
                Ok(None) => {}
                Ok(Some(command)) => self.execute(command)?,
                Err(e) => self.say(&format!("{e}\n"))?,
            }
        }
    }

    /// Shows `text` (which should end each line with `\n`), in pieces the terminal accepts.
    pub fn say(&self, text: &str) -> Result<(), Gone> {
        for piece in format::pieces(text, MAX_WRITE) {
            self.console.write(piece)?;
        }
        Ok(())
    }

    /// Shows the outcome of a command that has a path to blame: its text, or
    /// `<command>: <path>: <reason>`.
    fn show(&self, command: &str, outcome: Result<String, Failure>) -> Result<(), Gone> {
        match outcome {
            Ok(text) => self.say(&text),
            Err(failure) => self.say(&format!("{command}: {failure}\n")),
        }
    }

    /// Shows the outcome of a command with no path to blame: its text, or `<command>: <reason>`.
    fn show_plain(&self, command: &str, outcome: Result<String, Reason>) -> Result<(), Gone> {
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

    fn sysinfo_service(&self) -> Result<&sysinfo::Client, Reason> {
        self.sysinfo
            .as_ref()
            .ok_or_else(|| Reason::text("this shell was not given the system information service"))
    }

    fn execute(&mut self, command: Command) -> Result<(), Gone> {
        let name = command.name();
        match command {
            Command::Help => self.say(&help::help()),
            Command::About => {
                let version = self.version();
                let version = version.as_ref().map(|(n, v)| (n.as_str(), v.as_str()));
                self.say(&help::about_with(version))
            }
            Command::Clear => self.console.clear(),
            Command::Echo(text) => self.say(&format!("{text}\n")),
            Command::Potato => self.say(&help::potato()),
            Command::Fortune => self.say(&help::fortune(time::now())),
            Command::Time => self.show_plain(name, self.time()),
            Command::Tz(offset) => self.show_plain(name, self.tz(offset)),
            Command::Uptime => self.say(&clock::uptime_line(time::now() / 1_000_000_000)),
            Command::Mem => self.show_plain(name, self.mem()),
            Command::Fetch => self.say(&self.fetch()),
            Command::Calc(expr) => self.show_plain(name, facts::calc(&expr).map_err(Reason::text)),
            Command::Disk => self.show_plain(name, self.disks()),
            Command::Check { volume, fix } => self.check(volume, fix),
            Command::Edit(file) => {
                // Notepad is given the file, and told its path as its argument: that is how an
                // app launched with a file knows which it is (note 0025, section 3).
                let outcome = match file.as_deref() {
                    Some(typed) => self
                        .files
                        .absolute(typed)
                        .and_then(|p| {
                            apps::sandbox_path(p).map_err(|r| Failure::new(typed, r))
                        })
                        .and_then(|path| self.run_app("notepad", file.as_slice(), &path)),
                    None => self.run_app("notepad", &[], ""),
                };
                self.show(name, outcome)
            }
            Command::Open(app) => {
                let outcome = self.run_app(&app, &[], "");
                self.show(name, outcome)
            }
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
            Command::Settings => self.show_plain(name, self.settings()),
            Command::Set { key, value } => self.show_plain(name, self.set(&key, &value)),
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
            Command::PowerOff => self.power(false),
            Command::Reboot => self.power(true),
        }
    }

    // ---- Time, memory, disks (pbr.sysinfo, and the time zone from pbr.users) ----

    /// The user's time zone, in minutes east of UTC: the `timezone` setting (+00:00 if there is
    /// none).
    fn timezone(&self) -> Result<i32, Reason> {
        let list =
            self.users_service()?.settings().map_err(|e| users_reason(UsersOp::Settings, &e))?;
        let value = list.iter().find(|s| s.key == pbr_manifest::settings::TIMEZONE);
        Ok(value.and_then(|s| pbr_manifest::settings::timezone_minutes(&s.value)).unwrap_or(0))
    }

    /// `time` and `date`: the wall clock in the user's time zone. If the settings can't be
    /// had, the time is shown as UTC, and says so (`offset 0 h`).
    fn time(&self) -> Result<String, Reason> {
        let now = time::wall_now()
            .ok_or_else(|| Reason::text("this computer didn't say what time it is"))?;
        Ok(clock::time_line(now / 1_000_000_000, self.timezone().unwrap_or(0)))
    }

    /// `tz [HOURS]`: shows the offset, or sets the `timezone` setting (the users service
    /// checks it, saves it and applies it, so the taskbar's clock follows).
    fn tz(&self, offset: Option<i32>) -> Result<String, Reason> {
        let Some(minutes) = offset else {
            return Ok(clock::offset_line(self.timezone()?));
        };
        self.users_service()?
            .set(pbr_manifest::settings::TIMEZONE, &clock::setting_value(minutes))
            .map_err(|e| users_reason(UsersOp::Settings, &e))?;
        Ok(clock::offset_set_line(minutes))
    }

    /// `mem`.
    fn mem(&self) -> Result<String, Reason> {
        let m = self.sysinfo_service()?.memory().map_err(|e| sysinfo_reason(&e))?;
        Ok(facts::mem(m.total, m.available))
    }

    /// `disk`.
    fn disks(&self) -> Result<String, Reason> {
        let list = self.sysinfo_service()?.volumes().map_err(|e| sysinfo_reason(&e))?;
        Ok(facts::disk(&list))
    }

    /// `fetch`: asks for everything it shows; what can't be had is `unknown` in the picture.
    fn fetch(&self) -> String {
        let service = self.sysinfo.as_ref();
        let version = self.version();
        let memory = service.and_then(|s| s.memory().ok()).map(|m| (m.total, m.available));
        let screens = service.and_then(|s| s.screens().ok());
        let volumes = service.and_then(|s| s.volumes().ok());
        facts::fetch(&facts::Summary {
            version: version.as_ref().map(|(n, v)| (n.as_str(), v.as_str())),
            arch: if cfg!(target_arch = "x86_64") { "x86-64" } else { "ARM64" },
            screens: screens.as_deref(),
            memory,
            uptime: time::now() / 1_000_000_000,
            disk: volumes.as_deref().and_then(facts::system_volume),
        })
    }

    /// `check [VOLUME] [fix]`: says what it is about to do first, because checking the system
    /// volume restarts the session, which ends this shell before it could say anything after.
    fn check(&self, volume: Option<u32>, fix: bool) -> Result<(), Gone> {
        self.say(&facts::check_intro(volume, fix))?;
        let done = self.users_service().and_then(|u| {
            u.check(volume.unwrap_or(0), fix).map_err(|e| users_reason(UsersOp::Check, &e))
        });
        match done {
            Ok(r) => self.say(&facts::check_result(volume, fix, &r.report, r.problems)),
            Err(reason) => self.say(&format!("check: {reason}\n")),
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

    /// `settings`: the user's settings, one `key value` to a line, as their file has them.
    fn settings(&self) -> Result<String, Reason> {
        let list =
            self.users_service()?.settings().map_err(|e| users_reason(UsersOp::Settings, &e))?;
        Ok(list.iter().map(|s| format!("{} {}\n", s.key, s.value)).collect())
    }

    /// `set KEY VALUE`: the users service checks it, saves it and applies it.
    fn set(&self, key: &str, value: &str) -> Result<String, Reason> {
        self.users_service()?.set(key, value).map_err(|e| users_reason(UsersOp::Settings, &e))?;
        Ok(String::new())
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
    fn shutdown(&self) -> Result<(), Gone> {
        self.say("Saving everything...\n")?;
        let done = self
            .users_service()
            .and_then(|u| u.shutdown().map_err(|e| users_reason(UsersOp::Shutdown, &e)));
        match done {
            Ok(()) => self.say("It's now safe to turn off your computer.\n"),
            Err(reason) => self.say(&format!("shutdown: {reason}\n")),
        }
    }

    /// `poweroff` and `reboot` (note 0024, section 7): the users service saves everything,
    /// stops every app (this shell among them) and turns the computer off or restarts it. On
    /// success nothing comes back.
    fn power(&self, restart: bool) -> Result<(), Gone> {
        self.say(if restart { "Restarting...\n" } else { "Turning the computer off...\n" })?;
        let done = self.users_service().and_then(|u| {
            let r = if restart { u.restart() } else { u.power_off() };
            r.map_err(|e| users_reason(UsersOp::Power, &e))
        });
        match done {
            Ok(()) => Ok(()),
            Err(reason) => {
                let name = if restart { "reboot" } else { "poweroff" };
                self.say(&format!("{name}: {reason}\n"))
            }
        }
    }

    // ---- Apps (pbr.sandbox) ----

    /// `run APP [FILE…] [-- ARGS…]`.
    fn run_app(&self, app: &str, files: &[String], args: &str) -> Result<String, Failure> {
        let sandbox = self.sandbox_service().map_err(|r| Failure::new(app, r))?;
        let disk = &self.files.fs().0;
        // Open every file first, so that what can be checked here is checked before `prepare`:
        // a launch that is prepared and never started can't be cancelled (it waits in the
        // sandbox manager until this shell's later launches push it out).
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
