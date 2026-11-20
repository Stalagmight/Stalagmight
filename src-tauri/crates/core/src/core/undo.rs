//! The undo and redo stacks of the station-graph model (plan 8,
//! `docs/undo-redo.md`).
//!
//! Every step is one [`UndoEntry`]: a [`HistoryEntry`] (time, kind, the label
//! worded at record time, stable refs, recorded preview rows, and the
//! id-keyed [`Changeset`](crate::models::survey::Changeset)) plus its
//! persisted text. One entry serves both stacks: undo steps its changeset
//! back, redo forward, and nothing is re-encoded on a step.
//!
//! **How an entry is held (C0 decision).** The stack keeps the decoded
//! `HistoryEntry` AND its canonical text:
//! - the changeset of an entry captured in this session carries the session
//!   shot ids of every line it touches (in memory only, U8), so an in-session
//!   undo or redo gives every shot its id back exactly; an entry decoded from
//!   a file has none and its steps mint ids from the line allocators;
//! - the text is `encode_entry(entry)`, computed once at push (or read from
//!   the file), so the byte budget counts exactly what a save writes and what
//!   a push carries, and a save copies the text instead of re-encoding the
//!   whole history every autosave.
//!
//! The heap a changeset takes is a small multiple of its text (not measured);
//! the budget bounds the persisted size, which is what the 4 MiB Full-mode
//! ceiling (plan 8.3) and the 5 MiB push cap are about.

use std::collections::VecDeque;

use chrono::{DateTime, Utc};

use crate::models::survey::changeset::{decode_entry, encode_entry, EntryError, EntryLimits};
use crate::models::survey::HistoryEntry;
use crate::persistence::stalx_v2::{History, RawJson};

pub mod history_item;

pub use history_item::{command_kind, FieldChange, PreviewData, UndoHistoryItem};

/// The Full-mode history budget of a SpeleoDB-linked project: at most this
/// many bytes of persisted entries (both stacks), oldest evicted first. Plan
/// 8.3 lowered it from 32 MiB (sized for gzip in SQLite) to 4 MiB of JSON.
pub const UNDO_BYTE_BUDGET_BYTES: usize = 4 * 1024 * 1024;

/// The active depth bound of the stacks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthLimit {
    /// At most N undo entries (and N redo entries), oldest evicted first.
    /// Local projects and Disabled mode.
    Count(usize),
    /// At most N bytes of persisted entries over both stacks, oldest undo
    /// entries evicted first, then the furthest redo entries. Full mode on a
    /// SpeleoDB-linked project.
    Bytes(usize),
}

/// Why a persisted history was dropped whole on load (the banner says which).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDropped {
    /// Written by an earlier history format (or the SQLite format: every
    /// migrated legacy file drops its history, plan 7.2.8).
    EarlierVersion,
    /// An entry this build could not read (a later build's, or damaged).
    Unreadable,
}

impl HistoryDropped {
    fn of(error: &EntryError) -> Self {
        match error {
            EntryError::Version(found)
                if *found < u64::from(crate::models::survey::changeset::HISTORY_FORMAT_VERSION) =>
            {
                Self::EarlierVersion
            }
            _ => Self::Unreadable,
        }
    }
}

/// One step of the history.
#[derive(Debug, Clone)]
pub struct UndoEntry {
    entry: HistoryEntry,
    raw: RawJson,
}

impl UndoEntry {
    /// Encode `entry` (the text a save writes).
    pub fn new(entry: HistoryEntry) -> Result<Self, EntryError> {
        let text = encode_entry(&entry)?;
        let raw = RawJson::parse(&text).map_err(EntryError::Syntax)?;
        Ok(Self { entry, raw })
    }

    /// Decode an entry a file holds.
    pub fn decode(raw: &RawJson, limits: &EntryLimits) -> Result<Self, EntryError> {
        let entry = decode_entry(raw.as_str(), limits)?;
        Ok(Self {
            entry,
            raw: raw.clone(),
        })
    }

    pub fn entry(&self) -> &HistoryEntry {
        &self.entry
    }

    pub fn raw(&self) -> &RawJson {
        &self.raw
    }

    /// The persisted size, what the byte budget counts.
    pub fn bytes(&self) -> usize {
        self.raw.as_str().len()
    }

    pub fn timestamp(&self) -> DateTime<Utc> {
        self.entry.timestamp
    }
}

/// The undo and redo stacks. Each is a stack whose BACK is the next step
/// (the file's order, `History`).
#[derive(Debug)]
pub struct UndoStack {
    undo: VecDeque<UndoEntry>,
    redo: VecDeque<UndoEntry>,
    undo_bytes: usize,
    redo_bytes: usize,
    /// The configured entry-count window (`general.undo_history_limit`).
    history_limit: usize,
    depth_limit: DepthLimit,
    /// The inherited-history watermark (#303): a new entry is stamped after
    /// it, so the push's cut of inherited entries never takes it.
    inherited_through: Option<DateTime<Utc>>,
}

impl Default for UndoStack {
    fn default() -> Self {
        Self::new_with_limit(100)
    }
}

impl UndoStack {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn new_with_limit(history_limit: usize) -> Self {
        Self {
            undo: VecDeque::new(),
            redo: VecDeque::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            history_limit,
            depth_limit: DepthLimit::Count(history_limit),
            inherited_through: None,
        }
    }

    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        if matches!(self.depth_limit, DepthLimit::Count(_)) {
            self.depth_limit = DepthLimit::Count(limit);
        }
        self.enforce_depth_limit();
    }

    pub fn set_byte_budget(&mut self, budget: usize) {
        self.depth_limit = DepthLimit::Bytes(budget);
        self.enforce_depth_limit();
    }

    pub fn use_count_limit(&mut self) {
        self.depth_limit = DepthLimit::Count(self.history_limit);
        self.enforce_depth_limit();
    }

    pub fn depth_limit(&self) -> DepthLimit {
        self.depth_limit
    }

    pub fn set_inherited_through(&mut self, instant: Option<DateTime<Utc>>) {
        self.inherited_through = instant;
    }

    pub fn inherited_through(&self) -> Option<DateTime<Utc>> {
        self.inherited_through
    }

    /// The next entry's time: now, but strictly after the newest undo entry
    /// and the inherited watermark (timestamps strictly increase).
    pub fn next_stamp(&self) -> DateTime<Utc> {
        let now = Utc::now();
        let floor = match (self.newest_stamp(), self.inherited_through) {
            (Some(prev), Some(mark)) => Some(prev.max(mark)),
            (prev, mark) => prev.or(mark),
        };
        match floor {
            Some(prev) if prev >= now => prev + chrono::Duration::nanoseconds(1),
            _ => now,
        }
    }

    pub fn newest_stamp(&self) -> Option<DateTime<Utc>> {
        self.undo.back().map(UndoEntry::timestamp)
    }

    /// Record a new step: stamp it, encode it, push it, clear the redo
    /// branch, enforce the bound. On an encode failure nothing changes.
    pub fn record(&mut self, mut entry: HistoryEntry) -> Result<(), EntryError> {
        entry.timestamp = self.next_stamp();
        let entry = UndoEntry::new(entry)?;
        self.push_undo_tracked(entry);
        self.clear_redo();
        self.enforce_depth_limit();
        Ok(())
    }

    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        let entry = self.undo.pop_back()?;
        self.undo_bytes = self.undo_bytes.saturating_sub(entry.bytes());
        Some(entry)
    }

    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        let entry = self.redo.pop_back()?;
        self.redo_bytes = self.redo_bytes.saturating_sub(entry.bytes());
        Some(entry)
    }

    /// Put an entry back on (or onto) the undo stack.
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.push_undo_tracked(entry);
        self.enforce_depth_limit();
    }

    pub fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_bytes = self.redo_bytes.saturating_add(entry.bytes());
        self.redo.push_back(entry);
        self.enforce_depth_limit();
    }

    fn push_undo_tracked(&mut self, entry: UndoEntry) {
        self.undo_bytes = self.undo_bytes.saturating_add(entry.bytes());
        self.undo.push_back(entry);
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.undo_bytes = 0;
        self.clear_redo();
    }

    pub fn clear_redo(&mut self) {
        self.redo.clear();
        self.redo_bytes = 0;
    }

    /// Drop every undo entry stamped at or before `cutoff` (the SpeleoDB
    /// clear-on-push and the inherited cut) and the redo branch. Returns how
    /// many undo entries went.
    pub fn drop_entries_up_to(&mut self, cutoff: DateTime<Utc>) -> usize {
        let keep_from = self.undo.partition_point(|e| e.timestamp() <= cutoff);
        let dropped: usize = self.undo.drain(..keep_from).map(|e| e.bytes()).sum();
        self.undo_bytes = self.undo_bytes.saturating_sub(dropped);
        self.clear_redo();
        keep_from
    }

    pub fn peek_undo(&self) -> Option<&UndoEntry> {
        self.undo.back()
    }

    pub fn peek_redo(&self) -> Option<&UndoEntry> {
        self.redo.back()
    }

    pub fn len(&self) -> usize {
        self.undo.len()
    }

    pub fn is_empty(&self) -> bool {
        self.undo.is_empty()
    }

    pub fn redo_len(&self) -> usize {
        self.redo.len()
    }

    /// Persisted bytes of both stacks.
    pub fn bytes(&self) -> usize {
        self.undo_bytes.saturating_add(self.redo_bytes)
    }

    /// Undo entries, most recent first (index 0 is the next undo).
    pub fn undo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.undo.iter().rev()
    }

    /// Redo entries, next redo first.
    pub fn redo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.redo.iter().rev()
    }

    /// Evict the oldest entries past the active bound.
    ///
    /// Under `Bytes`, the newest entry of each stack is always kept even if
    /// it alone exceeds the budget, as 0.6.0 did: dropping it would silently
    /// discard the operation the user just did (and, with it, every older
    /// entry), and a single entry over the budget is a very large import, not
    /// runaway growth.
    pub fn enforce_depth_limit(&mut self) {
        match self.depth_limit {
            DepthLimit::Count(limit) => {
                while self.undo.len() > limit {
                    if let Some(e) = self.undo.pop_front() {
                        self.undo_bytes = self.undo_bytes.saturating_sub(e.bytes());
                    }
                }
                while self.redo.len() > limit {
                    if let Some(e) = self.redo.pop_front() {
                        self.redo_bytes = self.redo_bytes.saturating_sub(e.bytes());
                    }
                }
            }
            DepthLimit::Bytes(budget) => {
                let mut dropped = 0_usize;
                while self.bytes() > budget {
                    let popped = if self.undo.len() > 1 {
                        self.undo.pop_front().inspect(|e| {
                            self.undo_bytes = self.undo_bytes.saturating_sub(e.bytes());
                        })
                    } else if self.redo.len() > 1 {
                        self.redo.pop_front().inspect(|e| {
                            self.redo_bytes = self.redo_bytes.saturating_sub(e.bytes());
                        })
                    } else {
                        None
                    };
                    if popped.is_none() {
                        break;
                    }
                    dropped += 1;
                }
                if dropped > 0 {
                    tracing::info!(
                        dropped,
                        undo_remaining = self.undo.len(),
                        redo_remaining = self.redo.len(),
                        bytes = self.bytes(),
                        budget,
                        purpose = "bound a Full-mode SpeleoDB project's undo history",
                        "Trimmed the oldest undo entries to stay within the undo byte budget"
                    );
                }
            }
        }
    }

    /// The file's `history` member: both stacks in file order (each list's
    /// last element is the next step).
    pub fn persisted(&self) -> History {
        self.persisted_window(None)
    }

    /// [`Self::persisted`] keeping at most the newest `count` entries of
    /// each stack (`None`: all of them) - the persisted window
    /// (`general.undo_persist_limit`).
    pub fn persisted_window(&self, count: Option<usize>) -> History {
        let newest = |list: &VecDeque<UndoEntry>| -> Vec<RawJson> {
            let skip = count.map_or(0, |n| list.len().saturating_sub(n));
            list.iter().skip(skip).map(|e| e.raw.clone()).collect()
        };
        History {
            undo: newest(&self.undo),
            redo: newest(&self.redo),
        }
    }

    /// Replace both stacks with a file's history. Any entry that does not
    /// decode drops the whole history (both stacks stay empty) and says why;
    /// the bound is enforced afterwards.
    pub fn load_persisted(
        &mut self,
        history: &History,
        limits: &EntryLimits,
    ) -> Result<(), HistoryDropped> {
        self.clear();
        let decode_all = |list: &[RawJson]| -> Result<VecDeque<UndoEntry>, HistoryDropped> {
            list.iter()
                .map(|raw| {
                    UndoEntry::decode(raw, limits).map_err(|e| {
                        tracing::warn!(error = ?e, "an undo history entry could not be read; dropping the history");
                        HistoryDropped::of(&e)
                    })
                })
                .collect()
        };
        let undo = decode_all(&history.undo)?;
        let redo = decode_all(&history.redo)?;
        self.undo_bytes = undo.iter().map(UndoEntry::bytes).sum();
        self.redo_bytes = redo.iter().map(UndoEntry::bytes).sum();
        self.undo = undo;
        self.redo = redo;
        self.enforce_depth_limit();
        Ok(())
    }
}
