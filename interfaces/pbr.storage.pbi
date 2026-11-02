// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Disks and volumes (docs/design/0015-storage-and-the-block-cache.md, section 3). Served by
/// the storage service. Holding it means power over every volume, so the root program gives it
/// only to the users service.
interface pbr.storage version 1;

/// What a volume's contents look like.
enum Filesystem {
    RockFs @1,
    Fat32 @2,
    /// Nothing written yet: zeros.
    Empty @3,
    /// Something else.
    Unknown @4,
}

/// How a volume is.
enum VolumeState {
    /// Its filesystem is being served: `open` works.
    Ready @1,
    /// Its filesystem server is starting.
    Starting @2,
    /// There's no filesystem server for what it contains.
    Unsupported @3,
    /// Its structures are damaged, so it isn't served.
    Damaged @4,
    /// Its disk has failed.
    Failed @5,
}

/// One volume.
struct VolumeInfo {
    /// Its number, for the other operations.
    id: u32,
    /// The disk it's on: the device manager's id for the disk, or 1000 and up for a disk in
    /// memory.
    disk: u32,
    /// Its partition's number from 1, or 0 for a whole disk.
    partition: u32,
    filesystem: Filesystem,
    state: VolumeState,
    label: string<64>,
    /// Its size in bytes.
    size: u64,
    read_only: bool,
    in_memory: bool,
}

/// Every volume.
@1 fn list() -> (volumes: list<VolumeInfo, 32>) error StorageError;

/// A copy of the volume's root folder capability, with every right.
@2 fn open(id: u32) -> (root: handle<any>) error StorageError;

/// Makes a volume in memory of `size` bytes (rounded down to 4 KiB). It is formatted as
/// RockFS once the RockFS server exists (M5 stage 4); until then it is empty.
@3 fn create_ram_volume(size: u64, label: string<64>) -> (id: u32) error StorageError;

/// Puts a RockFS volume on an empty disk: an MBR with one partition from 1 MiB, as 1.3 does.
@4 fn format(id: u32, label: string<64>) -> () error StorageError;

/// Makes every volume clean and flushes every disk. Afterwards most operations fail with
/// `ShutDown`.
@5 fn shutdown() -> () error StorageError;

/// What can go wrong.
flexible enum StorageError {
    NoSuchVolume @1,
    /// Its filesystem server is starting: ask again.
    NotReady @2,
    /// `format` was asked to format something that isn't empty.
    NotEmpty @3,
    /// Not enough memory.
    NoMemory @4,
    /// A disk failed.
    DiskError @5,
    /// The service has been shut down.
    ShutDown @6,
    /// It can't be done to this volume, or not yet.
    Unsupported @7,
    /// A size or label is out of range.
    BadArgument @8,
}
