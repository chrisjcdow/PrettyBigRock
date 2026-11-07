// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Christopher Cameron-Dow

//! Cluster chains: the clusters of a file or folder, in order.
//!
//! FAT stores a file as a *chain*: the directory entry gives the first cluster, the FAT entry of
//! each cluster gives the next, and the last one is marked as the end. The *n*th cluster can
//! only be found by following *n* links from the start, so a [`Chain`] remembers where the last
//! search ended (its *hint*) and the next one, usually just after, starts from there. That makes
//! reading or appending a big file in order cost one step per cluster and not one walk from the
//! start per cluster.
//!
//! The chain is the first thing a hostile disk can attack: it can loop, point outside the
//! volume, point at free clusters, or stop too soon. Every step here is checked
//! ([`Volume::follow`]), and every walk that could go on for ever is cut off after as many steps
//! as the volume has clusters, because no valid chain is longer.

use crate::volume::Volume;
use alloc::vec;
use alloc::vec::Vec;
use pbr_fs::{BlockDevice, FsError};

/// The most bytes moved in one piece when filling a range with zeros.
const PIECE: u64 = 65_536;

/// A file's or folder's cluster chain, as far as memory knows it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Chain {
    /// The first cluster, or 0 if the file has no clusters yet.
    pub first: u32,
    /// The last place found: (index in the chain, cluster number). Only ever a place on the
    /// chain from `first`, so a search whose target is at or after it can start there.
    hint: Option<(u32, u32)>,
}

/// Where a search for the *n*th cluster ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Reach {
    /// It is this cluster.
    At(u32),
    /// The chain is shorter. The last cluster it does have is given as (index, number), or
    /// `None` if it has no cluster at all.
    EndedAt(Option<(u32, u32)>),
}

impl Chain {
    /// The chain that starts at `first` (0 for none).
    pub fn new(first: u32) -> Chain {
        Chain { first, hint: None }
    }

    /// Remembers that cluster number `index` of the chain is `cluster`.
    fn hint_set(&mut self, index: u32, cluster: u32) {
        self.hint = Some((index, cluster));
    }

    /// Forgets the hint. Needed whenever clusters before the hint may have changed.
    pub fn forget(&mut self) {
        self.hint = None;
    }
}

impl<D: BlockDevice> Volume<D> {
    /// Finds cluster number `index` of the chain (the first is 0). A chain that is broken before
    /// it gets there is [`FsError::Damaged`]; one that simply ends is [`Reach::EndedAt`].
    pub fn cluster_at(&mut self, chain: &mut Chain, index: u32) -> Result<Reach, FsError> {
        if chain.first == 0 {
            return Ok(Reach::EndedAt(None));
        }
        let (mut at, mut cluster) = match chain.hint {
            Some((i, c)) if i <= index => (i, c),
            _ => (0, chain.first),
        };
        if !self.geo.is_data_cluster(cluster) {
            return Err(FsError::Damaged);
        }
        let mut steps = 0u32;
        while at < index {
            let Some(next) = self.follow(cluster)? else {
                chain.hint = Some((at, cluster));
                return Ok(Reach::EndedAt(Some((at, cluster))));
            };
            cluster = next;
            at += 1;
            // A chain of valid clusters cannot be longer than the volume: if this walk has
            // gone further, the chain loops.
            steps += 1;
            if steps > self.geo.clusters {
                return Err(FsError::Damaged);
            }
        }
        chain.hint = Some((at, cluster));
        Ok(Reach::At(cluster))
    }

    /// Walks the whole chain from `first` and says how many clusters it has, checking it: it
    /// must end properly, within the volume, without a loop. 0 for no chain.
    pub fn check_chain(&mut self, first: u32) -> Result<u32, FsError> {
        if first == 0 {
            return Ok(0);
        }
        let mut cluster = first;
        let mut count = 1u32;
        while let Some(next) = self.follow(cluster)? {
            cluster = next;
            count += 1;
            if count > self.geo.clusters {
                return Err(FsError::Damaged);
            }
        }
        Ok(count)
    }

    /// The pieces of the disk holding bytes `offset..offset + len` of the chain, as (address in
    /// the volume, length): neighbouring clusters make one piece, so a file stored in one
    /// piece is read with few requests. A chain too short for the range is `Damaged`.
    pub fn runs(
        &mut self,
        chain: &mut Chain,
        offset: u64,
        len: usize,
    ) -> Result<Vec<(u64, usize)>, FsError> {
        let cluster_bytes = u64::from(self.geo.cluster_bytes);
        let end = offset.checked_add(len as u64).ok_or(FsError::BadArgument)?;
        let mut out = Vec::new();
        let mut pos = offset;
        while pos < end {
            let mut index = u32::try_from(pos / cluster_bytes).map_err(|_| FsError::BadArgument)?;
            let within = pos % cluster_bytes;
            let Reach::At(first) = self.cluster_at(chain, index)? else {
                return Err(FsError::Damaged); // the chain is shorter than the file says
            };
            let mut last = first;
            let mut available = cluster_bytes - within;
            // Extend the piece while the next cluster is the next one on the disk.
            while pos + available < end {
                match self.follow(last)? {
                    Some(next) if next == last + 1 => {
                        last = next;
                        index += 1;
                        available += cluster_bytes;
                    }
                    _ => break,
                }
            }
            chain.hint_set(index, last);
            let take = available.min(end - pos);
            let start = self.geo.cluster_offset(first).ok_or(FsError::Damaged)? + within;
            out.push((start, take as usize));
            pos += take;
        }
        Ok(out)
    }

    /// Reads bytes `offset..offset + buf.len()` of the chain. The chain must be long enough.
    pub fn read_chain(
        &mut self,
        chain: &mut Chain,
        offset: u64,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        let mut done = 0;
        for (addr, len) in self.runs(chain, offset, buf.len())? {
            self.read_bytes(addr, &mut buf[done..done + len])?;
            done += len;
        }
        Ok(())
    }

    /// Writes `data` at `offset` of the chain, over what is there. The chain must be long
    /// enough ([`Volume::grow`] first).
    pub fn write_chain(
        &mut self,
        chain: &mut Chain,
        offset: u64,
        data: &[u8],
    ) -> Result<(), FsError> {
        let mut done = 0;
        for (addr, len) in self.runs(chain, offset, data.len())? {
            self.write_bytes(addr, &data[done..done + len])?;
            done += len;
        }
        Ok(())
    }

    /// Writes zeros over bytes `from..to` of the chain, which must be long enough.
    pub fn zero_chain(&mut self, chain: &mut Chain, from: u64, to: u64) -> Result<(), FsError> {
        if to <= from {
            return Ok(());
        }
        let zeros = vec![0u8; (to - from).min(PIECE) as usize];
        let mut pos = from;
        while pos < to {
            let n = (to - pos).min(PIECE) as usize;
            let mut done = 0;
            for (addr, len) in self.runs(chain, pos, n)? {
                self.write_bytes(addr, &zeros[done..done + len])?;
                done += len;
            }
            pos += n as u64;
        }
        Ok(())
    }

    /// Makes the chain at least `needed` clusters long, taking new clusters from the free ones
    /// and linking each into the FAT as it goes. Does nothing if it is long enough already (a
    /// file that was cut and not yet committed keeps its old clusters, which are reused here).
    /// The new clusters' contents are whatever was there before, so the caller must write or
    /// zero every byte of them that will be read.
    ///
    /// If the space runs out, the clusters taken so far are given back and the chain is as it
    /// was: [`FsError::NoSpace`].
    pub fn grow(&mut self, chain: &mut Chain, needed: u32) -> Result<(), FsError> {
        if needed == 0 {
            return Ok(());
        }
        if needed > self.geo.clusters {
            return Err(FsError::NoSpace);
        }
        let last = match self.cluster_at(chain, needed - 1)? {
            Reach::At(_) => return Ok(()),
            Reach::EndedAt(last) => last,
        };
        let have = last.map_or(0, |(i, _)| i + 1);
        let original_last = last.map(|(_, c)| c);
        let mut prev = original_last;
        let mut first_new = None;
        for _ in have..needed {
            match self.alloc_cluster(prev) {
                Ok(c) => {
                    first_new.get_or_insert(c);
                    if prev.is_none() {
                        chain.first = c;
                    }
                    prev = Some(c);
                }
                Err(e) => {
                    if let Some(f) = first_new {
                        if original_last.is_none() {
                            chain.first = 0;
                        }
                        self.release_tail(original_last, f)?;
                    }
                    chain.forget();
                    return Err(e);
                }
            }
        }
        // The hint stays at the old end of the chain, which `cluster_at` left there. It is *not*
        // moved to the new end: the write that follows starts at or near the old end, and a hint
        // beyond the place searched for is useless, so every append would walk the whole chain
        // from the start (the cost of a file written in pieces would grow with its square).
        Ok(())
    }
}
