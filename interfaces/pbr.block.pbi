// SPDX-License-Identifier: MIT
// Copyright (C) 2026 Christopher Cameron-Dow

/// Disks (docs/design/0011-device-manager-and-drivers.md, section 8). Served by block drivers.
/// Reads and writes are idempotent: doing one twice has the same effect as doing it once, so a
/// client may retry after the driver restarts.
interface pbr.block version 1;

/// What a disk is.
struct BlockInfo {
    /// Bytes in a sector (512 for the disks M4 drives).
    sector_size: u32,
    /// How many sectors.
    sector_count: u64,
    /// Writes are refused.
    read_only: bool,
}

/// The disk's size and sector size.
@1 fn info() -> (info: BlockInfo) error BlockError;

/// `count` sectors from `sector`: at most 65536 bytes, so at most 128 sectors of 512 bytes.
@2 fn read(sector: u64, count: u32) -> (data: bytes<65536>) error BlockError;

/// Writes whole sectors from `sector`. `data` must be a multiple of the sector size.
@3 fn write(sector: u64, data: bytes<65536>) -> () error BlockError;

/// Does not return until everything written so far is on the disk.
@4 fn flush() -> () error BlockError;

/// What can go wrong.
flexible enum BlockError {
    /// The sectors are not all on the disk.
    OutOfRange @1,
    /// The disk is read-only.
    ReadOnly @2,
    /// The disk failed the request, or answered nonsense.
    DeviceError @3,
    /// The request was not a whole number of sectors, was empty, or was too big.
    BadArgument @4,
}
