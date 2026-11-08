// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Users and sessions (docs/design/0017-users-sandbox-manager-and-manifests.md, section 3).
/// Served by the users service, with a badge for each session, so it knows who is asking. A
/// session's sandbox manager holds it, and gives copies to the session's trusted apps.
interface pbr.users version 1;

/// What kind of user.
enum UserKind {
    /// Also holds every volume, and can add users and set up disks.
    Admin @1,
    Standard @2,
}

/// One user.
struct User {
    /// 1 to 32 lowercase letters, digits, `-` and `_`: it names their home folder.
    name: string<32>,
    kind: UserKind,
    /// The name people see (UTF-8, without `"`).
    display: string<128>,
}

/// What a volume's contents look like (copied from `pbr.storage` until interfaces can import
/// each other; note 0008).
enum Filesystem { RockFs @1, Fat32 @2, Empty @3, Unknown @4 }

/// How a volume is (copied from `pbr.storage`).
enum VolumeState { Ready @1, Starting @2, Unsupported @3, Damaged @4, Failed @5 }

/// One volume (copied from `pbr.storage`).
struct VolumeInfo {
    id: u32,
    disk: u32,
    partition: u32,
    filesystem: Filesystem,
    state: VolumeState,
    label: string<64>,
    size: u64,
    read_only: bool,
    in_memory: bool,
}

/// Who this session is.
@1 fn whoami() -> (user: User) error UsersError;

/// Every user (administrators only).
@2 fn list() -> (users: list<User, 64>) error UsersError;

/// Makes a RockFS volume the system volume from the next start, with `name` as its first
/// administrator (administrators only): adds `System/` and `Users/NAME/` to it and touches
/// nothing else. An empty volume is formatted first.
@3 fn setup(volume: u32, name: string<32>) -> () error UsersError;

/// Adds a user, with a home and an empty rules file (administrators only). Their session
/// starts at the next start.
@4 fn add(user: User) -> () error UsersError;

/// The storage service's volumes (administrators only).
@5 fn volumes() -> (volumes: list<VolumeInfo, 32>) error UsersError;

/// Puts RockFS on an empty volume (administrators only).
@6 fn format(volume: u32, label: string<64>) -> () error UsersError;

/// Has the storage service save every volume and flush every disk, so that it is safe to turn
/// the computer off (administrators only; the shell's `shutdown`, note 0018 section 6).
/// Afterwards the volumes can't be used until the next start.
@7 fn shutdown() -> () error UsersError;

/// What can go wrong.
flexible enum UsersError {
    /// Only administrators may.
    NotAllowed @1,
    /// A name or display name that breaks the rules.
    BadName @2,
    /// There is already a user with that name, or the volume is set up already.
    AlreadyExists @3,
    NoSuchVolume @4,
    /// The volume isn't empty (`format`), or isn't RockFS (`setup`).
    Unsuitable @5,
    /// The volume's filesystem server is starting: ask again.
    NotReady @6,
    /// The storage service or a filesystem failed.
    Failed @7,
    /// No more users fit.
    TooMany @8,
}
