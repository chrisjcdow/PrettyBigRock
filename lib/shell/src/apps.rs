// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Small decisions about apps that the shell makes before it calls the sandbox manager.

use crate::command::AppRef;
use crate::reason::Reason;
use alloc::string::String;
use pbr_interfaces::pbr_sandbox::AppInfo;

/// The longest path `pbr.sandbox` accepts, in bytes.
pub const MAX_SANDBOX_PATH: usize = 1024;

/// The number of the app `app` means. A number is taken as it is (the sandbox manager says if
/// there is no such app); a name means the most recently started running app of that name, and
/// app numbers only go up, so that is the one with the highest number.
pub fn resolve(list: &[AppInfo], app: &AppRef) -> Result<u32, Reason> {
    match app {
        AppRef::Id(id) => Ok(*id),
        AppRef::Name(name) => list
            .iter()
            .filter(|a| a.running && a.app == *name)
            .map(|a| a.id)
            .max()
            .ok_or_else(|| Reason::text("no running app has that name")),
    }
}

/// `path`, which must fit in what the sandbox manager accepts.
pub fn sandbox_path(path: String) -> Result<String, Reason> {
    if path.len() > MAX_SANDBOX_PATH { Err(Reason::BadPath) } else { Ok(path) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;
    use alloc::vec;

    fn app(id: u32, name: &str, running: bool) -> AppInfo {
        AppInfo { id, app: name.to_string(), trusted: false, running }
    }

    #[test]
    fn numbers_are_taken_as_they_are() {
        assert_eq!(resolve(&[], &AppRef::Id(42)), Ok(42));
    }

    #[test]
    fn names_mean_the_newest_running_app() {
        let list = vec![
            app(1, "notes", true),
            app(3, "notes", true),
            app(5, "notes", false),
            app(4, "paint", true),
            app(2, "notes", true),
        ];
        assert_eq!(resolve(&list, &AppRef::Name("notes".to_string())), Ok(3));
        assert_eq!(resolve(&list, &AppRef::Name("paint".to_string())), Ok(4));
        assert!(resolve(&list, &AppRef::Name("calc".to_string())).is_err());
        // An app that ended doesn't count, even if it is the only one of its name.
        let ended = vec![app(9, "gone", false)];
        assert!(resolve(&ended, &AppRef::Name("gone".to_string())).is_err());
        // Names are matched exactly.
        assert!(resolve(&list, &AppRef::Name("Notes".to_string())).is_err());
    }

    #[test]
    fn paths_for_the_sandbox_manager() {
        assert!(sandbox_path("a".repeat(1024)).is_ok());
        assert_eq!(sandbox_path("a".repeat(1025)), Err(Reason::BadPath));
    }
}
