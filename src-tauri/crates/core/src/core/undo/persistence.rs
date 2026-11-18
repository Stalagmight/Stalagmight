use chrono::{DateTime, Utc};

use super::{Command, PersistStack, UndoData, UndoEntry, UndoStack};
use crate::models::{Project, SectionId, ShotId};

/// What [`UndoData::shot_target`] hands back: the section, the target's uuid if
/// the entry records one, and the in-session id it was recorded with.
type ShotTarget<'a> = (SectionId, Option<&'a str>, ShotId);

/// The envelope every persisted undo payload is written in:
/// `{"v2": <UndoData>}`.
///
/// It marks the compact history format ([`UndoData::Delta`]), and it is what
/// keeps an OLDER build safe. A build before it reads `undo_data_json` as a
/// bare externally tagged `UndoData`, skips a row it cannot parse - silently -
/// and keeps every other row. A compact entry is a delta against the state the
/// entry above it leaves, so a history with one entry skipped undoes the rest
/// onto the wrong project. Under this key NO row parses there (an unknown
/// variant), so an older build opens the file with an empty history and the
/// survey intact, and its next save rewrites the tables in its own format.
///
/// In the other direction this build keeps nothing it cannot read as the
/// envelope: a history written before it (whole-project snapshot pairs) is
/// dropped on open, both stacks, by decision (the snapshot pairs are what made
/// large files large).
/// Why a loaded history was dropped whole (see `drop_history_with_a_hole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoryDropped {
    /// Rows written by a build before compact entries: no `{"v2": ...}` envelope.
    EarlierVersion,
    /// A row in this format that could not be read.
    Unreadable,
}

/// Tally of the rows a load could not read, by why.
#[derive(Default)]
struct Unread {
    earlier: usize,
    damaged: usize,
}

impl Unread {
    fn note(&mut self, undo_data_json: &str) {
        if undo_data_json.trim_start().starts_with("{\"v2\":") {
            self.damaged += 1;
        } else {
            self.earlier += 1;
        }
    }

    fn total(&self) -> usize {
        self.earlier + self.damaged
    }

    fn reason(&self) -> Option<HistoryDropped> {
        if self.damaged > 0 {
            Some(HistoryDropped::Unreadable)
        } else if self.earlier > 0 {
            Some(HistoryDropped::EarlierVersion)
        } else {
            None
        }
    }
}

#[derive(serde::Serialize)]
struct PersistedOut<'a> {
    #[serde(rename = "v2")]
    data: &'a UndoData,
}

#[derive(serde::Deserialize)]
struct PersistedIn {
    #[serde(rename = "v2")]
    data: UndoData,
}

/// A persisted payload, or `None` when it is not one this format wrote: a
/// row from an older build, or a corrupt one.
fn parse_undo_data(undo_data_json: &str) -> Option<UndoData> {
    serde_json::from_str::<PersistedIn>(undo_data_json)
        .ok()
        .map(|row| row.data)
}

fn write_undo_data(undo_data: &UndoData) -> Option<String> {
    serde_json::to_string(&PersistedOut { data: undo_data }).ok()
}

impl UndoStack {
    /// [#303] Every row a save must write for one stack, in stack order and in the
    /// tuple shape `Database::save_undo_stack_with_timestamps` takes.
    ///
    /// EVERY save writes the whole (windowed) stack. That is forced by the save
    /// pipeline: `save_project` builds a fresh `.stalx.tmp`, renames it over the
    /// target, and only then writes the undo tables into the reopened file.
    ///
    /// `count_limit` is the persisted window: `Some(n)` keeps the last n entries,
    /// while `None` keeps all byte-budgeted Full-mode entries.
    #[must_use]
    pub fn rows_to_persist(
        &self,
        stack: PersistStack,
        count_limit: Option<usize>,
    ) -> Vec<(String, String, Option<String>)> {
        let entries = match stack {
            PersistStack::Undo => &self.undo_stack,
            PersistStack::Redo => &self.redo_stack,
        };

        let skip = match count_limit {
            Some(limit) if entries.len() > limit => entries.len() - limit,
            _ => 0,
        };

        entries
            .iter()
            .skip(skip)
            .filter_map(Self::serialize_entry_with_timestamp)
            .collect()
    }

    /// Serialize an entry to JSON.
    pub fn serialize_entry(entry: &UndoEntry) -> Option<(String, String)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = write_undo_data(&entry.undo_data)?;
        Some((command_json, undo_data_json))
    }

    /// Serialize an entry with its timestamp for database persistence.
    pub fn serialize_entry_with_timestamp(
        entry: &UndoEntry,
    ) -> Option<(String, String, Option<String>)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = write_undo_data(&entry.undo_data)?;
        let timestamp_str = entry.timestamp.map(|t| t.to_rfc3339());
        Some((command_json, undo_data_json, timestamp_str))
    }

    /// Deserialize a legacy entry without a timestamp.
    pub fn deserialize_entry(command_json: &str, undo_data_json: &str) -> Option<UndoEntry> {
        let command = Self::deserialize_command(command_json)?;
        let undo_data = parse_undo_data(undo_data_json)?;
        Some(UndoEntry {
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
        let undo_data = parse_undo_data(undo_data_json)?;
        let timestamp = match timestamp_str {
            Some(raw) => Some(DateTime::parse_from_rfc3339(raw).ok()?.with_timezone(&Utc)),
            None => None,
        };
        Some(UndoEntry {
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
    pub fn load_from_persisted(
        &mut self,
        entries: Vec<(String, String)>,
    ) -> Option<HistoryDropped> {
        self.clear();

        let rows = entries.len();
        let mut unread = Unread::default();
        let mut dropped: Vec<String> = Vec::new();
        for (command_json, undo_data_json) in entries {
            match Self::deserialize_entry(&command_json, &undo_data_json) {
                Some(entry) if Self::is_applicable(&entry) => self.push_undo_tracked(entry),
                Some(entry) => dropped.push(entry.command.description()),
                None => unread.note(&undo_data_json),
            }
        }
        Self::warn_dropped_unapplicable(&dropped);
        if unread.total() > 0 {
            self.drop_history_with_a_hole(unread.total(), rows);
        }
        self.enforce_depth_limit();
        unread.reason()
    }

    /// Drop the whole history because `unreadable` of its `rows` could not be
    /// read: a history written by an older build (whole-project snapshots, not
    /// [`PersistedOut`]), or a corrupt row.
    ///
    /// All of it, not just the unreadable rows. A compact entry is a delta
    /// against the state the entry nearer the live project leaves, so with one
    /// skipped every entry beyond it would undo onto the wrong project. An older
    /// build's history is dropped whole by decision as well: it is the history
    /// of whole-project snapshot pairs this format replaces.
    fn drop_history_with_a_hole(&mut self, unreadable: usize, rows: usize) {
        tracing::warn!(
            unreadable,
            rows,
            purpose = "load persisted undo history",
            "Dropped the persisted undo history: some rows were not written in this build's \
             format (an older build's history, or a damaged row), and a history with an entry \
             missing would undo onto the wrong project"
        );
        self.clear();
    }

    /// Load persisted undo and redo data with timestamps.
    ///
    /// Returns why the whole history was dropped, when it was: an earlier
    /// version's rows, or a row that could not be read (see
    /// [`Self::drop_history_with_a_hole`]).
    pub fn load_from_persisted_with_timestamps(
        &mut self,
        undo_entries: Vec<(String, String, Option<String>)>,
        redo_entries: Vec<(String, String, Option<String>)>,
    ) -> Option<HistoryDropped> {
        self.clear();

        let rows = undo_entries.len() + redo_entries.len();
        let mut unread = Unread::default();
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
                match Self::deserialize_entry_with_timestamp(
                    &command_json,
                    &undo_data_json,
                    timestamp_str.as_deref(),
                ) {
                    Some(entry) if Self::is_applicable(&entry) => self.push_undo_tracked(entry),
                    Some(entry) => dropped.push(entry.command.description()),
                    None => unread.note(&undo_data_json),
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
                match Self::deserialize_entry_with_timestamp(
                    &command_json,
                    &undo_data_json,
                    timestamp_str.as_deref(),
                ) {
                    Some(entry) => self.push_redo_tracked(entry),
                    None => unread.note(&undo_data_json),
                }
            }
        }

        if unread.total() > 0 {
            self.drop_history_with_a_hole(unread.total(), rows);
        }
        self.enforce_depth_limit();
        unread.reason()
    }
}
