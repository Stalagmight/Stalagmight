use std::sync::Arc;

use chrono::{DateTime, Utc};

use super::{Command, PersistStack, StackSlot, UndoData, UndoEntry, UndoStack};
use crate::models::{Project, SectionId, ShotId};
use crate::persistence::undo_limits::UNDO_BYTE_BUDGET_BYTES;
use crate::persistence::{compress_history_json, StoredHistoryRow};

/// What [`UndoData::shot_target`] hands back: the section, the target's uuid if
/// the entry records one, and the in-session id it was recorded with.
type ShotTarget<'a> = (SectionId, Option<&'a str>, ShotId);

#[cfg(test)]
thread_local! {
    /// How many entries [`UndoStack::stored_rows_to_persist`] has compressed on
    /// this thread - what a test reads to tell a cached row from a fresh one.
    pub(crate) static ENTRIES_COMPRESSED: std::cell::Cell<usize> =
        const { std::cell::Cell::new(0) };
}

impl UndoStack {
    /// [#303] Every row a save must write for one stack, in stack order and in the
    /// tuple shape `Database::replace_history_stacks_with_timestamps` takes per stack.
    ///
    /// EVERY save writes the whole (windowed) stack. That is forced by the save
    /// pipeline: `save_project` builds a fresh `.stalx.tmp`, renames it over the
    /// target, and only then writes the undo tables into the reopened file.
    ///
    /// `count_limit` is the persisted window: `Some(n)` keeps the last n entries,
    /// and of those [#576 B20] only the newest that fit in
    /// [`UNDO_BYTE_BUDGET_BYTES`] (see [`Self::persist_window`]); `None` keeps all
    /// byte-budgeted Full-mode entries.
    #[must_use]
    pub fn rows_to_persist(
        &self,
        stack: PersistStack,
        count_limit: Option<usize>,
    ) -> Vec<(String, String, Option<String>)> {
        self.persist_window(stack, count_limit)
            .filter_map(|slot| Self::serialize_entry_with_timestamp(slot))
            .collect()
    }

    /// [`Self::rows_to_persist`], compressed - the rows exactly as
    /// `Database::replace_history_stacks_stored` writes them, byte for byte what
    /// `replace_history_stacks_with_timestamps` would store for the JSON rows.
    ///
    /// [#576 B20] Each entry is serialized and gzipped ONCE: the result is kept
    /// beside the entry ([`StackSlot`]) and every later save reuses it, so a save
    /// compresses only the entries that are new since the last one - instead of
    /// re-gzipping every whole-project snapshot pair in the window on every
    /// autosave. An entry that moves between the stacks (undo / redo) or is changed
    /// is compressed again on the next save.
    #[must_use]
    pub fn stored_rows_to_persist(
        &self,
        stack: PersistStack,
        count_limit: Option<usize>,
    ) -> Vec<StoredHistoryRow> {
        self.persist_window(stack, count_limit)
            .filter_map(|slot| {
                let (command, undo_data) = slot
                    .stored
                    .get_or_init(|| {
                        #[cfg(test)]
                        ENTRIES_COMPRESSED.with(|n| n.set(n.get() + 1));
                        Self::serialize_entry(&slot.entry).map(|(command, undo_data)| {
                            (
                                Arc::from(compress_history_json(&command)),
                                Arc::from(compress_history_json(&undo_data)),
                            )
                        })
                    })
                    .as_ref()?;
                Some(StoredHistoryRow {
                    command: Arc::clone(command),
                    undo_data: Arc::clone(undo_data),
                    timestamp: slot.timestamp.map(|t| t.to_rfc3339()),
                })
            })
            .collect()
    }

    /// The entries a save writes for one stack, oldest first.
    ///
    /// `Some(n)` (the count window: a local project, Disabled mode) takes the
    /// newest n, and then [#576 B20] only the newest of those whose estimated sizes
    /// add up to at most [`UNDO_BYTE_BUDGET_BYTES`] - the newest always kept, even
    /// alone over it - the same budget, measure and rule a Full-mode stack is held
    /// to in memory. Without it a save re-wrote (and re-gzipped) every whole-project
    /// snapshot pair of the window: ~340 MB of JSON per stack on a 20,000-shot
    /// project. `None` (Full mode) is the whole stack, already bounded by that
    /// budget in memory.
    fn persist_window(
        &self,
        stack: PersistStack,
        count_limit: Option<usize>,
    ) -> impl Iterator<Item = &StackSlot> {
        let entries = match stack {
            PersistStack::Undo => &self.undo_stack,
            PersistStack::Redo => &self.redo_stack,
        };

        let skip = match count_limit {
            Some(limit) => {
                let mut kept = 0usize;
                let mut bytes = 0usize;
                for slot in entries.iter().rev().take(limit) {
                    let size = Self::estimated_size(slot);
                    if kept > 0 && bytes.saturating_add(size) > UNDO_BYTE_BUDGET_BYTES {
                        break;
                    }
                    bytes = bytes.saturating_add(size);
                    kept += 1;
                }
                entries.len() - kept
            }
            None => 0,
        };

        entries.iter().skip(skip)
    }

    /// Serialize an entry to JSON.
    pub fn serialize_entry(entry: &UndoEntry) -> Option<(String, String)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = serde_json::to_string(&entry.undo_data).ok()?;
        Some((command_json, undo_data_json))
    }

    /// Serialize an entry with its timestamp for database persistence.
    pub fn serialize_entry_with_timestamp(
        entry: &UndoEntry,
    ) -> Option<(String, String, Option<String>)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = serde_json::to_string(&entry.undo_data).ok()?;
        let timestamp_str = entry.timestamp.map(|t| t.to_rfc3339());
        Some((command_json, undo_data_json, timestamp_str))
    }

    /// Deserialize a legacy entry without a timestamp.
    pub fn deserialize_entry(command_json: &str, undo_data_json: &str) -> Option<UndoEntry> {
        let command = Self::deserialize_command(command_json)?;
        let undo_data: UndoData = serde_json::from_str(undo_data_json).ok()?;
        Some(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command,
            undo_data,
            timestamp: None,
        })
    }

    /// Deserialize an entry with an optional timestamp.
    pub fn deserialize_entry_with_timestamp(
        command_json: &str,
        undo_data_json: &str,
        timestamp_str: Option<&str>,
    ) -> Option<UndoEntry> {
        let command = Self::deserialize_command(command_json)?;
        let undo_data: UndoData = serde_json::from_str(undo_data_json).ok()?;
        let timestamp = match timestamp_str {
            Some(raw) => Some(DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc)),
            None => None,
        };
        Some(UndoEntry {
            replayed_at: None,
            command_bytes: None,
            command,
            undo_data,
            timestamp,
        })
    }

    fn deserialize_command(command_json: &str) -> Option<Command> {
        match serde_json::from_str(command_json) {
            Ok(command) => Some(command),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    purpose = "load persisted undo history",
                    "Skipping an undo entry whose command could not be deserialized; its undo step is dropped"
                );
                None
            }
        }
    }

    /// True when `undo()` can actually apply this entry.
    fn is_applicable(entry: &UndoEntry) -> bool {
        !matches!(entry.undo_data, UndoData::None)
    }

    /// Drop restored entries whose shot target no longer exists - from BOTH
    /// stacks - and return how many went in total.
    ///
    /// The companion to [`Self::is_applicable`] for the one thing that check
    /// cannot see from the entry alone: whether the shot the restore addresses
    /// is still there. An entry that cannot resolve its target fails, and the
    /// epilogue of `undo_inner`/`redo_inner` pushes a failed entry straight
    /// back onto the stack it came from - so keeping one costs every step
    /// BENEATH it, forever (#413, finding 13), while dropping it costs exactly
    /// one. This is what makes a legacy stale-id entry (persisted before the
    /// uuid fields existed) a lost step rather than a wedged history.
    ///
    /// Both stacks, because a `.stalx` persists both and the redo side reaches
    /// the same dead end through the same resolver: `redo_inner`'s catch-all
    /// re-executes through `AppState::command_with_resolved_shot`, which fails
    /// on an unresolvable uuid exactly as the undo side does. What differs is
    /// WHICH entries the question applies to and where each scan has to stop -
    /// see [`Self::drop_unresolvable_redo_entries`].
    pub fn drop_entries_addressing_a_missing_shot(&mut self, project: &Project) -> usize {
        self.drop_unresolvable_undo_entries(project) + self.drop_unresolvable_redo_entries(project)
    }

    /// Does `project` still hold the shot a `shot_target` names?
    ///
    /// The ONE rule both stacks are filtered by, in the order
    /// `AppState::resolve_undo_shot` resolves in - which is what makes this
    /// predicate agree with the failure it exists to prevent.
    ///
    /// The legacy (uuid-less) arm answers a strictly weaker question, and it
    /// cannot do better: `Shot::id` is in-session only (the loader renumbers),
    /// so a stale recorded id can COLLIDE with a live shot that is not the one
    /// the entry was written against. Such an entry passes this filter and its
    /// undo edits the wrong shot. That is inherent to an entry that recorded no
    /// identity - there is nothing left to distinguish the two - and it is why
    /// the uuid arm exists; this filter only promises to drop the entries that
    /// would WEDGE the history, not to catch every legacy misfire.
    fn shot_target_resolves(target: ShotTarget<'_>, project: &Project) -> bool {
        let (section_id, uuid, recorded_id) = target;
        match uuid {
            Some(uuid) => project.shot_id_by_uuid(section_id, uuid).is_some(),
            // A legacy entry addresses by id, so that is what has to hold.
            None => project
                .get_section(section_id)
                .is_some_and(|section| section.shots.iter().any(|s| s.id == recorded_id)),
        }
    }

    /// How many dropped-entry descriptions the warning both scans emit carries.
    ///
    /// The `count` field beside them is the total, so a sample loses nothing
    /// that matters - and the field it feeds is one unbounded string built from
    /// a file this app did not write. A `.stalx` holding 100k unresolvable
    /// entries produced a single multi-megabyte log record.
    const DROPPED_SAMPLE: usize = 10;

    /// Longest uuid [`Self::describe_dropped`] will print. A uuid is 36 chars;
    /// anything past this is not one, and there is no reason to log it whole.
    const DROPPED_UUID_CHARS: usize = 64;

    /// How a dropped entry is named in the warning both scans emit.
    ///
    /// The uuid is file-controlled - a persisted entry can carry any string at
    /// all, from any writer - and this goes verbatim into a log record, so it
    /// gets the same guard the import receipts use: control characters (a
    /// newline forges a log line of its own) and bidi overrides dropped, then
    /// clamped on a char boundary.
    fn describe_dropped(entry: &UndoEntry, target: ShotTarget<'_>) -> String {
        let (section_id, uuid, recorded_id) = target;
        let shot = uuid.map_or_else(
            || recorded_id.to_string(),
            |u| crate::core::command::sanitize_display_text(u, Self::DROPPED_UUID_CHARS),
        );
        format!(
            "{} (section {section_id}, shot {shot})",
            entry.command.description(),
        )
    }

    /// The `entries` field both scans log: the first [`Self::DROPPED_SAMPLE`]
    /// descriptions, with the elided remainder named rather than silently cut.
    fn dropped_sample(dropped: &[String]) -> String {
        let mut sample = dropped
            .iter()
            .take(Self::DROPPED_SAMPLE)
            .cloned()
            .collect::<Vec<_>>()
            .join("; ");
        if let Some(elided) = dropped
            .len()
            .checked_sub(Self::DROPPED_SAMPLE)
            .filter(|n| *n > 0)
        {
            sample.push_str(&format!("; ... and {elided} more"));
        }
        sample
    }

    /// The undo-stack half.
    ///
    /// Deliberately a TOP-DOWN scan that STOPS at the first entry whose undo
    /// restores recorded state wholesale
    /// ([`UndoData::restores_recorded_state`]). Such an entry is a wall: it puts
    /// a whole project or section back, in the id space it was recorded in, so
    /// a target that does not resolve against the project as loaded may resolve
    /// perfectly once the walk gets there. Above the wall the question is
    /// answerable, because every live path that REMOVES a shot records a
    /// `ProjectSnapshotPair` - itself a wall - so no entry above one can be
    /// addressing a shot that something above it deleted.
    fn drop_unresolvable_undo_entries(&mut self, project: &Project) -> usize {
        let mut doomed: Vec<usize> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for (index, entry) in self.undo_stack.iter().enumerate().rev() {
            if entry.undo_data.restores_recorded_state() {
                break;
            }
            let Some(target) = entry.undo_data.shot_target() else {
                continue;
            };
            if !Self::shot_target_resolves(target, project) {
                doomed.push(index);
                dropped.push(Self::describe_dropped(entry, target));
            }
        }
        if doomed.is_empty() {
            return 0;
        }
        // Descending, because the scan ran newest-first - so each removal
        // leaves the lower indices where they were.
        for index in doomed {
            if let Some(entry) = self.undo_stack.remove(index) {
                self.undo_bytes = self.undo_bytes.saturating_sub(Self::estimated_size(&entry));
            }
        }
        tracing::warn!(
            count = dropped.len(),
            entries = %Self::dropped_sample(&dropped),
            purpose = "load persisted undo history",
            "Dropped persisted undo entries whose shot no longer exists; each would have failed \
             and been pushed back, blocking every older entry"
        );
        dropped.len()
    }

    /// The redo-stack half - same resolvability rule, a narrower scan.
    ///
    /// Without it, Ctrl+Y is the wedge the undo-side filter was built to
    /// prevent, one stack over: `redo_inner` pops the entry, re-resolves its
    /// shot, fails, and its epilogue pushes the entry back onto the redo stack -
    /// where it blocks every redo beneath it for the life of the project.
    ///
    /// Scanned back-to-front, the order `pop_redo` takes entries in, and it
    /// covers the run of FIELD EDITS at the top: an entry the redo re-executes
    /// as an `UpdateShot`/`SetShotKind` ([`UndoData::redo_shot_target`]). The
    /// stop rule is what that buys - such a redo changes a shot's fields and
    /// nothing else, so every entry in the run sees exactly the shot population
    /// the project was loaded with, and "is the target there" is answerable for
    /// all of them at once. The first entry that is anything else can add shots
    /// (a snapshot restore, a re-created shot, an arbitrary re-executed
    /// command), so nothing beneath it can be judged and the scan stops - the
    /// same conservatism as the undo side's wall.
    fn drop_unresolvable_redo_entries(&mut self, project: &Project) -> usize {
        let mut doomed: Vec<usize> = Vec::new();
        let mut dropped: Vec<String> = Vec::new();
        for (index, entry) in self.redo_stack.iter().enumerate().rev() {
            let Some(target) = entry.undo_data.redo_shot_target() else {
                break;
            };
            if !Self::shot_target_resolves(target, project) {
                doomed.push(index);
                dropped.push(Self::describe_dropped(entry, target));
            }
        }
        if doomed.is_empty() {
            return 0;
        }
        // Descending, as on the undo side: the scan ran back-to-front, so each
        // removal leaves the lower indices where they were.
        for index in doomed {
            if let Some(entry) = self.redo_stack.remove(index) {
                self.redo_bytes = self.redo_bytes.saturating_sub(Self::estimated_size(&entry));
            }
        }
        tracing::warn!(
            count = dropped.len(),
            entries = %Self::dropped_sample(&dropped),
            purpose = "load persisted redo history",
            "Dropped persisted redo entries whose shot no longer exists; each would have failed \
             and been pushed back, blocking every redo beneath it"
        );
        dropped.len()
    }

    fn warn_dropped_unapplicable(dropped: &[String]) {
        if dropped.is_empty() {
            return;
        }
        tracing::warn!(
            count = dropped.len(),
            commands = %dropped.join("; "),
            purpose = "load persisted undo history",
            "Dropped persisted undo entries that carry no undo data; they could not be undone and would have blocked every older entry"
        );
    }

    /// Validate the raw undo timestamp column before any rows are filtered.
    /// Legacy `NULL`s may only be a prefix, followed by parseable non-decreasing stamps.
    fn undo_timestamps_are_valid(entries: &[(String, String, Option<String>)]) -> bool {
        let mut previous: Option<DateTime<Utc>> = None;
        let mut saw_timestamp = false;
        for (_, _, raw) in entries {
            let Some(raw) = raw else {
                if saw_timestamp {
                    return false;
                }
                continue;
            };
            let Ok(stamp) = DateTime::parse_from_rfc3339(raw) else {
                return false;
            };
            let stamp = stamp.with_timezone(&Utc);
            if previous.is_some_and(|prev| stamp < prev) {
                return false;
            }
            previous = Some(stamp);
            saw_timestamp = true;
        }
        true
    }

    fn timestamps_are_parseable(entries: &[(String, String, Option<String>)]) -> bool {
        entries.iter().all(|(_, _, raw)| {
            raw.as_deref()
                .is_none_or(|stamp| DateTime::parse_from_rfc3339(stamp).is_ok())
        })
    }

    /// Load legacy persisted undo data.
    pub fn load_from_persisted(&mut self, entries: Vec<(String, String)>) {
        self.clear();

        let mut dropped: Vec<String> = Vec::new();
        for (command_json, undo_data_json) in entries {
            if let Some(entry) = Self::deserialize_entry(&command_json, &undo_data_json) {
                if Self::is_applicable(&entry) {
                    self.push_undo_tracked(entry);
                } else {
                    dropped.push(entry.command.description());
                }
            }
        }
        Self::warn_dropped_unapplicable(&dropped);
        self.enforce_depth_limit();
    }

    /// Load persisted undo and redo data with timestamps.
    pub fn load_from_persisted_with_timestamps(
        &mut self,
        undo_entries: Vec<(String, String, Option<String>)>,
        redo_entries: Vec<(String, String, Option<String>)>,
    ) {
        self.clear();

        if !Self::undo_timestamps_are_valid(&undo_entries) {
            tracing::warn!(
                entries = undo_entries.len(),
                purpose = "refuse invalid persisted undo timestamps (#303)",
                "loaded undo history has malformed, non-monotonic, or misplaced missing \
                 timestamps; dropping it rather than risk an incorrect push cutoff"
            );
        } else {
            let mut dropped: Vec<String> = Vec::new();
            for (command_json, undo_data_json, timestamp_str) in undo_entries {
                if let Some(entry) = Self::deserialize_entry_with_timestamp(
                    &command_json,
                    &undo_data_json,
                    timestamp_str.as_deref(),
                ) {
                    if Self::is_applicable(&entry) {
                        self.push_undo_tracked(entry);
                    } else {
                        dropped.push(entry.command.description());
                    }
                }
            }
            Self::warn_dropped_unapplicable(&dropped);

            // Stack order is undo order, so a non-monotonic timestamp sequence cannot be
            // sorted safely. Drop it rather than letting the push cutoff delete an arbitrary
            // prefix.
            if !Self::stamps_ascend(&self.undo_stack) {
                tracing::warn!(
                    entries = self.undo_stack.len(),
                    purpose = "refuse a non-monotonic undo stack (#303)",
                    "loaded undo history is not in time order; dropping it rather than risk \
                 deleting the wrong entries on the next push"
                );
                self.clear_undo();
            }
        }

        // A redo entry with `UndoData::None` still re-executes correctly, so the redo
        // side is intentionally not filtered by `is_applicable`.
        if !Self::timestamps_are_parseable(&redo_entries) {
            tracing::warn!(
                entries = redo_entries.len(),
                purpose = "refuse invalid persisted redo timestamps (#303)",
                "loaded redo history has a malformed timestamp; dropping it"
            );
        } else {
            for (command_json, undo_data_json, timestamp_str) in redo_entries {
                if let Some(entry) = Self::deserialize_entry_with_timestamp(
                    &command_json,
                    &undo_data_json,
                    timestamp_str.as_deref(),
                ) {
                    self.push_redo_tracked(entry);
                }
            }
        }

        self.enforce_depth_limit();
    }
}
