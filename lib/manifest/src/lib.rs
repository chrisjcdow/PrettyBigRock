// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! The plain-text files that decide what programs may do, and the decision itself
//! (docs/design/0017-users-sandbox-manager-and-manifests.md):
//!
//! - [`lines`]: the line reader every such file shares, driver manifests included;
//! - [`app`]: an app's manifest, `apps/<name>.app`: what it asks for;
//! - [`rules`]: a user's rules, `System/rules/<user>.txt`: what they allow;
//! - [`users`]: `System/users.txt`, the users;
//! - [`decide`]: what an app is granted, from its manifest and its user's rules;
//! - [`volume_names`]: the names volumes have under an administrator's `/Volumes`.
//!
//! None of it trusts its input: these files can be damaged, or written by someone trying to get
//! more than they should, so every format is checked strictly and the tests include hostile and
//! randomly damaged files. It's all plain computation, so it's built and tested on the host.

#![cfg_attr(not(test), no_std)]

extern crate alloc;

pub mod app;
pub mod lines;
pub mod rules;
pub mod settings;
pub mod users;

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// One of the folders in a user's home that apps ask for by name (note 0017, section 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Place {
    /// `/Home/Documents`
    Documents,
    /// `/Home/Pictures`
    Pictures,
    /// `/Home/Music`
    Music,
    /// `/Home/Videos`
    Videos,
    /// `/Home/Downloads`
    Downloads,
}

impl Place {
    /// Every place, in the order homes are made.
    pub const ALL: [Place; 5] =
        [Place::Documents, Place::Pictures, Place::Music, Place::Videos, Place::Downloads];

    /// The word manifests and rules use for it.
    pub fn word(self) -> &'static str {
        match self {
            Place::Documents => "documents",
            Place::Pictures => "pictures",
            Place::Music => "music",
            Place::Videos => "videos",
            Place::Downloads => "downloads",
        }
    }

    /// The folder's name in a home.
    pub fn folder(self) -> &'static str {
        match self {
            Place::Documents => "Documents",
            Place::Pictures => "Pictures",
            Place::Music => "Music",
            Place::Videos => "Videos",
            Place::Downloads => "Downloads",
        }
    }

    /// Its path in a session's tree.
    pub fn path(self) -> String {
        format!("/Home/{}", self.folder())
    }

    /// The place a word names.
    pub fn from_word(w: &str) -> Option<Place> {
        Place::ALL.into_iter().find(|p| p.word() == w)
    }
}

/// What may be done with a place. Reading always comes with it: manifests and rules say
/// `read` or `read write`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Rights {
    /// Read only.
    Read,
    /// Read and write.
    ReadWrite,
}

impl Rights {
    /// Parses `read` or `read write`.
    fn parse(words: &[&str]) -> Option<Rights> {
        match words {
            ["read"] => Some(Rights::Read),
            ["read", "write"] => Some(Rights::ReadWrite),
            _ => None,
        }
    }
}

/// Whether `s` is a user or app name: 1 to 32 lowercase ASCII letters, digits, `-` and `_`
/// (note 0017, section 3). Names become folder names and appear in rules, so they are kept
/// plain: no spaces, nothing that could be mistaken for a path.
pub fn is_name(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// A decimal number of bytes with an optional `K` or `M` suffix, as manifests write sizes.
pub(crate) fn size(s: &str) -> Option<u64> {
    let (digits, shift) = match s.as_bytes().last() {
        Some(b'K') => (&s[..s.len() - 1], 10),
        Some(b'M') => (&s[..s.len() - 1], 20),
        _ => (s, 0),
    };
    if digits.is_empty() || digits.len() > 12 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()?.checked_mul(1 << shift)
}

/// Why something was granted (note 0017, section 7, `GrantReason`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// The manifest asked, and as a system app it gets what it asks for unless a rule says no.
    Manifest,
    /// A rule allowed it.
    Rule,
}

/// One place granted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Granted {
    /// Which.
    pub place: Place,
    /// With what rights.
    pub rights: Rights,
    /// Why.
    pub reason: Reason,
}

/// What an app gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    /// The places, in the order the manifest asked for them.
    pub places: Vec<Granted>,
    /// Whether it gets the session's whole tree.
    pub trusted: bool,
    /// Whether it gets the session's terminal.
    pub terminal: bool,
}

/// What the app called `name`, with manifest `app`, is granted under `rules` (note 0017,
/// section 5). `system` says whether it is a system app (shipped in the start-up archive).
///
/// - A place is granted if a rule allows it, or the app is a system app and no rule denies
///   it. Its rights are what the manifest asks for, cut down to what an `allow` rule gives.
///   (A rules file can't both allow and deny one place to one app: [`rules::Rules::parse`]
///   refuses it.)
/// - `trusted` is granted only if the manifest asks, and the app is a system app no rule
///   distrusts, or a rule trusts it.
/// - `terminal` is granted only if the manifest asks and the app is trusted.
///
/// Nothing is granted that the manifest didn't ask for: rules only ever take away, or let an
/// app that isn't a system app have what it asked for.
pub fn decide(name: &str, app: &app::App, rules: &rules::Rules, system: bool) -> Decision {
    let mut places = Vec::new();
    for &(place, asked) in &app.places {
        if rules.denies(name, place) {
            continue;
        }
        if let Some(allowed) = rules.allows(name, place) {
            places.push(Granted { place, rights: asked.min(allowed), reason: Reason::Rule });
        } else if system {
            places.push(Granted { place, rights: asked, reason: Reason::Manifest });
        }
    }
    let trusted = app.trusted && (rules.trusts(name) || (system && !rules.distrusts(name)));
    Decision { places, trusted, terminal: app.terminal && trusted }
}

/// The names an administrator's volumes have under `/Volumes`, one for each label in `labels`
/// (in the same order): the label itself, or `Volume` if it is empty or can't be a name in a
/// file tree; and if two would be the same, the later ones get ` 2`, ` 3`… after them.
pub fn volume_names(labels: &[&str]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for &label in labels {
        // A name in a tree has no `/`, and is what `normalize` leaves it as (no `.` or `..`,
        // no control characters).
        let usable = !label.is_empty()
            && !label.contains('/')
            && pbr_filetree::normalize("/", label).is_ok_and(|p| p[1..] == *label);
        let base = if usable { label } else { "Volume" };
        let mut name = String::from(base);
        let mut n = 2;
        while out.contains(&name) {
            name = format!("{base} {n}");
            n += 1;
        }
        out.push(name);
    }
    out
}

/// A small deterministic random number generator for tests (xorshift64*).
#[cfg(test)]
pub(crate) struct Rng(pub u64);

#[cfg(test)]
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Randomly damages `base` a few bytes at a time, `rounds` times, handing each result to `f`.
#[cfg(test)]
pub(crate) fn damage(base: &[u8], rounds: usize, mut f: impl FnMut(&[u8])) {
    let mut rng = Rng(0x5eed);
    for _ in 0..rounds {
        let mut b = base.to_vec();
        for _ in 0..1 + rng.below(6) {
            let i = rng.below(b.len() as u64) as usize;
            match rng.below(3) {
                0 => b[i] = rng.next() as u8,
                1 => {
                    b.remove(i);
                }
                _ => b.insert(
                    i,
                    b"abcdefghijklmnopqrstuvwxyz \n\"#KM0123456789"[rng.below(42) as usize],
                ),
            }
        }
        f(&b);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn app(text: &str) -> app::App {
        app::App::parse(text.as_bytes()).unwrap()
    }
    fn rules(text: &str) -> rules::Rules {
        rules::Rules::parse(text.as_bytes()).unwrap()
    }
    fn g(place: Place, rights: Rights, reason: Reason) -> Granted {
        Granted { place, rights, reason }
    }

    const NOTES: &str =
        "title \"Notes\"\nbudget 1M\nfiles documents read write\nfiles pictures read\n";

    #[test]
    fn names() {
        assert!(
            is_name("chris") && is_name("a") && is_name("user_2-b") && is_name(&"x".repeat(32))
        );
        for bad in ["", "Chris", "a b", "a/b", "..", "é", &"x".repeat(33), "a.b"] {
            assert!(!is_name(bad), "{bad:?}");
        }
        assert_eq!(Place::from_word("music"), Some(Place::Music));
        assert_eq!(Place::from_word("Music"), None);
        assert_eq!(Place::Videos.path(), "/Home/Videos");
    }

    #[test]
    fn a_system_app_gets_what_it_asks_for() {
        let d = decide("notes", &app(NOTES), &rules(""), true);
        assert_eq!(
            d.places,
            vec![
                g(Place::Documents, Rights::ReadWrite, Reason::Manifest),
                g(Place::Pictures, Rights::Read, Reason::Manifest),
            ]
        );
        assert!(!d.trusted && !d.terminal);
        // Not a system app, and no rule: nothing.
        let d = decide("notes", &app(NOTES), &rules(""), false);
        assert!(d.places.is_empty());
    }

    #[test]
    fn rules_take_away() {
        let r = rules("deny notes pictures\nallow notes documents read\ndeny other documents\n");
        let d = decide("notes", &app(NOTES), &r, true);
        assert_eq!(d.places, vec![g(Place::Documents, Rights::Read, Reason::Rule)]);
        // Rules for other apps don't matter.
        let d = decide("paint", &app(NOTES), &r, true);
        assert_eq!(d.places.len(), 2);
        // An allow never gives more than the manifest asked for, nor a place it didn't ask for.
        let r = rules("allow notes pictures read write\nallow notes music read write\n");
        let d = decide("notes", &app(NOTES), &r, true);
        assert_eq!(
            d.places,
            vec![
                g(Place::Documents, Rights::ReadWrite, Reason::Manifest),
                g(Place::Pictures, Rights::Read, Reason::Rule),
            ]
        );
        // An app that isn't a system app gets only what rules allow.
        let d = decide("notes", &app(NOTES), &r, false);
        assert_eq!(d.places, vec![g(Place::Pictures, Rights::Read, Reason::Rule)]);
    }

    #[test]
    fn trust() {
        let shell = app("title \"Shell\"\nbudget 1M\ntrusted\nterminal\nfiles documents read\n");
        let d = decide("shell", &shell, &rules(""), true);
        assert!(d.trusted && d.terminal);
        let d = decide("shell", &shell, &rules("distrust shell\n"), true);
        assert!(!d.trusted && !d.terminal);
        // Distrusted, it still gets its places.
        assert_eq!(d.places, vec![g(Place::Documents, Rights::Read, Reason::Manifest)]);
        assert!(!decide("shell", &shell, &rules(""), false).trusted);
        assert!(decide("shell", &shell, &rules("trust shell\n"), false).trusted);
        // Trust only if the manifest asks.
        let d = decide("notes", &app(NOTES), &rules("trust notes\n"), true);
        assert!(!d.trusted);
        // A terminal only with trust.
        let t = app("title \"T\"\nbudget 1M\nterminal\n");
        assert!(!decide("t", &t, &rules(""), true).terminal);
    }

    #[test]
    fn volumes_are_named() {
        assert_eq!(
            volume_names(&["Data", "Home", "Data", "", "a/b", "Data 2", "Data", "..", "x\u{1}"]),
            vec![
                "Data", "Home", "Data 2", "Volume", "Volume 2", "Data 2 2", "Data 3", "Volume 3",
                "Volume 4"
            ]
        );
        assert_eq!(volume_names(&[]), Vec::<String>::new());
    }
}
