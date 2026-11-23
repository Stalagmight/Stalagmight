//! Who owns each archived record (plan 2026-11-23 RS, package RS-FX4).
//!
//! A node's records ([`ArchivedStation`]) are a forest under the node's own
//! identity. A record's [`ArchivedStation::into`] is unset when the node's
//! identity holds it (the merge that archived it kept this identity), or the
//! Station ID of another record of the node: the station that held it as
//! its own record when a later merge archived that station whole. The list
//! order is the order (oldest first); a subtree taken out keeps it.
//!
//! A record and every record under it are one former junction: Detach Line
//! from Station on any line the subtree names brings the top record back as
//! a station, on all those lines, with the records under it inside it as
//! they were before the merge that archived it (RS 4.2, together-detach).
//!
//! Invariant A6 ([`owner_faults`]): `into` names another record of the same
//! node, which is not history (A5: a history record owns nothing), and
//! following `into` never cycles. Every resolution here is linear and
//! bounded, so a crafted file whose owners cycle reports instead of hanging.

use std::collections::HashMap;

use crate::{LineId, StationId};

use super::{ArchivedStation, Station};

/// What is wrong with a record's `into` (invariant A6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerFault {
    /// It names no other record of the node (or the record itself).
    Dangling,
    /// It names a history record, which owns nothing (A5).
    History,
    /// Following it comes back to the record.
    Cycle,
}

/// The index of each record's id in `records` (the first, for a duplicate
/// id, which A1 reports).
fn index_of(records: &[ArchivedStation]) -> HashMap<StationId, usize> {
    let mut index = HashMap::with_capacity(records.len());
    for (i, record) in records.iter().enumerate() {
        index.entry(record.id).or_insert(i);
    }
    index
}

/// The walk state of one record in [`walk`].
#[derive(Debug, Clone, Copy)]
enum State {
    Unseen,
    Walking,
    Done(Option<usize>),
}

/// For each record, the index of the top of its owner chain (the record
/// under the node's own identity it is archived under; itself for a record
/// with no `into`), and whether it lies on a cycle. A chain that names a
/// missing record or cycles has no top. Each record is walked once.
fn walk(records: &[ArchivedStation]) -> (Vec<Option<usize>>, Vec<bool>) {
    let index = index_of(records);
    let mut state = vec![State::Unseen; records.len()];
    let mut on_cycle = vec![false; records.len()];
    let mut path: Vec<usize> = Vec::new();
    for start in 0..records.len() {
        if !matches!(state[start], State::Unseen) {
            continue;
        }
        path.clear();
        let mut current = start;
        let answer = loop {
            match state[current] {
                State::Done(top) => break top,
                State::Walking => {
                    if let Some(first) = path.iter().position(|&i| i == current) {
                        for &i in &path[first..] {
                            on_cycle[i] = true;
                        }
                    }
                    break None;
                }
                State::Unseen => {}
            }
            state[current] = State::Walking;
            path.push(current);
            match records[current].into {
                None => break Some(current),
                Some(owner) => match index.get(&owner) {
                    Some(&next) if next != current => current = next,
                    _ => break None,
                },
            }
        };
        for &i in &path {
            state[i] = State::Done(answer);
        }
    }
    let tops = state
        .into_iter()
        .map(|s| match s {
            State::Done(top) => top,
            State::Unseen | State::Walking => None,
        })
        .collect();
    (tops, on_cycle)
}

/// Each record's owner fault (A6), in list order: a record whose own `into`
/// is wrong. Records that only lead into a wrong one are not listed: they
/// are right once it is.
pub(super) fn owner_faults(records: &[ArchivedStation]) -> Vec<(usize, OwnerFault)> {
    if records.iter().all(|r| r.into.is_none()) {
        return Vec::new();
    }
    let index = index_of(records);
    let (_, on_cycle) = walk(records);
    let mut out = Vec::new();
    for (i, record) in records.iter().enumerate() {
        let Some(owner) = record.into else {
            continue;
        };
        match index.get(&owner) {
            Some(&o) if o != i => {
                if on_cycle[i] {
                    out.push((i, OwnerFault::Cycle));
                } else if records[o].is_history() {
                    out.push((i, OwnerFault::History));
                }
            }
            _ => out.push((i, OwnerFault::Dangling)),
        }
    }
    out
}

impl Station {
    /// The record under this station's own identity whose subtree holds
    /// record `id` (itself when `id` is archived under the identity): the
    /// station Detach Line from Station gives back for a line `id` names.
    /// `None` for an id that is no record here, or whose chain is broken.
    pub fn record_top(&self, id: StationId) -> Option<&ArchivedStation> {
        let at = self.archived.iter().position(|r| r.id == id)?;
        if self.archived[at].into.is_none() {
            return Some(&self.archived[at]);
        }
        let (tops, _) = walk(&self.archived);
        tops[at].map(|top| &self.archived[top])
    }

    /// The indices of record `top`'s subtree (`top` and every record under
    /// it), in list order. Each record is decided once.
    fn subtree(&self, top: StationId) -> Vec<usize> {
        let Some(at) = self.archived.iter().position(|r| r.id == top) else {
            return Vec::new();
        };
        if self.archived.iter().all(|r| r.into.is_none()) {
            return vec![at];
        }
        let index = index_of(&self.archived);
        // `Done(Some(_))` marks a record inside, `Done(None)` one outside;
        // a chain that meets a record it is walking cycles (outside).
        let mut state = vec![State::Unseen; self.archived.len()];
        state[at] = State::Done(Some(at));
        let mut path: Vec<usize> = Vec::new();
        for start in 0..self.archived.len() {
            path.clear();
            let mut current = start;
            let answer = loop {
                match state[current] {
                    State::Done(known) => break known,
                    State::Walking => break None,
                    State::Unseen => {}
                }
                state[current] = State::Walking;
                path.push(current);
                match self.archived[current].into.and_then(|o| index.get(&o)) {
                    Some(&next) => current = next,
                    None => break None,
                }
            };
            for &i in &path {
                state[i] = State::Done(answer);
            }
        }
        (0..self.archived.len())
            .filter(|&i| matches!(state[i], State::Done(Some(_))))
            .collect()
    }

    /// Every line record `top`'s subtree names, in list order (the lines
    /// a Detach of any of them takes off together).
    pub fn subtree_lines(&self, top: StationId) -> Vec<LineId> {
        let mut lines = Vec::new();
        for i in self.subtree(top) {
            for &line in &self.archived[i].lines {
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
        lines
    }

    /// Take record `id` out of the list; the records archived under it move
    /// up to the identity it was archived under. `None` when no record has
    /// that id.
    pub fn take_record(&mut self, id: StationId) -> Option<ArchivedStation> {
        let at = self.archived.iter().position(|r| r.id == id)?;
        let record = self.archived.remove(at);
        for r in &mut self.archived {
            if r.into == Some(id) {
                r.into = record.into;
            }
        }
        Some(record)
    }

    /// Take record `top`'s subtree out of the list: `top`, and the records
    /// under it in list order, those archived under `top` itself with `into`
    /// unset (they are the restored station's own records now).
    pub fn take_subtree(
        &mut self,
        top: StationId,
    ) -> Option<(ArchivedStation, Vec<ArchivedStation>)> {
        let indices = self.subtree(top);
        if indices.is_empty() {
            return None;
        }
        let mut taken = Vec::with_capacity(indices.len());
        let mut kept = Vec::with_capacity(self.archived.len() - indices.len());
        let mut next = indices.iter().peekable();
        for (i, record) in std::mem::take(&mut self.archived).into_iter().enumerate() {
            if next.peek() == Some(&&i) {
                next.next();
                taken.push(record);
            } else {
                kept.push(record);
            }
        }
        self.archived = kept;
        let at = taken.iter().position(|r| r.id == top)?;
        let mut head = taken.remove(at);
        head.into = None;
        for r in &mut taken {
            if r.into == Some(top) {
                r.into = None;
            }
        }
        Some((head, taken))
    }

    /// Archive `gone` whole in this station (RS 4.1 step 2, the merge's
    /// fold): its records move here under its identity (those it held
    /// under its own identity now name it), then its own identity as a
    /// record naming `native` (its native lines). When `native` is empty the
    /// record is history and owns nothing: its records stay under this
    /// station's identity.
    pub fn archive_whole(&mut self, gone: Station, native: Vec<LineId>) {
        let record = gone.as_record(native);
        let owner = (!record.is_history()).then_some(gone.id);
        self.archived.extend(gone.archived.into_iter().map(|mut r| {
            if r.into.is_none() {
                r.into = owner;
            }
            r
        }));
        self.archived.push(record);
    }

    /// Give this station the identity of its record `id` (a merge keeping
    /// the moving station, RS 4.1 step 3; a restore, RS 4.3; Combine's
    /// forced join keep): the record leaves the list and the records under
    /// it come under the station's identity; the old identity becomes a
    /// record naming `old_lines`, pushed last, which owns the station's
    /// other records (none when it is history). The id is the caller's to
    /// re-key. Returns the record taken, whose name, comment and external id
    /// the station now has.
    pub fn swap_identity(
        &mut self,
        id: StationId,
        old_lines: Vec<LineId>,
    ) -> Option<ArchivedStation> {
        if !self.archived.iter().any(|r| r.id == id) {
            return None;
        }
        let old = self.as_record(old_lines);
        if !old.is_history() {
            for r in &mut self.archived {
                if r.into.is_none() && r.id != id {
                    r.into = Some(self.id);
                }
            }
        }
        let record = self.take_record(id)?;
        self.name.clone_from(&record.name);
        self.comment.clone_from(&record.comment);
        self.external.clone_from(&record.external);
        self.archived.push(old);
        Some(record)
    }

    /// A5 (a history record owns nothing): every record archived under a
    /// history record moves up to the nearest owner that is not history, or
    /// under the station's identity; an `into` that names no record of the
    /// station is unset too (nothing a command does leaves one, but a record
    /// a command took out must not strand its subtree). Run by every
    /// epilogue after record lines go stale. Returns how many moved.
    pub fn settle_record_owners(&mut self) -> usize {
        if self.archived.iter().all(|r| r.into.is_none()) {
            return 0;
        }
        let index = index_of(&self.archived);
        let owners: Vec<Option<StationId>> = self.archived.iter().map(|r| r.into).collect();
        let mut moved = 0;
        for (i, &owner) in owners.iter().enumerate() {
            // Climb past history (and missing) owners, at most once round
            // the list, so a cycle cannot hold it.
            let mut current = owner;
            let mut steps = 0;
            while let Some(o) = current {
                match index.get(&o) {
                    Some(&at) if at != i && !self.archived[at].is_history() => break,
                    Some(&at) if at != i && steps < owners.len() => {
                        current = owners[at];
                        steps += 1;
                    }
                    _ => current = None,
                }
            }
            if current != owner {
                self.archived[i].into = current;
                moved += 1;
            }
        }
        moved
    }

    /// Put every record directly under this station's identity (the load
    /// of an earlier version 2 file, RS 8.2: U1 alias rows have no owner).
    pub fn flatten_records(&mut self) {
        for record in &mut self.archived {
            record.into = None;
        }
    }
}
