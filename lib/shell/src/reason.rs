// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Why something failed, in plain English.
//!
//! Every error line the shell prints looks like `<command>: <path>: <reason>`. The reasons come
//! from several places (the filesystem, the sandbox manager, the users service, the kernel's
//! calls, the shell's own checks), and none of them may reach the user as Rust debug output such
//! as `Service(NotFound)`. [`Reason`] is the one type they are all turned into, and its
//! `Display` is the English.
//!
//! In Rust an `enum` is a type that is exactly one of several named variants, and `impl
//! From<A> for B` says how to turn an `A` into a `B` (`.into()` or `?` then does it for you).

use alloc::boxed::Box;
use alloc::format;
use alloc::string::String;
use core::fmt;
use granite::interface::CallError;
use pbr_interfaces::pbr_file::FileError;
use pbr_interfaces::pbr_sandbox::SandboxError;
use pbr_interfaces::pbr_users::UsersError;

/// Why a command couldn't do what it was asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reason {
    /// There is nothing at that path.
    NotFound,
    /// There is something there already.
    AlreadyExists,
    /// A file was found where a folder was needed.
    NotAFolder,
    /// A folder was found where a file was needed.
    NotAFile,
    /// The capability the shell holds for it doesn't allow this (note 0014, section 2).
    NoRight,
    /// A name that breaks the rules.
    BadName,
    /// A path that breaks the rules (too long, or not valid).
    BadPath,
    /// The folder has things in it.
    FolderNotEmpty,
    /// The volume is full.
    NoSpace,
    /// The volume can't be written.
    ReadOnly,
    /// The filesystem can't hold something that big.
    TooBig,
    /// It was removed while the shell was using it.
    Gone,
    /// The filesystem found damage.
    Damaged,
    /// The disk gave an error.
    DiskError,
    /// The server holds as many open files as it can.
    TooManyOpen,
    /// The filesystem can't do that.
    BadArgument,
    /// The path is a folder that only exists because something is attached below it (`/`,
    /// `/Volumes`, `/System`): nothing can be changed in it.
    Virtual,
    /// The path is inside such a folder, so it would have to be made there.
    UnderVirtual,
    /// The path is where a part of the tree is attached (`/Home`, `/Volumes/Data`): it can't be
    /// removed, moved or replaced.
    MountPoint,
    /// A folder was named where `-r` is needed.
    NeedsRecursive,
    /// A folder can't go inside itself.
    IntoItself,
    /// The two paths are the same.
    SameFile,
    /// Folders inside folders go deeper than the shell will follow.
    TooDeep,
    /// A file is too big for `cat`.
    TooBigToShow,
    /// A move copied the thing, but the original could not be removed afterwards.
    LeftBehind(Box<Reason>),
    /// The service holding the thing has gone away (it ended, or it was shut down).
    ServiceGone,
    /// The service didn't answer in time.
    TimedOut,
    /// The answer made no sense.
    Garbled,
    /// Anything else, already in words.
    Text(String),
}

impl Reason {
    /// A reason made of the words given.
    pub fn text(words: impl Into<String>) -> Reason {
        Reason::Text(words.into())
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Reason::NotFound => "not found",
            Reason::AlreadyExists => "already exists",
            Reason::NotAFolder => "not a folder",
            Reason::NotAFile => "not a file",
            Reason::NoRight => "you don't have the right to do that",
            Reason::BadName => "that isn't a name the filesystem allows",
            Reason::BadPath => "that isn't a valid path",
            Reason::FolderNotEmpty => "the folder isn't empty",
            Reason::NoSpace => "no space left",
            Reason::ReadOnly => "read only",
            Reason::TooBig => "too big for the filesystem",
            Reason::Gone => "it has been removed",
            Reason::Damaged => "the disk is damaged here",
            Reason::DiskError => "the disk gave an error",
            Reason::TooManyOpen => "too many files are open",
            Reason::BadArgument => "the filesystem can't do that",
            Reason::Virtual => "that folder only holds other folders, so it can't be changed",
            Reason::UnderVirtual => "it would have to go in a folder that only holds other folders",
            Reason::MountPoint => {
                "that is where something is attached, so it can't be removed, moved or replaced"
            }
            Reason::NeedsRecursive => "it is a folder (use -r to include what is inside it)",
            Reason::IntoItself => "a folder can't go inside itself",
            Reason::SameFile => "those are the same place",
            Reason::TooDeep => "the folders go too deep to follow",
            Reason::TooBigToShow => "too big to show (64 KB at most)",
            Reason::LeftBehind(why) => {
                return write!(f, "copied, but the original could not be removed: {why}");
            }
            Reason::ServiceGone => "the service holding it has gone",
            Reason::TimedOut => "it took too long to answer",
            Reason::Garbled => "the answer made no sense",
            Reason::Text(t) => t,
        })
    }
}

impl From<FileError> for Reason {
    fn from(e: FileError) -> Reason {
        match e {
            FileError::NotFound => Reason::NotFound,
            FileError::AlreadyExists => Reason::AlreadyExists,
            FileError::NotAFolder => Reason::NotAFolder,
            FileError::NotAFile => Reason::NotAFile,
            FileError::NoRight => Reason::NoRight,
            FileError::BadName => Reason::BadName,
            FileError::BadPath => Reason::BadPath,
            FileError::FolderNotEmpty => Reason::FolderNotEmpty,
            FileError::NoSpace => Reason::NoSpace,
            FileError::ReadOnly => Reason::ReadOnly,
            FileError::TooBig => Reason::TooBig,
            FileError::Gone => Reason::Gone,
            FileError::Damaged => Reason::Damaged,
            FileError::DiskError => Reason::DiskError,
            FileError::TooManyOpen => Reason::TooManyOpen,
            FileError::BadArgument => Reason::BadArgument,
            FileError::Unknown(n) => Reason::Text(format!("the filesystem gave error {n}")),
        }
    }
}

/// A call that failed before the service could answer it.
impl From<granite::Error> for Reason {
    fn from(e: granite::Error) -> Reason {
        match e {
            granite::Error::PeerGone => Reason::ServiceGone,
            granite::Error::TimedOut => Reason::TimedOut,
            granite::Error::Revoked => Reason::Text(String::from("access to it was taken back")),
            other => Reason::Text(format!("the request failed ({other})")),
        }
    }
}

/// What a failed call to a service says when the service itself had no complaint: the call
/// didn't get through, or the answer was wrong.
fn call_reason<E>(e: &CallError<E>) -> Reason {
    match e {
        CallError::Call(e) => Reason::from(*e),
        CallError::Unknown => Reason::text("the service doesn't know how to do that"),
        CallError::Rejected => Reason::text("the service refused the request as malformed"),
        CallError::Failed => Reason::text("the service failed"),
        CallError::Malformed => Reason::Garbled,
        // The service's own error is the caller's to explain; this is only a safe fallback.
        CallError::Service(_) => Reason::text("the service said no"),
    }
}

/// Why a `pbr.file` call failed: the filesystem's own answer, or the call not getting through.
pub fn file_reason(e: &CallError<FileError>) -> Reason {
    match e {
        CallError::Service(e) => Reason::from(*e),
        other => call_reason(other),
    }
}

/// Which `pbr.sandbox` call failed, because a few errors read differently for each.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SandboxOp {
    /// `run`: `prepare`, `choose` and `start`.
    Run,
    /// `apps`.
    List,
    /// `grants`.
    Grants,
    /// `revoke`.
    Revoke,
    /// `stop`.
    Stop,
}

/// Why a `pbr.sandbox` call failed.
pub fn sandbox_reason(op: SandboxOp, e: &CallError<SandboxError>) -> Reason {
    let CallError::Service(e) = e else { return call_reason(e) };
    Reason::text(match e {
        SandboxError::NoSuchApp => "no such app",
        SandboxError::BadManifest => "its manifest is broken",
        SandboxError::BadRules => "your rules file is broken",
        SandboxError::NotFound => match op {
            SandboxOp::Revoke => "no such app, or it doesn't hold that",
            SandboxOp::Run => "the launch was lost; try again",
            _ => "no such app",
        },
        SandboxError::BadPath => "that isn't a valid path for the app",
        SandboxError::Overlaps => "that would overlap something it gets already",
        SandboxError::TooMany => "too many apps or launches at once",
        SandboxError::NoMemory => "there isn't enough memory to start it",
        SandboxError::BadCapability => "that isn't a file or folder the app can be given",
        SandboxError::Failed => "it couldn't be started (the log says why)",
        SandboxError::NotAllowed => "the shell isn't allowed to do that",
        SandboxError::Unknown(n) => return Reason::Text(format!("the sandbox gave error {n}")),
    })
}

/// Which `pbr.users` call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UsersOp {
    /// `whoami`.
    WhoAmI,
    /// `users`.
    List,
    /// `adduser`.
    Add,
    /// `volumes`.
    Volumes,
    /// `format`.
    Format,
    /// `setup`.
    Setup,
    /// `shutdown`.
    Shutdown,
    /// `poweroff` and `reboot`.
    Power,
    /// `settings` and `set`.
    Settings,
}

/// Why a `pbr.users` call failed.
pub fn users_reason(op: UsersOp, e: &CallError<UsersError>) -> Reason {
    let CallError::Service(e) = e else { return call_reason(e) };
    Reason::text(match e {
        UsersError::NotAllowed => "you don't have the right to do that",
        UsersError::BadName => match op {
            UsersOp::Add => "names are 1 to 32 lowercase letters, digits, - and _",
            UsersOp::Setup => "names are 1 to 32 lowercase letters, digits, - and _",
            _ => "that name isn't allowed",
        },
        UsersError::AlreadyExists => match op {
            UsersOp::Add => "there is a user with that name already",
            UsersOp::Setup => "that volume is set up already",
            _ => "already exists",
        },
        UsersError::NoSuchVolume => "there is no volume with that number",
        UsersError::Unsuitable => match op {
            UsersOp::Format => "that volume isn't empty, so it won't be formatted",
            UsersOp::Setup => "that volume doesn't have RockFS on it",
            _ => "that volume isn't suitable",
        },
        UsersError::NotReady => match op {
            UsersOp::Settings => "the input service isn't running; try again in a moment",
            _ => "that volume is still starting; try again in a moment",
        },
        UsersError::Failed => "the storage service failed (the log says why)",
        UsersError::TooMany => match op {
            UsersOp::Settings => "no more settings fit",
            _ => "no more users fit",
        },
        UsersError::BadSetting => {
            "the settings are layout (a keyboard layout there is, such as us, gb or us-intl) \
             and timezone (such as +02:00)"
        }
        UsersError::NotSupported => {
            "this computer can't be turned off by software: it has shut down, and you can turn \
             it off now"
        }
        UsersError::Unknown(n) => return Reason::Text(format!("the users service gave error {n}")),
    })
}

/// A failure, with the path it was about: `<path>: <reason>`, ready to follow `<command>: `.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Failure {
    /// What was being worked on, as the user typed it (or, deeper inside a folder, the full
    /// path of the item that failed).
    pub path: String,
    /// Why it failed.
    pub reason: Reason,
}

impl Failure {
    /// A failure at `path`.
    pub fn new(path: impl Into<String>, reason: Reason) -> Failure {
        Failure { path: path.into(), reason }
    }
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.path, self.reason)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    #[test]
    fn file_errors_read_as_english() {
        for (e, want) in [
            (FileError::NotFound, "not found"),
            (FileError::AlreadyExists, "already exists"),
            (FileError::NotAFolder, "not a folder"),
            (FileError::NoRight, "you don't have the right to do that"),
            (FileError::FolderNotEmpty, "the folder isn't empty"),
            (FileError::NoSpace, "no space left"),
            (FileError::ReadOnly, "read only"),
        ] {
            assert_eq!(Reason::from(e).to_string(), want);
        }
    }

    #[test]
    fn nothing_looks_like_debug_output() {
        let all = [
            FileError::NotFound,
            FileError::AlreadyExists,
            FileError::NotAFolder,
            FileError::NotAFile,
            FileError::NoRight,
            FileError::BadName,
            FileError::BadPath,
            FileError::FolderNotEmpty,
            FileError::NoSpace,
            FileError::ReadOnly,
            FileError::TooBig,
            FileError::Gone,
            FileError::Damaged,
            FileError::DiskError,
            FileError::TooManyOpen,
            FileError::BadArgument,
            FileError::Unknown(77),
        ];
        for e in all {
            let text = Reason::from(e).to_string();
            assert!(!text.contains("Service") && !text.contains('('), "{text}");
            assert!(text.chars().next().unwrap().is_lowercase(), "{text}");
        }
        assert!(Reason::from(FileError::Unknown(77)).to_string().contains("77"));
    }

    #[test]
    fn calls_that_never_arrived() {
        assert_eq!(Reason::from(granite::Error::PeerGone), Reason::ServiceGone);
        assert_eq!(Reason::from(granite::Error::TimedOut), Reason::TimedOut);
        let other = Reason::from(granite::Error::BadHandle).to_string();
        assert!(other.starts_with("the request failed"), "{other}");
    }

    #[test]
    fn file_calls() {
        assert_eq!(file_reason(&CallError::Service(FileError::NoSpace)), Reason::NoSpace);
        assert_eq!(file_reason(&CallError::Call(granite::Error::PeerGone)), Reason::ServiceGone);
        assert_eq!(file_reason(&CallError::Malformed), Reason::Garbled);
    }

    #[test]
    fn sandbox_errors() {
        let svc = |e| CallError::Service(e);
        let run = |e| sandbox_reason(SandboxOp::Run, &svc(e)).to_string();
        assert_eq!(run(SandboxError::NoSuchApp), "no such app");
        assert_eq!(run(SandboxError::BadManifest), "its manifest is broken");
        assert_eq!(run(SandboxError::BadRules), "your rules file is broken");
        assert_eq!(run(SandboxError::Overlaps), "that would overlap something it gets already");
        assert_eq!(
            sandbox_reason(SandboxOp::Revoke, &svc(SandboxError::NotFound)).to_string(),
            "no such app, or it doesn't hold that"
        );
        assert!(run(SandboxError::Unknown(99)).contains("99"));
        assert_eq!(
            sandbox_reason(SandboxOp::Stop, &CallError::Call(granite::Error::PeerGone)),
            Reason::ServiceGone
        );
    }

    #[test]
    fn users_errors() {
        let svc = |e| CallError::Service(e);
        assert_eq!(
            users_reason(UsersOp::Volumes, &svc(UsersError::NotAllowed)).to_string(),
            "you don't have the right to do that"
        );
        assert!(
            users_reason(UsersOp::Format, &svc(UsersError::Unsuitable))
                .to_string()
                .contains("isn't empty")
        );
        assert!(
            users_reason(UsersOp::Setup, &svc(UsersError::Unsuitable))
                .to_string()
                .contains("RockFS")
        );
        assert_eq!(
            users_reason(UsersOp::Add, &CallError::<UsersError>::Malformed),
            Reason::Garbled
        );
    }

    #[test]
    fn failure_lines() {
        let f = Failure::new("a.txt", Reason::NotFound);
        assert_eq!(f.to_string(), "a.txt: not found");
        let moved = Failure::new("a", Reason::LeftBehind(Box::new(Reason::ReadOnly)));
        assert_eq!(
            moved.to_string(),
            "a: copied, but the original could not be removed: read only"
        );
    }
}
