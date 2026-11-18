use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Command;
use crate::models::{
    CreditTargets, GpsCoordinates, LineData, LineId, LineIdentity, LoopClosure, ShotData, ShotId,
    StationRef, SurveyCredit,
};

mod added_content;
mod budget;
mod cave_sections;
mod created_station;
pub mod delta;
pub(crate) mod history_batch;
mod history_cave_section;
mod history_credit;
pub mod history_names;
pub mod history_view;
mod history_walk;
mod persistence;
pub(crate) mod snapshot_fixes;
mod uuid_remap;

pub use added_content::{extend_unique_hashes, AddedContent, CarriedIgnore};
pub use cave_sections::{CaveSectionChange, CaveSectionMember, CaveSectionsState};
pub use created_station::CreatedStation;
pub use delta::{BeforePreview, Doc, LineIds, ProjectDelta, SnapshotDelta};
pub use persistence::HistoryDropped;

/// Serialize a line-offset map in KEY order rather than in `HashMap`
/// iteration order.
///
/// These maps ride inside the undo/redo payloads, which are persisted as
/// gzipped JSON in the `.stalx`. `HashMap` hands serde its entries in an order
/// that differs per map instance, so a load-then-save re-serialized the SAME
/// payload with the keys transposed and the file changed for no reason -
/// measured as `{"3":null,"2":[...]}` one save and `{"2":[...],"3":null}` the
/// next. That is what made the save/load fixpoint check RE-ROLL rather than
/// settle (issue #413, finding 23; same class as finding 21). The map type is
/// unchanged - only what it writes is pinned.
fn ordered_line_offsets<S>(
    map: &HashMap<LineId, Option<(f64, f64)>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    map.iter()
        .collect::<std::collections::BTreeMap<_, _>>()
        .serialize(serializer)
}

/// `command` as an entry stores it: what the history shows and what a step
/// needs, without a second copy of data the payload already holds.
///
/// A `CreateLine` carrying shots records the line it created whole in
/// its `CreatedLineSnapshot` (and refreshes it from the live line before
/// an undo), and that record is what its redo restores; the command's own
/// shots are read by nothing after the push (the history shows the name). Kept
/// they would double the entry of every line created with data.
///
/// A `Batch` stored with a compact payload ([`UndoData::Delta`]) is cut down
/// the same way ([`trimmed_batch`]): a step never reads the command, so all
/// that is left of it is what the history shows and what a step settles.
fn stored_command(command: Command, data: &mut UndoData) -> Command {
    match (command, data) {
        (command @ Command::CreateLine { .. }, UndoData::CreatedLineSnapshot { .. }) => {
            without_line_shots(command)
        }
        // An `EditShotCredit` that names its shots keeps none of them: they
        // are in-session ids, which a reload renumbers, and the entry's runs
        // hold what it changed by position. Its redo writes those runs, and
        // the history counts its shots from them (`history_credit`). Kept, a
        // thousand shots were 15 KB of command.
        (
            Command::EditShotCredit {
                targets: CreditTargets::Shots { .. },
                edit,
            },
            UndoData::ShotCredits { .. },
        ) => Command::EditShotCredit {
            targets: CreditTargets::Shots { shots: Vec::new() },
            edit,
        },
        (Command::Batch { commands, label }, UndoData::Delta(delta)) => {
            let total = commands.len();
            let (commands, omitted) = trimmed_batch(commands);
            // The fallback label counts every sub-command, so it is worded
            // before any is left out.
            let label = match label {
                None if omitted > 0 => Some(format!("Batch of {total} commands")),
                label => label,
            };
            delta.omitted_commands += omitted;
            Command::Batch { commands, label }
        }
        (command, _) => command,
    }
}

/// `command` without a `CreateLine`'s shots, which an entry that records the
/// line elsewhere never reads; every other command as it is.
fn without_line_shots(command: Command) -> Command {
    match command {
        Command::CreateLine {
            name,
            surveyor,
            date,
            color,
            direction,
            shots: _,
            cave_section_id,
        } => Command::CreateLine {
            name,
            surveyor,
            date,
            color,
            direction,
            shots: Vec::new(),
            cave_section_id,
        },
        command => command,
    }
}

/// How many sub-commands of a batch a compact entry keeps for the history: its
/// preview lists the first [`history_view::BATCH_PREVIEW_ROWS`] and counts the
/// rest ([`SnapshotDelta::omitted_commands`] holds the ones left out). A
/// margin over that, not every one: a 1,000-cell paste is 1,000 `UpdateShot`s,
/// three times the payload that actually undoes it.
const KEPT_BATCH_COMMANDS: usize = 20;

const _: () = assert!(KEPT_BATCH_COMMANDS >= history_view::BATCH_PREVIEW_ROWS);

/// A compact entry's batch as it is stored, and how many sub-commands it left
/// out. A step through a [`UndoData::Delta`] never reads the command: the
/// delta holds the state on the other side. What still reads it is the
/// history (the label, recorded when the batch ran; the preview, its first
/// rows), the log line, and the reverse settle after a step
/// (`AppState::settle_reversed_lines`, through
/// [`Command::reversed_line_ids`]). So the first [`KEPT_BATCH_COMMANDS`] are
/// kept, and after them every sub-command that reverses a line, in order:
/// the settle reads the batch's net reverses, and a sub-command that reverses
/// nothing adds nothing to them, so leaving it out keeps them exactly.
///
/// A `CreateLine` sub-command loses its shots: the delta carries the line it
/// made, and the history shows only its name.
fn trimmed_batch(commands: Vec<Command>) -> (Vec<Command>, usize) {
    let total = commands.len();
    let kept: Vec<Command> = commands
        .into_iter()
        .enumerate()
        .filter(|(i, cmd)| *i < KEPT_BATCH_COMMANDS || !cmd.reversed_line_ids().is_empty())
        .map(|(_, cmd)| without_line_shots(cmd))
        .collect();
    let omitted = total - kept.len();
    (kept, omitted)
}

/// Default maximum number of undo entries to keep in memory
const DEFAULT_UNDO_HISTORY_LIMIT: usize = 100;

pub(crate) use crate::persistence::undo_limits::ENTRY_OVERHEAD_BYTES;
pub use crate::persistence::undo_limits::{MAX_PERSISTED_HISTORY_ROWS, UNDO_BYTE_BUDGET_BYTES};

/// How a stack's depth is bounded.
///
/// Two shapes rather than one number, because the two are not interchangeable: a
/// count window is what a local project (and Disabled mode) has always had, and a byte
/// budget is what makes Full mode's "full depth since the last commit" survivable. See
/// [`UNDO_BYTE_BUDGET_BYTES`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthLimit {
    /// At most N entries, oldest evicted first. Local projects and Disabled mode.
    Count(usize),
    /// At most N bytes of estimated serialized size, oldest evicted first. Full mode
    /// on a SpeleoDB-linked project.
    Bytes(usize),
}

/// Which stack [`UndoStack::rows_to_persist`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistStack {
    Undo,
    Redo,
}

/// Undo/redo stack
#[derive(Debug)]
#[expect(clippy::struct_field_names)]
pub struct UndoStack {
    /// Commands that can be undone
    undo_stack: VecDeque<UndoEntry>,
    /// Commands that can be redone
    redo_stack: VecDeque<UndoEntry>,
    /// Running estimated byte total for `undo_stack`.
    undo_bytes: usize,
    /// Running estimated byte total for `redo_stack`.
    redo_bytes: usize,
    /// Maximum entries to keep in memory
    history_limit: usize,
    /// [#303] The active bound. `Count(history_limit)` unless the open project is a
    /// Full-mode linked one, in which case `Bytes(UNDO_BYTE_BUDGET_BYTES)`.
    ///
    /// Kept alongside `history_limit` rather than replacing it so that switching a
    /// project back to a count window (closing it, flipping the mode to Disabled)
    /// restores the user's configured number without having to re-read the config.
    depth_limit: DepthLimit,
    /// [#303] The inherited-history watermark, mirrored from `speleo_undo_watermark` so
    /// [`Self::next_stamp`] can stay above it.
    ///
    /// The push cuts every undo row stamped at or before this instant, on the grounds
    /// that it came off another machine with a loaded revision rather than being authored
    /// here (`repair::cut_inherited_undo_rows`). Staying above it is therefore the
    /// difference between the user's new entry shipping and being deleted.
    ///
    /// Mirrored rather than read live because the authoritative copy lives in the
    /// database, and the lock order is database-then-`undo_stack`: reading it from inside
    /// a method that already holds this stack would invert that. Kept in sync by
    /// `speleodb_set_inherited_through`, the single funnel every set and clear goes
    /// through, and seeded on load beside the entries themselves.
    inherited_through: Option<DateTime<Utc>>,
}

/// Entry in the undo stack
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    /// The command that was executed
    pub command: Command,
    /// Data needed to undo this command
    pub undo_data: UndoData,
    /// Timestamp when the command was executed (None for legacy entries)
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    /// [#303] Serialized byte length of `command`, measured once when the entry was
    /// pushed. `None` for an entry restored from disk (nothing persists it) or built by a
    /// test literal. [`UndoStack::estimated_size`] deliberately charges those restored
    /// commands the historical 512-byte flat estimate instead of reserializing them;
    /// their undo payload is still counted separately and normally dominates the entry.
    ///
    /// The byte budget bounds what a Full-mode save gzips and what the next commit
    /// carries, and `command_json` is half of what gets written - but `estimated_size`
    /// charged the command a flat `ENTRY_OVERHEAD`, so a `Batch` of an entire import
    /// counted as 512 bytes against a 32 MiB ceiling whose own doc names Batch as the
    /// motivating case.
    ///
    /// Measured at push rather than at trim, and stored rather than recomputed, because
    /// the two are not the same cost: pushing happens once per edit, trimming happens on
    /// every push after every edit. This mirrors what the snapshot variants already get
    /// for free - they hold their serialized `String`, so their length is exact and cheap
    /// to read; the command is still structured, so it has to be measured deliberately.
    #[serde(default, skip_serializing)]
    pub command_bytes: Option<usize>,
}

/// Consecutive shots of one line whose survey credit one edit changed from
/// the same `before` to the same `after` (see [`UndoData::ShotCredits`]).
///
/// Addressed by POSITION, which a save persists (the shots' order; their
/// in-session ids are renumbered by every load), and checked against the uuid
/// of the run's first shot. A credit edit over a whole line or a thousand-shot
/// selection is then a run per line and per previous credit, not a uuid per
/// shot: about 54 bytes a shot, most of it uuids, was what made such an edit
/// pass 1% of a 20,000-shot file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShotCreditRun {
    pub line_id: LineId,
    /// The run's first shot, by its position in the line. `None` for a run
    /// read from a row written before positions were recorded: one shot,
    /// found by [`Self::anchor`] alone.
    pub start: Option<usize>,
    /// How many consecutive shots the run covers, from `start`.
    pub len: usize,
    /// The uuid of the run's first shot when the edit ran. A step writes the
    /// run only where the shot at `start` still holds it.
    pub anchor: String,
    pub before: SurveyCredit,
    pub after: SurveyCredit,
}

impl ShotCreditRun {
    /// The one shot at `position` of `line_id`, as a capture records it
    /// before the runs are joined ([`join_credit_runs`]).
    pub fn shot(line_id: LineId, position: usize, uuid: String, credit: SurveyCredit) -> Self {
        Self {
            line_id,
            start: Some(position),
            len: 1,
            anchor: uuid,
            before: credit.clone(),
            after: credit,
        }
    }

    /// Whether `next` is the shot after this run, in the same line, changed
    /// the same way.
    fn continued_by(&self, next: &Self) -> bool {
        self.line_id == next.line_id
            && self
                .start
                .zip(next.start)
                .is_some_and(|(start, next)| start.checked_add(self.len) == Some(next))
            && self.before == next.before
            && self.after == next.after
    }
}

/// `changes` (single shots or runs, in any order) as the fewest runs: sorted
/// by line and position, each run extended while the next shot follows it
/// with the same credits. A run read from an older row (no position) stays
/// as it is.
#[must_use]
pub fn join_credit_runs(mut changes: Vec<ShotCreditRun>) -> Vec<ShotCreditRun> {
    changes.sort_by_key(|run| (run.line_id, run.start));
    let mut out: Vec<ShotCreditRun> = Vec::with_capacity(changes.len());
    for run in changes {
        match out.last_mut() {
            Some(last) if last.continued_by(&run) => last.len += run.len,
            _ => out.push(run),
        }
    }
    out.shrink_to_fit();
    out
}

/// `ShotCredits::changes` as the history stores it: every distinct
/// (`before`, `after`) pair once (`c`), and the runs as `[line, start, len,
/// anchor uuid, pair]` (`r`). A credit edit over a whole line usually gives
/// every shot the same credit from the same one, so this is one credit pair
/// and one short row per line. Read back to the same runs in order.
///
/// A row written before positions were recorded (a list of `{s, b, a, u}`
/// groups, a uuid per shot) still reads: each uuid is a one-shot run found by
/// its uuid.
mod credit_runs {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::ShotCreditRun;
    use crate::models::{LineId, SurveyCredit};

    #[derive(Serialize)]
    struct Out<'a> {
        c: Vec<(&'a SurveyCredit, &'a SurveyCredit)>,
        r: Vec<(LineId, usize, usize, &'a str, usize)>,
    }

    #[derive(Deserialize)]
    struct LegacyGroup {
        s: LineId,
        b: SurveyCredit,
        a: SurveyCredit,
        u: Vec<String>,
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum In {
        Runs {
            c: Vec<(SurveyCredit, SurveyCredit)>,
            r: Vec<(LineId, usize, usize, String, usize)>,
        },
        Legacy(Vec<LegacyGroup>),
    }

    pub(super) fn serialize<S: Serializer>(
        runs: &[ShotCreditRun],
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let mut pairs: Vec<(&SurveyCredit, &SurveyCredit)> = Vec::new();
        let mut rows = Vec::with_capacity(runs.len());
        for run in runs {
            let pair = (&run.before, &run.after);
            let index = match pairs.iter().position(|p| *p == pair) {
                Some(index) => index,
                None => {
                    pairs.push(pair);
                    pairs.len() - 1
                }
            };
            // A run without a position (an older row's) is written as one at
            // the position no line reaches, which reads back as one found by
            // its anchor.
            let start = run.start.unwrap_or(usize::MAX);
            rows.push((run.line_id, start, run.len, run.anchor.as_str(), index));
        }
        Out { c: pairs, r: rows }.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Vec<ShotCreditRun>, D::Error> {
        match In::deserialize(deserializer)? {
            In::Runs { c, r } => r
                .into_iter()
                .map(|(line_id, start, len, anchor, pair)| {
                    let (before, after) = c.get(pair).cloned().ok_or_else(|| {
                        serde::de::Error::custom("a credit run names a credit pair past the list")
                    })?;
                    Ok(ShotCreditRun {
                        line_id,
                        start: (start != usize::MAX).then_some(start),
                        len,
                        anchor,
                        before,
                        after,
                    })
                })
                .collect(),
            In::Legacy(groups) => Ok(groups
                .into_iter()
                .flat_map(|group| {
                    let LegacyGroup { s, b, a, u } = group;
                    u.into_iter().map(move |anchor| ShotCreditRun {
                        line_id: s,
                        start: None,
                        len: 1,
                        anchor,
                        before: b.clone(),
                        after: a.clone(),
                    })
                })
                .collect()),
        }
    }
}

/// Data needed to undo a command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoData {
    /// Simple inverse command (SetStationComment -> SetStationComment with the
    /// previous comment)
    InverseCommand(Command),

    /// Previous values for an update operation (JSON serialized)
    PreviousValues {
        line_id: LineId,
        shot_id: Option<ShotId>,
        /// The target shot's persistent UUID - the identity the restore path
        /// addresses it by. `shot_id` is an IN-SESSION id: the loader
        /// renumbers every shot to `line_id * FEATURE_ID_STRIDE +
        /// sequence_order` (`persistence/sqlite/read.rs`) while this entry
        /// reloads as opaque JSON, so after a save/reopen the id names
        /// whichever shot now occupies that slot (issue #413, findings 8, 12
        /// and 13). A uuid survives the renumbering by construction.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): absent in entries
        /// written before this field existed, where `shot_id` is the only
        /// address there is and stays the legacy fallback. Every new entry
        /// carries it whenever the target shot has a uuid.
        #[serde(default)]
        shot_uuid: Option<String>,
        /// JSON serialized previous values (ShotUpdates or LineUpdates)
        values_json: String,
        /// The `line_offsets` entries the edited line's geometry epilogue
        /// can reach, as they stood BEFORE the command - `None` for a line that
        /// had no key.
        ///
        /// The undo of this variant RE-EXECUTES `update_shot` with the previous
        /// field values, and `realign_after_shot_edit` then re-derives the group's
        /// offsets. An inverse RESTORES; it does not re-derive (#413, finding 27).
        ///
        /// PRESENCE is no longer the reason: a dust-zero entry is unrepresentable
        /// now (the invariant on `Project::write_line_offset`), so the
        /// re-derivation cannot drop a key the pre-edit state held. The VALUE
        /// still is. The realign recomputes from line-local coordinates and is
        /// self-correcting, but the sibling arm below genuinely accumulates, and
        /// keeping one rule for both is what makes either readable: an inverse
        /// hands back what it recorded.
        ///
        /// Only the RIGID CONNECTED SET of the edited line
        /// (`Project::capture_rigid_connected_line_offsets` - merges plus
        /// aligned loop closures), not the whole map: that is exactly what the
        /// geometry epilogue can displace, and this entry is written on every
        /// keystroke-scale shot edit.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): empty in entries written
        /// before this field existed and in entries for a command whose undo
        /// re-derives nothing (a metadata-only shot edit, a line-metadata
        /// edit); an empty map restores nothing, which is the pre-fix behaviour.
        #[serde(default)]
        #[serde(skip_serializing_if = "HashMap::is_empty")]
        #[serde(serialize_with = "ordered_line_offsets")]
        previous_offsets: HashMap<LineId, Option<(f64, f64)>>,
    },

    /// Snapshot of a newly created line (for CreateLine undo/redo ID preservation)
    CreatedLineSnapshot {
        line_id: LineId,
        /// Full line data including all shots
        line_data: LineData,
        /// What `line_data` cannot carry: the shots' persistent uuids, the
        /// reverse parity and the least-squares gauge pair. Without it the redo
        /// rebuilds through `Line::new` and mints a fresh uuid per shot -
        /// dangling every `StationKey::AfterShot` anchor onto the line, the
        /// same way a fresh uuid would in `CreatedShotSnapshot` (#413, finding
        /// 1).
        ///
        /// REFRESHED BY THE UNDO, not only recorded at push: the gauge pair is
        /// a save-time heal (`capture_missing_gauge_anchors`) with no undo entry
        /// of its own, so a line created, saved and then undone carries
        /// anchors that did not exist when this entry was pushed. The undo
        /// re-reads the live line immediately before deleting it, so the redo
        /// puts back what the undo destroyed.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): absent in entries
        /// written before this field existed, where the default (no uuids,
        /// parity false, no anchors) reproduces the pre-fix mint.
        #[serde(default)]
        identity: LineIdentity,
    },

    /// Snapshot of a newly created shot (for CreateShot undo/redo ID preservation)
    CreatedShotSnapshot {
        line_id: LineId,
        shot_id: ShotId,
        shot_data: ShotData,
        /// Position in the line's shot list (for re-insertion at correct position)
        position: usize,
        /// Station index from which to shift refs when restoring (redo).
        /// For insert_before: same as position. For insert_after/append: position + 1.
        /// Undo of CreateShot (delete) uses the inverse shift automatically via remove_shot.
        #[serde(default)]
        station_shift_from: u32,
        /// The original command's anchor shot. Redo re-runs the SAME
        /// `insert_shot_after`/`insert_shot_before` (with `shot_id` forced) so the
        /// forward and redo paths share one code path and can't drift apart. Both
        /// `None` means the shot was appended.
        #[serde(default)]
        after_shot_id: Option<ShotId>,
        #[serde(default)]
        before_shot_id: Option<ShotId>,
        /// The anchor shots' persistent UUIDs - a SECOND, independent address
        /// on this entry, and the one the redo re-inserts by. The anchor ids
        /// above are in-session ids and go stale on a reload exactly as
        /// `shot_id` does (issue #413, finding 16), which decides WHERE the
        /// shot comes back.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): absent in entries
        /// written before these fields existed; the anchor id is then the only
        /// address there is and stays the legacy fallback.
        #[serde(default)]
        after_shot_uuid: Option<String>,
        #[serde(default)]
        before_shot_uuid: Option<String>,
        /// The shot's persistent UUID at creation. Redo restores it via
        /// `Shot::with_uuid` so the recreated shot keeps the same identity that
        /// station cross-references (`AfterShot(uuid)`) anchor on.
        /// `None` for legacy persisted entries written before this field existed
        /// (redo then mints a fresh UUID, the pre-fix behavior).
        #[serde(default)]
        uuid: Option<String>,
        /// The `line_offsets` entries the insert's cascade can reach, as they
        /// stood BEFORE it - `None` for a line that had no key. The RIGID
        /// CONNECTED SET, which is what `Project::cascade_connection_offsets`
        /// walks (merges plus aligned loop closures).
        ///
        /// Finding 27's second arm, and the one where the re-derivation genuinely
        /// ACCUMULATES rather than recomputing. Undoing a `CreateShot` REMOVES the
        /// shot, and a mid-line remove pays the line's offset back by the
        /// segment it took out (`Project::remove_shot*` -> `adjust_line_offset`),
        /// so the undo computes `(v + d) - d` - which is not `v` in IEEE. One
        /// cycle of that drift is below every oracle's tolerance; it is the shape
        /// the horizon mode watches accumulate (finding 19). Handing the recorded
        /// map back makes the undo bit-exact instead.
        ///
        /// Captured before the command runs and handed straight back after the
        /// removal, so the undo restores rather than re-derives.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): empty in entries written
        /// before this field existed; an empty map restores nothing, which is the
        /// pre-fix behaviour.
        #[serde(default)]
        #[serde(skip_serializing_if = "HashMap::is_empty")]
        #[serde(serialize_with = "ordered_line_offsets")]
        previous_offsets: HashMap<LineId, Option<(f64, f64)>>,
        /// The legacy surveyor/date the line held when this shot was the
        /// first one appended into it (`Line::legacy_surveyor`): the shot
        /// took it as its credit and the line let it go. The undo, which
        /// leaves the line shot-less again, gives it back.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): absent in entries
        /// written before it existed and for every other insert.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        consumed_legacy: Option<(Option<String>, Option<chrono::NaiveDate>)>,
        /// The Station ID and name of the station the insert created, so
        /// the redo gives the re-created station back exactly
        /// ([`CreatedStation`]).
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): absent in entries
        /// written before Station IDs existed; the redo then names the station
        /// by the creation rules and the epilogue gives it a fresh id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        created_station: Option<CreatedStation>,
    },

    /// Loop closure info for undoing CreateLoopClosure
    ClosureInfo {
        /// The full loop closure data to remove on undo
        closure: LoopClosure,
        /// Where the closure sat in `Project::loop_closures` when
        /// `RemoveLoopClosure` took it out, so undo can put it BACK THERE.
        /// `Vec::remove` preserves the order of the rest, so appending on undo
        /// permutes the list (#413, finding 4) - and that order is persisted
        /// into the `.stalx`, indexed by the frontend, and the order
        /// `resolve_offsets` builds its adjacency lists in.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): `None` for a legacy
        /// persisted entry and for a `CreateLoopClosure` entry, whose undo
        /// REMOVES the closure and whose redo appends the one it first
        /// appended; both keep the push.
        #[serde(default)]
        position: Option<usize>,
    },

    /// Zero datum info for restoring previous GPS and line offsets
    ZeroDatumInfo {
        station_ref: StationRef,
        /// Previous GPS coordinates, None if there was no previous datum
        previous_gps: Option<GpsCoordinates>,
        /// Previous accuracy of that datum (#741). `None` is the default
        /// accuracy, and also what a legacy persisted entry carries (written
        /// before accuracy existed, so the datum had the default).
        #[serde(default)]
        previous_accuracy_m: Option<f64>,
        /// Previous entrance override of that datum (#741). `None` is the
        /// default rule, and also what a legacy persisted entry carries
        /// (written before the override existed).
        #[serde(default)]
        previous_entrance: Option<bool>,
        /// Where the datum sat in `Project::zero_datums` when it was captured,
        /// so undo of `RemoveZeroDatum` re-inserts it there instead of
        /// appending (#413, finding 17 - finding 4's defect one collection
        /// over). The order is persisted, indexed by the frontend, and is the
        /// order `resolve_offsets` walks when it picks the datum that anchors a
        /// connected group, so a permutation can decide where the survey ends
        /// up.
        ///
        /// FORMAT EVOLUTION (additive, no schema bump): `None` for a legacy
        /// persisted entry, and for a `SetZeroDatum` on a station that had no
        /// datum; both keep the push.
        #[serde(default)]
        position: Option<usize>,
        /// Previous line offsets for affected lines
        /// - None value means the key didn't exist (should be removed on undo)
        /// - Some value means the offset should be restored to that value
        #[serde(default)]
        #[serde(serialize_with = "ordered_line_offsets")]
        previous_offsets: HashMap<LineId, Option<(f64, f64)>>,
    },

    /// Per-line previous offsets for segment moves (captured at execution time)
    /// Unlike InverseCommand, this stores the exact set of affected lines and their
    /// previous offsets so that undo/redo is correct even if the connectivity graph changes.
    SegmentMoveInfo {
        /// Line IDs that were moved (captured at execution time)
        affected_lines: Vec<LineId>,
        /// Previous offsets for each affected line (None = no offset existed)
        #[serde(serialize_with = "ordered_line_offsets")]
        previous_offsets: HashMap<LineId, Option<(f64, f64)>>,
        /// The offset that was applied (for redo)
        offset: (f64, f64),
    },

    /// Before and after project snapshots for operations that can't be re-executed
    /// (e.g., batch operations that create lines with dynamic IDs)
    /// - before: Snapshot before the operation (used for undo)
    /// - after: Snapshot after the operation (used for redo)
    ///
    /// A CAPTURE shape only: [`UndoData::compact`] turns every pair into a
    /// [`UndoData::Delta`] before it is pushed, so a live stack holds one only
    /// if the conversion failed (it never has).
    ProjectSnapshotPair { before: String, after: String },

    /// A snapshot pair as an entry holds it: only what differs between the
    /// two states, as the edit that turns the LIVE project into the state on
    /// the other side of this entry - the `before` while the entry is on the
    /// undo stack, the `after` while it is on the redo stack. Each step
    /// re-encodes the way back from the two states it has in hand. See
    /// [`SnapshotDelta`] and `docs/undo-redo.md`.
    Delta(Box<SnapshotDelta>),

    /// Survey credit before and after an `EditShotCredit`, or an
    /// `UpdateLineMetadata` that set the legacy surveyor/date on every shot,
    /// as runs of consecutive shots changed the same way ([`ShotCreditRun`]).
    ///
    /// Undo writes `before` back onto each run's shots, found by position and
    /// checked by the uuid of the run's first shot; redo writes `after` (an
    /// `EditShotCredit`) or re-executes the command (an `UpdateLineMetadata`,
    /// which addresses its line by its stable id). A run that no longer
    /// resolves is skipped with a warning rather than failing the step: a
    /// credit edit is metadata, and a failed undo would wedge every entry
    /// beneath it.
    ShotCredits {
        #[serde(with = "credit_runs")]
        changes: Vec<ShotCreditRun>,
        /// `UpdateLineMetadata` only: the line and its OTHER previous
        /// values (a `LineUpdates` JSON with no surveyor/date), put back by
        /// the undo exactly as `PreviousValues` does.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line_values: Option<(LineId, String)>,
    },

    /// What one "Add .stalx to current project" brought (#674): undo removes
    /// exactly it, redo reinstalls it with the same ids. See [`AddedContent`].
    /// Boxed: the payload is far larger than every other variant.
    AddedProjectContent(Box<AddedContent>),

    /// The Section list and the memberships and colours of the lines a
    /// Section command (#327) changed, before and after: undo restores the
    /// first, redo the second. See [`CaveSectionChange`].
    CaveSections(Box<CaveSectionChange>),

    /// No undo data (for commands that can't be undone)
    None,
}

impl UndoData {
    /// The shot this entry's restore path addresses, when it addresses one:
    /// `(line, the target's uuid if the entry records it, the in-session id
    /// it was recorded with)`.
    ///
    /// The uuid is the address to use; the id is only the legacy fallback for
    /// entries written before the uuid fields existed (issue #413, findings 8,
    /// 12, 13 and 16).
    pub fn shot_target(&self) -> Option<(LineId, Option<&str>, ShotId)> {
        match self {
            UndoData::CreatedShotSnapshot {
                line_id,
                shot_id,
                uuid,
                ..
            } => Some((*line_id, uuid.as_deref(), *shot_id)),
            UndoData::PreviousValues {
                line_id,
                shot_id: Some(shot_id),
                shot_uuid,
                ..
            } => Some((*line_id, shot_uuid.as_deref(), *shot_id)),
            _ => None,
        }
    }

    /// The shot this entry's REDO addresses and needs to still be there.
    ///
    /// A SUBSET of [`Self::shot_target`], because the two directions do not ask
    /// the same question. What is left re-executes its command through
    /// `AppState::command_with_resolved_shot` (an `UpdateShot` or a
    /// `SetShotKind`), which resolves the target first and fails the redo if it
    /// cannot - so for those, "is the shot still there" decides the redo exactly
    /// as it decides the undo.
    ///
    /// `CreatedShotSnapshot` is excluded, and the rule is INVERTED for it: its
    /// entry only sits on the redo stack because the create was undone, so the
    /// shot is ABSENT by construction and the redo puts it back. Its redo path
    /// refuses precisely when the uuid DOES resolve (that would duplicate a shot
    /// identity, #413 finding 16). Filtering the redo stack by resolvability
    /// would therefore drop every persisted create-a-shot redo there is.
    pub fn redo_shot_target(&self) -> Option<(LineId, Option<&str>, ShotId)> {
        if matches!(self, UndoData::CreatedShotSnapshot { .. }) {
            return None;
        }
        self.shot_target()
    }

    /// Does undoing this entry restore recorded state WHOLESALE?
    ///
    /// Such an entry is a wall for any reasoning about which shots exist: it
    /// puts back a whole project recorded in a different id space, so nothing
    /// below it can be judged against the project as loaded (issue #413,
    /// finding 12's write-up calls a `ProjectSnapshotPair` "a wall" for exactly
    /// this reason).
    pub fn restores_recorded_state(&self) -> bool {
        matches!(
            self,
            UndoData::ProjectSnapshotPair { .. } | UndoData::Delta(_)
        )
    }

    /// Drop every `previous_offsets` key whose recorded value is the one
    /// `live` (the project the command left) holds, bit for bit: a key the
    /// command did not change.
    ///
    /// The capture records the map before the command - the whole map, because
    /// a re-frame or a cascade can reach any key - and this cuts it down to
    /// what the command moved, which is all an undo has to put back. For the
    /// two inverses that RE-DERIVE offsets before restoring them (a shot
    /// edit's realign, a shot removal's cascade), the undo pins every key it
    /// does not hold to its live value (`restore_offsets_pinning_the_rest`):
    /// that value is the one the command left, so it is the one it found.
    pub fn keep_changed_offsets(&mut self, live: &HashMap<LineId, (f64, f64)>) {
        let unchanged = |id: &LineId, before: &Option<(f64, f64)>| {
            let bits = |o: Option<(f64, f64)>| o.map(|(x, y)| (x.to_bits(), y.to_bits()));
            bits(*before) == bits(live.get(id).copied())
        };
        match self {
            UndoData::PreviousValues {
                previous_offsets, ..
            }
            | UndoData::CreatedShotSnapshot {
                previous_offsets, ..
            }
            | UndoData::ZeroDatumInfo {
                previous_offsets, ..
            }
            | UndoData::SegmentMoveInfo {
                previous_offsets, ..
            } => previous_offsets.retain(|id, before| !unchanged(id, before)),
            _ => {}
        }
    }

    /// This payload as an entry holds it: a [`UndoData::ProjectSnapshotPair`]
    /// becomes the [`UndoData::Delta`] towards its `before`, and every other
    /// payload is returned as it is.
    ///
    /// Called where a capture is pushed, outside the stack's lock: the
    /// conversion parses and diffs two whole projects. A pair that cannot be
    /// converted (its halves are not serialized projects, which a capture never
    /// produces) is kept as a pair, which still steps correctly.
    #[must_use]
    pub fn compact(self, command: &Command) -> Self {
        let UndoData::ProjectSnapshotPair { before, after } = self else {
            return self;
        };
        let preview = history_view::before_preview(command, &before);
        match SnapshotDelta::from_pair(&before, &after, preview) {
            Ok(delta) => UndoData::Delta(Box::new(delta)),
            Err(e) => {
                tracing::error!(
                    command = ?command.description(),
                    reason = %e,
                    purpose = "store an undo entry compactly",
                    "Could not encode a snapshot pair as a delta; keeping the pair"
                );
                UndoData::ProjectSnapshotPair { before, after }
            }
        }
    }

    /// Return a short, safe variant name for logging (no payload data)
    pub fn variant_name(&self) -> &'static str {
        match self {
            UndoData::InverseCommand(_) => "InverseCommand",
            UndoData::PreviousValues { .. } => "PreviousValues",
            UndoData::CreatedLineSnapshot { .. } => "CreatedLineSnapshot",
            UndoData::CreatedShotSnapshot { .. } => "CreatedShotSnapshot",
            UndoData::ClosureInfo { .. } => "ClosureInfo",
            UndoData::ZeroDatumInfo { .. } => "ZeroDatumInfo",
            UndoData::SegmentMoveInfo { .. } => "SegmentMoveInfo",
            UndoData::ProjectSnapshotPair { .. } => "ProjectSnapshotPair",
            UndoData::Delta(_) => "Delta",
            UndoData::AddedProjectContent(_) => "AddedProjectContent",
            UndoData::ShotCredits { .. } => "ShotCredits",
            UndoData::CaveSections(_) => "CaveSections",
            UndoData::None => "None",
        }
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            history_limit: DEFAULT_UNDO_HISTORY_LIMIT,
            depth_limit: DepthLimit::Count(DEFAULT_UNDO_HISTORY_LIMIT),
            inherited_through: None,
        }
    }
}

impl UndoStack {
    /// Create a new empty undo stack with default limit
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new empty undo stack with custom limit
    pub fn new_with_limit(history_limit: usize) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            undo_bytes: 0,
            redo_bytes: 0,
            history_limit,
            depth_limit: DepthLimit::Count(history_limit),
            inherited_through: None,
        }
    }

    /// Get the current history limit
    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// Set the history limit
    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        // [#303] Only re-arm the COUNT bound. A Full-mode linked project is on the byte
        // budget, and a config reload (which is what calls this) must not knock it back
        // onto the entry window - that would silently truncate the history the next push
        // is supposed to carry to the server.
        if matches!(self.depth_limit, DepthLimit::Count(_)) {
            self.depth_limit = DepthLimit::Count(limit);
        }
        self.enforce_depth_limit();
    }

    /// [#303] Switch this stack onto the byte budget (Full mode on a linked project).
    /// Idempotent, and trims immediately if the stack already exceeds the budget.
    pub fn set_byte_budget(&mut self, budget: usize) {
        self.depth_limit = DepthLimit::Bytes(budget);
        self.enforce_depth_limit();
    }

    /// [#303] Switch this stack back onto the configured entry-count window (a local
    /// project, Disabled mode, or no project open).
    pub fn use_count_limit(&mut self) {
        self.depth_limit = DepthLimit::Count(self.history_limit);
        self.enforce_depth_limit();
    }

    /// The active bound.
    pub fn depth_limit(&self) -> DepthLimit {
        self.depth_limit
    }

    /// [#303] Are this stack's timestamps non-decreasing, front to back?
    ///
    /// The invariant `next_stamp` maintains on the way in, checked on the way out of a
    /// load. Untimestamped (legacy) entries are ignored rather than failing the check: they
    /// sort as a prefix by construction - a legacy file is uniformly unstamped, since the
    /// build that wrote it had no timestamps at all - and `drop_entries_up_to` reads them
    /// as "before the cutoff" anyway.
    fn stamps_ascend(stack: &VecDeque<UndoEntry>) -> bool {
        let mut previous: Option<DateTime<Utc>> = None;
        for stamp in stack.iter().filter_map(|entry| entry.timestamp) {
            if previous.is_some_and(|prev| stamp < prev) {
                return false;
            }
            previous = Some(stamp);
        }
        true
    }

    /// [#303] Mirror the inherited-history watermark onto this stack.
    ///
    /// See [`Self::inherited_through`]. Always call it on a load, with `None` when the
    /// project has no watermark, so a value cannot outlive the project it came from.
    pub fn set_inherited_through(&mut self, instant: Option<DateTime<Utc>>) {
        self.inherited_through = instant;
    }

    /// [#303] The stamp for the next entry: wall-clock, but never at or below the entry
    /// beneath it, nor at or below the inherited-history watermark.
    ///
    /// Everything that reasons about this stack by time assumes later-in-the-stack means
    /// later-in-time - `drop_entries_up_to` says so outright and uses `partition_point`,
    /// which silently returns a meaningless boundary if the order does not hold. Wall time
    /// alone does not guarantee it: an entry can arrive from a peer whose clock ran ahead
    /// (a chain step loads a past commit's history, stamped on the machine that authored
    /// it), and a local clock can be corrected backwards under us.
    ///
    /// So the stack keeps its own order. The stamp is the real instant whenever that is
    /// already ahead - the ordinary case, and the one that keeps displayed times honest -
    /// and otherwise the smallest value that preserves the invariant.
    ///
    /// The watermark is the second floor, and staying above the entry beneath is NOT
    /// enough to clear it. Undo far enough into a chain step's loaded rows and the entry
    /// beneath is an OLD one: a peer whose clock ran a day fast stamps its rows across
    /// (T+1d-5min .. T+1d), the watermark lands at T+1d, and an edit made after undoing to
    /// the oldest of them took `T+1d-5min + 1ns` - correctly above its neighbour, and five
    /// minutes below the watermark. `cut_inherited_undo_rows` then deletes the user's own
    /// entry from the upload as inherited, so the commit ships the survey change with no
    /// history for it and a later Unwind cannot step through the operation.
    fn next_stamp(&self) -> DateTime<Utc> {
        let now = Utc::now();
        // The highest instant a new entry must beat, whichever of the two floors it is.
        let floor = match (
            self.undo_stack.back().and_then(|entry| entry.timestamp),
            self.inherited_through,
        ) {
            (Some(prev), Some(mark)) => Some(prev.max(mark)),
            (prev, mark) => prev.or(mark),
        };
        match floor {
            // `>=`, not `>`: two entries sharing an instant would break the strict
            // ordering just as an inversion does - and the push's cut is itself `<=`, so
            // an entry landing exactly ON the watermark is one it deletes.
            Some(prev) if prev >= now => prev + chrono::Duration::nanoseconds(1),
            // No previous entry (or a legacy one carrying no timestamp), and no watermark:
            // there is nothing to stay ahead of.
            _ => now,
        }
    }

    /// The newest stamp on the undo stack, if any - the ceiling a caller must clear to be
    /// sure it is talking about "everything currently on the stack" (see
    /// `speleodb_clear_undo_after_push`). Cheap: the invariant above makes it the back.
    pub fn newest_stamp(&self) -> Option<DateTime<Utc>> {
        self.undo_stack.back().and_then(|entry| entry.timestamp)
    }

    /// Push a command with its undo data to the undo stack
    pub fn push_with_data(&mut self, command: Command, undo_data: UndoData) {
        // Production callers compact before taking the lock; this catches the
        // rest, so no snapshot pair is ever held.
        let mut undo_data = undo_data.compact(&command);
        let command = stored_command(command, &mut undo_data);
        let entry = UndoEntry {
            // Measured once, HERE, and never again: this runs per edit, while
            // `estimated_size` runs on every trim after every edit. See the field's doc.
            command_bytes: serde_json::to_string(&command).ok().map(|json| json.len()),
            command,
            undo_data,
            timestamp: Some(self.next_stamp()),
        };

        self.push_undo_tracked(entry);

        // Clear redo stack when new command is executed
        self.clear_redo();

        self.enforce_depth_limit();
    }

    /// Push a command without undo data (convenience wrapper, timestamps are added automatically)
    ///
    /// The resulting entry carries [`UndoData::None`], which `undo()` cannot apply
    /// (see [`Self::is_applicable`]). Production code must not use this: the command
    /// path guards on `UndoData::None` before pushing, and `load_from_persisted*`
    /// drops such entries. Kept for the stack-mechanics tests in
    /// `tests/integration_tests.rs`, which only exercise push/pop/limit ordering.
    pub fn push(&mut self, command: Command) {
        self.push_with_data(command, UndoData::None);
    }

    /// Pop from undo stack and return the entry
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        let entry = self.undo_stack.pop_back()?;
        self.undo_bytes = self.undo_bytes.saturating_sub(Self::estimated_size(&entry));
        Some(entry)
    }

    /// Push to redo stack
    pub fn push_redo(&mut self, entry: UndoEntry) {
        self.push_redo_tracked(entry);
        // The redo stack is bounded too - undoing a long delete-heavy session moves
        // every one of those whole-project snapshots across.
        self.enforce_depth_limit();
    }

    /// Pop from redo stack
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        let entry = self.redo_stack.pop_back()?;
        self.redo_bytes = self.redo_bytes.saturating_sub(Self::estimated_size(&entry));
        Some(entry)
    }

    /// Push back to undo stack (for redo operations)
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.push_undo_tracked(entry);
        // Don't clear redo stack when pushing for redo
        self.enforce_depth_limit();
    }

    /// Append to the undo stack.
    ///
    /// The single point an entry enters that stack: `push_with_data` (a new edit) and
    /// `push_undo` (a redo replaying one back) both route through here. Note that it does
    /// NOT bound the stack - each caller applies `enforce_depth_limit` itself, because
    /// they differ in what else the append implies (a new edit also clears redo; a replay
    /// must not).
    fn push_undo_tracked(&mut self, entry: UndoEntry) {
        self.undo_bytes = self.undo_bytes.saturating_add(Self::estimated_size(&entry));
        self.undo_stack.push_back(entry);
    }

    /// Append to the redo stack without applying the active bound.
    fn push_redo_tracked(&mut self, entry: UndoEntry) {
        self.redo_bytes = self.redo_bytes.saturating_add(Self::estimated_size(&entry));
        self.redo_stack.push_back(entry);
    }

    /// Check if undo is available
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clear both stacks
    pub fn clear(&mut self) {
        self.clear_undo();
        self.clear_redo();
    }

    fn clear_undo(&mut self) {
        self.undo_stack.clear();
        self.undo_bytes = 0;
    }

    /// [#303] Drop every undo entry whose timestamp is at or before `cutoff`, and the
    /// whole redo stack, returning how many undo entries went.
    ///
    /// This is clear-on-push (q8 / Q25): a confirmed push has just uploaded those
    /// operations, so keeping them locally would duplicate them in the NEXT commit -
    /// and the hybrid model's answer to "undo past the last commit" is the explicit
    /// Unwind, not a second copy of the history.
    ///
    /// Keyed on the upload's undo cutoff rather than clearing everything, because an
    /// edit that landed AFTER the snapshot was taken is not on the server:
    /// `speleodb_upload` deliberately keeps its dirty bit so the next push carries it,
    /// so its undo entry has to survive too. Clearing the lot would leave a dirty
    /// project whose pending edit had no history at all.
    ///
    /// Entries with NO timestamp are legacy persisted ones, which necessarily predate
    /// this session's push, so they are dropped.
    pub fn drop_entries_up_to(&mut self, cutoff: DateTime<Utc>) -> usize {
        let before = self.undo_stack.len();
        // Timestamps are assigned in push order, so the entries to drop are a PREFIX -
        // a front eviction, exactly like a trim. `partition_point` finds the boundary
        // without scanning the tail.
        let keep_from = self
            .undo_stack
            .partition_point(|e| e.timestamp.is_none_or(|ts| ts <= cutoff));
        let dropped_bytes = self
            .undo_stack
            .drain(..keep_from)
            .map(|entry| Self::estimated_size(&entry))
            .sum::<usize>();
        self.undo_bytes = self.undo_bytes.saturating_sub(dropped_bytes);
        // Redo is never uploaded (clarification #1) and a redo branch that replays onto
        // a state the server has now committed is exactly the stale-replay hazard, so it
        // goes wholesale.
        self.clear_redo();
        before - self.undo_stack.len()
    }

    /// Clear the redo stack only, leaving undo history intact.
    ///
    /// `push_with_data` already clears redo as part of pushing; this is for the
    /// command path's other branch, where a command SUCCEEDED but produced no
    /// undoable entry. The redo branch is invalidated either way - the project has
    /// moved on from the state those entries would replay onto - so the clear must
    /// not be coupled to whether an entry was pushed.
    pub fn clear_redo(&mut self) {
        self.redo_stack.clear();
        self.redo_bytes = 0;
    }

    /// Get the last command that can be undone
    pub fn peek_undo(&self) -> Option<&Command> {
        self.undo_stack.back().map(|e| &e.command)
    }

    /// Get the last command that can be redone
    pub fn peek_redo(&self) -> Option<&Command> {
        self.redo_stack.back().map(|e| &e.command)
    }

    /// Get the undo data for the last undoable command
    pub fn peek_undo_data(&self) -> Option<&UndoData> {
        self.undo_stack.back().map(|e| &e.undo_data)
    }

    /// Get the number of entries in the undo stack
    pub fn len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Check if the undo stack is empty
    pub fn is_empty(&self) -> bool {
        self.undo_stack.is_empty()
    }

    /// Get the number of entries in the redo stack
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Get an iterator over the undo stack entries (index 0 = most recent)
    /// Returns entries in LIFO order (most recent first)
    pub fn undo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.undo_stack.iter().rev()
    }

    /// Get an iterator over the redo stack entries (index 0 = most recent)
    /// Returns entries in LIFO order (most recent first)
    pub fn redo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.redo_stack.iter().rev()
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::models::ShotData;

    /// Helper to create a test command
    fn test_create_line_command(name: &str) -> Command {
        Command::CreateLine {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
            cave_section_id: None,
        }
    }

    /// Helper to create a test delete line command
    fn test_delete_line_command(line_id: LineId) -> Command {
        Command::DeleteLine { line_id }
    }

    /// Helper to create test line data
    fn test_line_data(name: &str) -> LineData {
        LineData {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![],
        }
    }

    // === Undo Stack Basic Tests ===

    #[test]
    fn test_new_undo_stack_is_empty() {
        let stack = UndoStack::new();
        assert!(stack.is_empty());
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_new_with_limit() {
        let stack = UndoStack::new_with_limit(50);
        assert_eq!(stack.history_limit(), 50);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_default_history_limit() {
        let stack = UndoStack::new();
        assert_eq!(stack.history_limit(), 100);
    }

    #[test]
    fn test_push_enables_undo() {
        let mut stack = UndoStack::new();
        let cmd = test_create_line_command("Test");

        stack.push_with_data(cmd, UndoData::None);

        assert!(stack.can_undo());
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_push_multiple_commands() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_line_command("First"), UndoData::None);
        stack.push_with_data(test_create_line_command("Second"), UndoData::None);
        stack.push_with_data(test_create_line_command("Third"), UndoData::None);

        assert_eq!(stack.len(), 3);
        assert!(stack.can_undo());
    }

    #[test]
    fn test_pop_undo_returns_last_command() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_line_command("First"), UndoData::None);
        stack.push_with_data(test_create_line_command("Second"), UndoData::None);

        let entry = stack.pop_undo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Second".to_string()));
        } else {
            panic!("Expected CreateLine command");
        }

        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_pop_undo_empty_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.pop_undo().is_none());
    }

    #[test]
    fn test_peek_undo_does_not_modify_stack() {
        let mut stack = UndoStack::new();
        let cmd = test_create_line_command("Test");

        stack.push_with_data(cmd, UndoData::None);

        let peeked = stack.peek_undo();
        assert!(peeked.is_some());
        assert_eq!(stack.len(), 1); // Stack unchanged
    }

    #[test]
    fn test_clear_removes_all_entries() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_line_command("First"), UndoData::None);
        stack.push_with_data(test_create_line_command("Second"), UndoData::None);
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        stack.clear();

        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 0);
    }

    // === Redo Stack Tests ===

    #[test]
    fn test_push_redo_enables_redo() {
        let mut stack = UndoStack::new();
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        stack.push_redo(entry);

        assert!(stack.can_redo());
        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_pop_redo_returns_last_entry() {
        let mut stack = UndoStack::new();

        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("First"),
            undo_data: UndoData::None,
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Second"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        let entry = stack.pop_redo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Second".to_string()));
        } else {
            panic!("Expected CreateLine command");
        }

        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_pop_redo_empty_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.pop_redo().is_none());
    }

    #[test]
    fn test_peek_redo_does_not_modify_stack() {
        let mut stack = UndoStack::new();
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        stack.push_redo(entry);

        let peeked = stack.peek_redo();
        assert!(peeked.is_some());
        assert_eq!(stack.redo_len(), 1); // Stack unchanged
    }

    #[test]
    fn test_push_clears_redo_stack() {
        let mut stack = UndoStack::new();

        // Add some redo entries
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Redo1"),
            undo_data: UndoData::None,
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Redo2"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        assert!(stack.can_redo());

        // New command should clear redo stack
        stack.push_with_data(test_create_line_command("New"), UndoData::None);

        assert!(!stack.can_redo());
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_push_undo_does_not_clear_redo_stack() {
        let mut stack = UndoStack::new();

        // Add redo entries
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        // push_undo is used for redo operations and should NOT clear redo
        stack.push_undo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Restored"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        assert!(stack.can_redo()); // Redo stack preserved
    }

    // === History Limit Tests ===

    #[test]
    fn test_history_limit_enforced_on_push() {
        let mut stack = UndoStack::new_with_limit(3);

        for i in 0..5 {
            stack.push_with_data(
                test_create_line_command(&format!("Line {i}")),
                UndoData::None,
            );
        }

        assert_eq!(stack.len(), 3);

        // The oldest entries should be removed
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Line 4".to_string()));
        } else {
            panic!("Expected Line 4");
        }
    }

    #[test]
    fn test_set_history_limit_trims_stack() {
        let mut stack = UndoStack::new();

        for i in 0..10 {
            stack.push_with_data(
                test_create_line_command(&format!("Line {i}")),
                UndoData::None,
            );
        }

        assert_eq!(stack.len(), 10);

        // Reduce limit - should trim oldest entries
        stack.set_history_limit(5);

        assert_eq!(stack.len(), 5);
        assert_eq!(stack.history_limit(), 5);

        // Verify we have the most recent entries
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Line 9".to_string()));
        } else {
            panic!("Expected Line 9");
        }
    }

    #[test]
    fn test_set_history_limit_zero() {
        let mut stack = UndoStack::new();
        stack.push_with_data(test_create_line_command("Test"), UndoData::None);

        stack.set_history_limit(0);

        assert_eq!(stack.len(), 0);
        assert!(!stack.can_undo());
    }

    #[test]
    fn test_set_history_limit_trims_redo_stack() {
        let mut stack = UndoStack::new();

        // Build up a redo stack by pushing then undoing
        for i in 0..10 {
            stack.push_redo(UndoEntry {
                command_bytes: None,
                command: test_create_line_command(&format!("Redo {i}")),
                undo_data: UndoData::None,
                timestamp: None,
            });
        }

        assert_eq!(stack.redo_len(), 10);

        // Reduce limit - should trim redo stack too
        stack.set_history_limit(3);

        assert_eq!(stack.redo_len(), 3);
    }

    // === UndoData Tests ===

    #[test]
    fn test_undo_data_inverse_command() {
        let mut stack = UndoStack::new();

        let inverse = Command::DeleteLine { line_id: 42 };
        stack.push_with_data(
            test_create_line_command("Test"),
            UndoData::InverseCommand(inverse),
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::InverseCommand(cmd) = data {
            if let Command::DeleteLine { line_id } = cmd {
                assert_eq!(*line_id, 42);
            } else {
                panic!("Expected DeleteLine");
            }
        } else {
            panic!("Expected InverseCommand");
        }
    }

    #[test]
    fn test_undo_data_zero_datum_info() {
        let mut stack = UndoStack::new();

        let gps = GpsCoordinates::new(45.0, -73.5, 100.0).unwrap();
        let mut offsets = HashMap::new();
        offsets.insert(1, Some((10.0, 20.0)));
        offsets.insert(2, None); // Key didn't exist before
        stack.push_with_data(
            Command::SetZeroDatum {
                station_ref: StationRef::new(1, 0),
                coordinates: gps,
                accuracy_m: None,
                entrance: None,
            },
            UndoData::ZeroDatumInfo {
                station_ref: StationRef::new(1, 0),
                previous_gps: Some(GpsCoordinates::new(40.0, -70.0, 50.0).unwrap()),
                previous_accuracy_m: None,
                position: Some(0),
                previous_offsets: offsets,
                previous_entrance: None,
            },
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::ZeroDatumInfo {
            station_ref,
            previous_gps,
            previous_offsets,
            ..
        } = data
        {
            assert_eq!(station_ref.line_id, 1);
            let prev = previous_gps.as_ref().unwrap();
            assert_eq!(prev.latitude, 40.0);
            assert_eq!(previous_offsets.get(&1), Some(&Some((10.0, 20.0))));
            assert_eq!(previous_offsets.get(&2), Some(&None));
        } else {
            panic!("Expected ZeroDatumInfo");
        }
    }

    // === Serialization / Persistence Tests ===

    #[test]
    fn test_serialize_entry_basic() {
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Serialize Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        let result = UndoStack::serialize_entry(&entry);
        assert!(result.is_some());

        let (cmd_json, undo_json) = result.unwrap();
        assert!(cmd_json.contains("Serialize Test"));
        assert!(undo_json.contains("None"));
    }

    #[test]
    fn test_deserialize_entry_basic() {
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Deserialize Test"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(42)),
            timestamp: None,
        };

        let (cmd_json, undo_json) = UndoStack::serialize_entry(&entry).unwrap();
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json).unwrap();

        if let Command::CreateLine { name, .. } = restored.command {
            assert_eq!(name, Some("Deserialize Test".to_string()));
        } else {
            panic!("Expected CreateLine");
        }

        if let UndoData::InverseCommand(Command::DeleteLine { line_id }) = restored.undo_data {
            assert_eq!(line_id, 42);
        } else {
            panic!("Expected InverseCommand with DeleteLine");
        }
    }

    #[test]
    fn test_deserialize_entry_invalid_json() {
        let result = UndoStack::deserialize_entry("not valid json", "{}");
        assert!(result.is_none());

        let result = UndoStack::deserialize_entry("{}", "not valid json");
        assert!(result.is_none());
    }

    #[test]
    fn test_serialize_entry_with_line_snapshot() {
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Snapshot"),
            undo_data: UndoData::CreatedLineSnapshot {
                line_id: 5,
                line_data: test_line_data("Snapshot"),
                identity: LineIdentity::default(),
            },
            timestamp: None,
        };

        let result = UndoStack::serialize_entry(&entry);
        assert!(result.is_some());

        let (cmd_json, undo_json) = result.unwrap();
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json).unwrap();

        if let UndoData::CreatedLineSnapshot {
            line_id, line_data, ..
        } = restored.undo_data
        {
            assert_eq!(line_id, 5);
            assert_eq!(line_data.name, Some("Snapshot".to_string()));
        } else {
            panic!("Expected CreatedLineSnapshot");
        }
    }

    #[test]
    fn test_load_from_persisted() {
        let mut stack = UndoStack::new();

        // Create serialized entries
        let entries: Vec<(String, String)> = (0..3)
            .map(|i| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("Line {i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(i as u32)),
                    timestamp: None,
                };
                UndoStack::serialize_entry(&entry).unwrap()
            })
            .collect();

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 3);
        assert!(stack.can_undo());
        assert!(!stack.can_redo()); // Redo should be cleared

        // Verify entries loaded in correct order
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Line 2".to_string()));
        } else {
            panic!("Expected Line 2");
        }
    }

    #[test]
    fn test_load_from_persisted_respects_limit() {
        let mut stack = UndoStack::new_with_limit(2);

        // Create more entries than the limit. The filler undo data must be a REAL
        // variant: `load_from_persisted` drops `UndoData::None` entries, so `None`
        // here would test the filter instead of the limit.
        let entries: Vec<(String, String)> = (0..5)
            .map(|i| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("Line {i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
                    timestamp: None,
                };
                UndoStack::serialize_entry(&entry).unwrap()
            })
            .collect();

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 2);

        // Should have the most recent entries
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateLine { name, .. } = entry.command {
            assert_eq!(name, Some("Line 4".to_string()));
        } else {
            panic!("Expected Line 4");
        }
    }

    #[test]
    fn test_load_from_persisted_clears_existing() {
        let mut stack = UndoStack::new();

        // Add some entries
        stack.push_with_data(test_create_line_command("Existing"), UndoData::None);
        stack.push_redo(UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Existing Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        // Load persisted data should clear existing. Real undo data, so the load
        // filter keeps the entry and this stays a test of the clear.
        let entries: Vec<(String, String)> = vec![UndoStack::serialize_entry(&UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Loaded"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
            timestamp: None,
        })
        .unwrap()];

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 1);
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_load_from_persisted_drops_a_history_with_an_unreadable_row() {
        let mut stack = UndoStack::new();

        // Real undo data, so the only thing that can drop this entry is the
        // unreadable rows this test is about.
        let valid_entry = UndoStack::serialize_entry(&UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Valid"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
            timestamp: None,
        })
        .unwrap();

        assert_eq!(stack.load_from_persisted(vec![valid_entry.clone()]), None);
        assert_eq!(stack.len(), 1, "a readable history loads");

        // A compact entry is a delta against the entry above it, so a history
        // with a hole would undo onto the wrong project: all of it goes.
        let damaged = stack.load_from_persisted(vec![
            ("invalid json".to_string(), "{\"v2\":7}".to_string()),
            valid_entry.clone(),
        ]);
        assert_eq!(stack.len(), 0);
        assert_eq!(damaged, Some(HistoryDropped::Unreadable));

        // So does a history written by an older build (a bare payload), and the
        // banner is told it was an earlier version's rather than damaged.
        let legacy =
            serde_json::to_string(&UndoData::InverseCommand(test_delete_line_command(1))).unwrap();
        let earlier =
            stack.load_from_persisted(vec![valid_entry.clone(), (valid_entry.0.clone(), legacy)]);
        assert_eq!(stack.len(), 0);
        assert_eq!(earlier, Some(HistoryDropped::EarlierVersion));

        // A later build's envelope is not "an earlier version".
        let later = stack.load_from_persisted(vec![
            valid_entry.clone(),
            (
                valid_entry.0.clone(),
                "{\"v3\":{\"None\":null}}".to_string(),
            ),
        ]);
        assert_eq!(stack.len(), 0);
        assert_eq!(later, Some(HistoryDropped::Unreadable));

        // A compact entry that breaks the delta rules is untrusted input, not a
        // history: one that keeps its single line twice would double the
        // project on every step. It is unreadable, and the history goes.
        let hostile = r#"{"v2":{"Delta":{"d":{"s":[1,[{"k":[0,1]},{"k":[0,1]}]]}}}}"#;
        let refused = stack.load_from_persisted(vec![
            valid_entry.clone(),
            (valid_entry.0.clone(), hostile.to_string()),
        ]);
        assert_eq!(stack.len(), 0);
        assert_eq!(refused, Some(HistoryDropped::Unreadable));
        // The same entry without the repeat is read.
        let fine = r#"{"v2":{"Delta":{"d":{"s":[1,[{"k":[0,1]}]]}}}}"#;
        assert_eq!(
            stack.load_from_persisted(vec![
                valid_entry.clone(),
                (valid_entry.0.clone(), fine.to_string())
            ]),
            None
        );
        assert_eq!(stack.len(), 2);
    }

    /// A stack persisted before the forward guard existed can carry an entry with
    /// `UndoData::None`. `undo()` cannot apply it and pushes it straight back, so
    /// everything below it is unreachable forever. Loading must drop it and keep the
    /// real history underneath usable.
    #[test]
    fn load_drops_a_persisted_none_entry_and_keeps_the_history_under_it() {
        let mut stack = UndoStack::new();

        let entries: Vec<(String, String)> = vec![
            UndoStack::serialize_entry(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Older"),
                undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
                timestamp: None,
            })
            .unwrap(),
            UndoStack::serialize_entry(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Poison"),
                undo_data: UndoData::None,
                timestamp: None,
            })
            .unwrap(),
            UndoStack::serialize_entry(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Newer"),
                undo_data: UndoData::InverseCommand(test_delete_line_command(3)),
                timestamp: None,
            })
            .unwrap(),
        ];

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 2, "the unapplicable entry must be dropped");

        let top = stack.pop_undo().unwrap();
        match top.command {
            Command::CreateLine { name, .. } => assert_eq!(name, Some("Newer".to_string())),
            other => panic!("Expected Newer, got {other:?}"),
        }
        let below = stack.pop_undo().unwrap();
        match below.command {
            Command::CreateLine { name, .. } => assert_eq!(name, Some("Older".to_string())),
            other => panic!("Expected Older, got {other:?}"),
        }
    }

    /// The timestamped twin - the only loader real project opens use
    /// (`project_load.rs`), and previously the only one with no unit coverage.
    #[test]
    fn load_with_timestamps_drops_a_persisted_none_undo_entry() {
        let mut stack = UndoStack::new();

        let undo_entries: Vec<(String, String, Option<String>)> = vec![
            UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Older"),
                undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
                timestamp: None,
            })
            .unwrap(),
            UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Poison"),
                undo_data: UndoData::None,
                timestamp: None,
            })
            .unwrap(),
            UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Newer"),
                undo_data: UndoData::InverseCommand(test_delete_line_command(3)),
                timestamp: None,
            })
            .unwrap(),
        ];

        stack.load_from_persisted_with_timestamps(undo_entries, vec![]);

        assert_eq!(stack.len(), 2);

        let top = stack.pop_undo().unwrap();
        match top.command {
            Command::CreateLine { name, .. } => assert_eq!(name, Some("Newer".to_string())),
            other => panic!("Expected Newer, got {other:?}"),
        }
        let below = stack.pop_undo().unwrap();
        match below.command {
            Command::CreateLine { name, .. } => assert_eq!(name, Some("Older".to_string())),
            other => panic!("Expected Older, got {other:?}"),
        }
    }

    // === [#413] Entries whose shot target no longer resolves ===

    /// A one-line project whose single shot carries `uuid`.
    fn project_holding_one_shot(uuid: &str) -> crate::models::Project {
        let mut project = crate::models::Project::new("Addressing".to_string(), None);
        let mut line = crate::models::Line::new(
            0,
            LineData {
                shots: vec![ShotData {
                    length: 5.0,
                    ..Default::default()
                }],
                ..test_line_data("Only")
            },
        );
        line.shots[0].uuid = Some(uuid.to_string());
        project.lines.push(line);
        project
    }

    /// An entry whose `CreateShot` created the shot named by `uuid`, recorded
    /// against in-session id `shot_id`.
    fn created_shot_entry(uuid: Option<&str>, shot_id: ShotId) -> UndoEntry {
        UndoEntry {
            command_bytes: None,
            command: Command::CreateShot {
                line_id: 0,
                shot: ShotData::default(),
                after_shot_id: None,
                before_shot_id: None,
            },
            undo_data: UndoData::CreatedShotSnapshot {
                line_id: 0,
                shot_id,
                shot_data: ShotData::default(),
                position: 0,
                station_shift_from: 0,
                after_shot_id: None,
                before_shot_id: None,
                after_shot_uuid: None,
                before_shot_uuid: None,
                uuid: uuid.map(str::to_string),
                previous_offsets: HashMap::new(),
                consumed_legacy: None,
                created_station: None,
            },
            timestamp: None,
        }
    }

    #[test]
    fn an_entry_whose_shot_uuid_is_gone_is_dropped_and_the_history_under_it_survives() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        stack.push_with_data(
            test_create_line_command("Older"),
            UndoData::InverseCommand(test_delete_line_command(1)),
        );
        let doomed = created_shot_entry(Some("uuid-that-is-not-there"), 0);
        stack.push_with_data(doomed.command, doomed.undo_data);

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 1);
        assert_eq!(
            stack.len(),
            1,
            "the entry beneath it must still be undoable"
        );
        assert!(matches!(
            stack.peek_undo(),
            Some(Command::CreateLine { name: Some(name), .. }) if name == "Older"
        ));
    }

    #[test]
    fn an_entry_whose_shot_uuid_still_resolves_is_kept() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        // The recorded in-session id is deliberately WRONG (the loader would
        // have renumbered it); the uuid is what has to decide.
        let entry = created_shot_entry(Some("live-uuid"), 4_242);
        stack.push_with_data(entry.command, entry.undo_data);

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 0);
        assert_eq!(stack.len(), 1);
    }

    /// A LEGACY entry carries no uuid, so the in-session id is the only address
    /// it has - and an id past the renumbered range is what wedges the history.
    #[test]
    fn a_legacy_entry_with_an_out_of_range_shot_id_is_dropped() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        let entry = created_shot_entry(None, 4_242);
        stack.push_with_data(entry.command, entry.undo_data);

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 1);
        assert!(stack.is_empty());
    }

    /// The scan stops at a wall. A `ProjectSnapshotPair` restores a whole
    /// project recorded in ITS id space, so an entry beneath one may address a
    /// shot that only exists once the walk gets there - dropping it would throw
    /// away a step that works.
    #[test]
    fn an_unresolvable_entry_below_a_snapshot_pair_is_not_dropped() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        let buried = created_shot_entry(Some("uuid-that-is-not-there"), 0);
        stack.push_with_data(buried.command, buried.undo_data);
        stack.push_with_data(
            Command::Batch {
                commands: vec![],
                label: None,
            },
            UndoData::ProjectSnapshotPair {
                before: "{}".to_string(),
                after: "{}".to_string(),
            },
        );

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 0);
        assert_eq!(stack.len(), 2);
    }

    /// An entry whose `SetShotKind` edited the shot named by `uuid`, recorded
    /// against in-session id `shot_id`. This is the shape a REDO re-executes
    /// through `AppState::command_with_resolved_shot`, so it is the shape whose
    /// target has to still be there.
    fn kind_edit_entry(uuid: Option<&str>, shot_id: ShotId) -> UndoEntry {
        UndoEntry {
            command_bytes: None,
            command: Command::SetShotKind {
                line_id: 0,
                shot_id,
                kind: crate::models::ShotKind::Jump,
            },
            undo_data: UndoData::PreviousValues {
                line_id: 0,
                shot_id: Some(shot_id),
                shot_uuid: uuid.map(str::to_string),
                values_json: "{}".to_string(),
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        }
    }

    /// The undo-side wedge, one stack over: a redo entry that cannot resolve its
    /// shot fails, `redo_inner`'s epilogue pushes it back onto the redo stack,
    /// and every redo beneath it is sealed off.
    #[test]
    fn a_redo_entry_whose_shot_uuid_is_gone_is_dropped_and_the_redo_under_it_survives() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        stack.push_redo(kind_edit_entry(Some("live-uuid"), 7));
        stack.push_redo(kind_edit_entry(Some("uuid-that-is-not-there"), 9));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 1);
        assert_eq!(
            stack.redo_len(),
            1,
            "the redo beneath it must still be redoable"
        );
        assert!(matches!(
            stack.peek_redo(),
            Some(Command::SetShotKind { shot_id: 7, .. })
        ));
        assert_eq!(
            stack.estimated_redo_bytes(),
            UndoStack::estimated_size(&kind_edit_entry(Some("live-uuid"), 7)),
            "the dropped entry's bytes must leave the running total with it"
        );
    }

    #[test]
    fn a_redo_entry_whose_shot_uuid_still_resolves_is_kept() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        // The recorded in-session id is deliberately WRONG, exactly as on the
        // undo side: the uuid is what has to decide.
        stack.push_redo(kind_edit_entry(Some("live-uuid"), 4_242));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 0);
        assert_eq!(stack.redo_len(), 1);
    }

    /// A LEGACY redo entry has only its in-session id, and an id past the
    /// renumbered range is what wedges the redo.
    #[test]
    fn a_legacy_redo_entry_with_an_out_of_range_shot_id_is_dropped() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        stack.push_redo(kind_edit_entry(None, 4_242));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 1);
        assert_eq!(stack.redo_len(), 0);
    }

    /// The rule INVERTS for a `CreatedShotSnapshot` on the redo stack: that entry
    /// is there because the create was undone, so its shot is absent by
    /// construction and the redo puts it back. Filtering it by resolvability
    /// would drop every persisted create-a-shot redo there is.
    #[test]
    fn a_redo_entry_that_re_creates_its_shot_is_never_dropped() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        stack.push_redo(created_shot_entry(Some("not-there-until-the-redo"), 0));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 0);
        assert_eq!(stack.redo_len(), 1);
    }

    /// ...and it is a wall as well as an exception: a redo that re-creates a
    /// shot changes which shots exist, so nothing beneath it can be judged
    /// against the project as loaded.
    #[test]
    fn an_unresolvable_redo_entry_below_a_re_created_shot_is_not_dropped() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        stack.push_redo(kind_edit_entry(Some("uuid-that-is-not-there"), 9));
        stack.push_redo(created_shot_entry(Some("uuid-that-is-not-there"), 0));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 0);
        assert_eq!(stack.redo_len(), 2);
    }

    /// Both stacks in one pass, and the count is the total: a project reloaded
    /// after its shot went away can carry a doomed entry on each side.
    #[test]
    fn a_reload_drops_the_dead_entry_from_both_stacks() {
        let project = project_holding_one_shot("live-uuid");
        let mut stack = UndoStack::new();
        let doomed = created_shot_entry(Some("uuid-that-is-not-there"), 0);
        stack.push_with_data(doomed.command, doomed.undo_data);
        stack.push_redo(kind_edit_entry(Some("uuid-that-is-not-there"), 9));
        stack.push_redo(kind_edit_entry(Some("live-uuid"), 7));

        assert_eq!(stack.drop_entries_addressing_a_missing_shot(&project), 2);
        assert!(stack.is_empty());
        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn load_rejects_a_malformed_undo_timestamp_instead_of_treating_it_as_legacy() {
        let mut stack = UndoStack::new();
        let entry = UndoStack::serialize_entry_with_timestamp(&UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Malformed"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
            timestamp: None,
        })
        .unwrap();

        stack.load_from_persisted_with_timestamps(
            vec![(entry.0, entry.1, Some("not-a-timestamp".to_owned()))],
            vec![],
        );

        assert!(stack.is_empty());
    }

    #[test]
    fn load_rejects_a_missing_timestamp_after_a_stamped_entry() {
        let row = |name: &str, timestamp: Option<&str>| {
            let serialized = UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command(name),
                undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
                timestamp: None,
            })
            .unwrap();
            (serialized.0, serialized.1, timestamp.map(str::to_owned))
        };
        let mut stack = UndoStack::new();

        stack.load_from_persisted_with_timestamps(
            vec![
                row("Stamped", Some("2026-08-01T00:00:00+00:00")),
                row("Missing", None),
            ],
            vec![],
        );

        assert!(stack.is_empty());
    }

    /// Guards against over-filtering. `redo_inner`'s catch-all arm re-executes
    /// `entry.command` and never reads `undo_data`, so a `UndoData::None` REDO entry
    /// redoes correctly. Dropping it would destroy working functionality and shift
    /// the indices `redo_to` is called with.
    #[test]
    fn load_keeps_a_none_redo_entry() {
        let mut stack = UndoStack::new();

        let redo_entries: Vec<(String, String, Option<String>)> =
            vec![UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Redoable"),
                undo_data: UndoData::None,
                timestamp: None,
            })
            .unwrap()];

        stack.load_from_persisted_with_timestamps(vec![], redo_entries);

        assert_eq!(stack.redo_len(), 1, "the redo stack must not be filtered");
        assert!(stack.can_redo());
    }

    #[test]
    fn test_rows_to_persist_all_entries() {
        let mut stack = UndoStack::new();

        for i in 0..5 {
            stack.push_with_data(
                test_create_line_command(&format!("Line {i}")),
                UndoData::None,
            );
        }

        let persisted = stack.rows_to_persist(PersistStack::Undo, Some(10));
        assert_eq!(persisted.len(), 5);
    }

    #[test]
    fn test_rows_to_persist_limited() {
        let mut stack = UndoStack::new();

        for i in 0..10 {
            stack.push_with_data(
                test_create_line_command(&format!("Line {i}")),
                UndoData::None,
            );
        }

        let persisted = stack.rows_to_persist(PersistStack::Undo, Some(3));
        assert_eq!(persisted.len(), 3);

        // Should be the most recent 3
        assert!(
            persisted[0].0.contains("Line 7"),
            "the window is the TAIL of the stack, got {}",
            persisted[0].0
        );
    }

    // === Complex Operation Sequences ===

    #[test]
    fn test_undo_redo_cycle() {
        let mut stack = UndoStack::new();

        // Push command
        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("Test"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
            timestamp: None,
        };
        stack.push_with_data(entry.command.clone(), entry.undo_data);

        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        // Simulate undo: pop from undo, push to redo
        let undone = stack.pop_undo().unwrap();
        stack.push_redo(undone);

        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        // Simulate redo: pop from redo, push to undo
        let redone = stack.pop_redo().unwrap();
        stack.push_undo(redone);

        assert!(stack.can_undo());
        assert!(!stack.can_redo());
    }

    #[test]
    fn test_multiple_undo_redo_cycles() {
        let mut stack = UndoStack::new();

        // Push 3 commands
        for i in 0..3 {
            stack.push_with_data(
                test_create_line_command(&format!("Line {i}")),
                UndoData::InverseCommand(test_delete_line_command(i as u32)),
            );
        }

        assert_eq!(stack.len(), 3);

        // Undo all 3
        for _ in 0..3 {
            let entry = stack.pop_undo().unwrap();
            stack.push_redo(entry);
        }

        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 3);

        // Redo 2
        for _ in 0..2 {
            let entry = stack.pop_redo().unwrap();
            stack.push_undo(entry);
        }

        assert_eq!(stack.len(), 2);
        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_new_command_after_undo_clears_redo() {
        let mut stack = UndoStack::new();

        // Create -> Undo -> New action (should clear redo)
        stack.push_with_data(test_create_line_command("Original"), UndoData::None);

        let entry = stack.pop_undo().unwrap();
        stack.push_redo(entry);

        assert!(stack.can_redo());

        // New command should clear redo
        stack.push_with_data(test_create_line_command("New"), UndoData::None);

        assert!(!stack.can_redo());
        assert!(stack.can_undo());
    }

    #[test]
    fn test_roundtrip_serialization_all_undo_data_types() {
        // Test that all UndoData variants can be serialized and deserialized

        let test_cases: Vec<UndoEntry> = vec![
            UndoEntry {
                command_bytes: None,
                command: test_create_line_command("None"),
                undo_data: UndoData::None,
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Inverse"),
                undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: test_create_line_command("Created Snapshot"),
                undo_data: UndoData::CreatedLineSnapshot {
                    line_id: 3,
                    line_data: test_line_data("Created"),
                    identity: LineIdentity::default(),
                },
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: Command::CreateShot {
                    line_id: 1,
                    shot: ShotData {
                        length: 8.0,
                        ..Default::default()
                    },
                    after_shot_id: None,
                    before_shot_id: None,
                },
                undo_data: UndoData::CreatedShotSnapshot {
                    line_id: 1,
                    shot_id: 200,
                    shot_data: ShotData {
                        length: 8.0,
                        ..Default::default()
                    },
                    position: 0,
                    station_shift_from: 1,
                    after_shot_id: None,
                    before_shot_id: None,
                    after_shot_uuid: None,
                    before_shot_uuid: None,
                    uuid: None,
                    previous_offsets: HashMap::new(),
                    consumed_legacy: None,
                    created_station: None,
                },
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: Command::SetZeroDatum {
                    station_ref: StationRef::new(1, 0),
                    coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
                    accuracy_m: None,
                    entrance: None,
                },
                undo_data: UndoData::ZeroDatumInfo {
                    station_ref: StationRef::new(1, 0),
                    previous_gps: None,
                    previous_accuracy_m: None,
                    position: None,
                    previous_offsets: HashMap::new(),
                    previous_entrance: None,
                },
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: Command::Batch {
                    commands: vec![],
                    label: None,
                },
                undo_data: UndoData::ProjectSnapshotPair {
                    before: "{}".to_string(),
                    after: "{}".to_string(),
                },
                timestamp: None,
            },
            UndoEntry {
                command_bytes: None,
                command: Command::UpdateShot {
                    line_id: 1,
                    shot_id: 100,
                    updates: crate::models::ShotUpdates {
                        length: Some(10.0),
                        ..Default::default()
                    },
                },
                undo_data: UndoData::PreviousValues {
                    line_id: 1,
                    shot_id: Some(100),
                    shot_uuid: None,
                    values_json: r#"{"length":5.0}"#.to_string(),
                    previous_offsets: HashMap::new(),
                },
                timestamp: None,
            },
        ];

        for (i, original) in test_cases.iter().enumerate() {
            let serialized = UndoStack::serialize_entry(original);
            assert!(serialized.is_some(), "Failed to serialize test case {i}");

            let (cmd_json, undo_json) = serialized.unwrap();
            let deserialized = UndoStack::deserialize_entry(&cmd_json, &undo_json);
            assert!(
                deserialized.is_some(),
                "Failed to deserialize test case {i}"
            );
        }
    }

    // === [#303] Byte budget (spec items 7, 8, 9) ===

    /// An entry whose estimated size is `payload` bytes plus the flat overhead:
    /// a line-values edit, which the estimate charges its recorded JSON.
    fn sized_payload(payload: usize) -> UndoData {
        UndoData::PreviousValues {
            line_id: 0,
            shot_id: None,
            shot_uuid: None,
            values_json: "x".repeat(payload),
            previous_offsets: HashMap::new(),
        }
    }

    /// [`sized_payload`] under a command named `name`.
    fn sized_entry(payload: usize, name: &str) -> (Command, UndoData) {
        (test_create_line_command(name), sized_payload(payload))
    }

    /// [#303] The measured size of one `sized_entry`, so a budget can be expressed in
    /// entries rather than in a hardcoded overhead.
    ///
    /// These tests used to assume `1024 + ENTRY_OVERHEAD`, i.e. that the command costs a
    /// flat 512. It no longer does - the command is measured at push - so the constant
    /// both over-counted a small command and would under-count a large one. Deriving it
    /// keeps the tests about the BUDGET rather than about the estimator's internals.
    fn measured_entry_size(payload: usize) -> usize {
        let mut probe = UndoStack::new_with_limit(1000);
        let (cmd, data) = sized_entry(payload, "S0");
        probe.push_with_data(cmd, data);
        probe.estimated_undo_bytes()
    }

    fn assert_cached_byte_totals(stack: &UndoStack) {
        assert_eq!(
            stack.estimated_undo_bytes(),
            stack
                .undo_stack
                .iter()
                .map(UndoStack::estimated_size)
                .sum::<usize>()
        );
        assert_eq!(
            stack.estimated_redo_bytes(),
            stack
                .redo_stack
                .iter()
                .map(UndoStack::estimated_size)
                .sum::<usize>()
        );
    }

    #[test]
    fn running_byte_totals_follow_every_stack_mutation() {
        let mut stack = UndoStack::new_with_limit(10);
        for i in 0..3 {
            let (cmd, data) = sized_entry(128 + i * 32, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }
        assert_cached_byte_totals(&stack);

        let undone = stack.pop_undo().expect("entry to undo");
        stack.push_redo(undone);
        assert_cached_byte_totals(&stack);

        let redone = stack.pop_redo().expect("entry to redo");
        stack.push_undo(redone);
        assert_cached_byte_totals(&stack);

        stack.set_history_limit(2);
        assert_cached_byte_totals(&stack);

        let newest = stack.newest_stamp().expect("timestamped entries");
        stack.drop_entries_up_to(newest);
        assert_cached_byte_totals(&stack);

        let entry = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("persisted"),
            undo_data: sized_payload(256),
            timestamp: None,
        };
        let persisted = UndoStack::serialize_entry(&entry).expect("serialize entry");
        stack.load_from_persisted(vec![persisted]);
        assert_cached_byte_totals(&stack);

        stack.clear();
        assert_cached_byte_totals(&stack);
    }

    /// Spec item 7: Full mode keeps MORE than `undo_history_limit` entries. The count
    /// window is what the byte budget replaces, so a stack on the budget must not be
    /// truncated at 100 (or at whatever the user configured).
    #[test]
    fn undo_budget_keeps_more_than_the_count_limit() {
        let mut stack = UndoStack::new_with_limit(10);
        stack.set_byte_budget(10 * 1024 * 1024);

        for i in 0..50 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        assert_eq!(
            stack.len(),
            50,
            "the byte budget replaces the count window, it does not coexist with it"
        );
    }

    /// Spec item 8: past the budget, the OLDEST entries go first and the newest survive.
    #[test]
    fn undo_budget_drops_oldest_past_the_budget() {
        // Budget for exactly 4 entries, measured rather than assumed.
        let entry_size = measured_entry_size(1024);
        let mut stack = UndoStack::new_with_limit(1000);
        stack.set_byte_budget(entry_size * 4);

        for i in 0..10 {
            let (cmd, data) = sized_entry(1024, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        assert_eq!(stack.len(), 4, "bounded by bytes, not by count");
        assert!(
            stack.estimated_undo_bytes() <= entry_size * 4,
            "the surviving stack fits the budget"
        );
        // The NEWEST entry is the one the user just did, so it must be the last to go.
        let top = stack.pop_undo().expect("newest");
        match top.command {
            Command::CreateLine { name, .. } => assert_eq!(name, Some("S9".to_owned())),
            other => panic!("expected the newest entry, got {other:?}"),
        }
    }

    /// A single entry larger than the whole budget is KEPT: dropping it would silently
    /// discard the operation the user just performed, and one oversized entry is a very
    /// large import rather than runaway growth.
    #[test]
    fn undo_budget_always_keeps_the_newest_entry() {
        let mut stack = UndoStack::new_with_limit(1000);
        stack.set_byte_budget(1024);

        let (cmd, data) = sized_entry(64 * 1024, "huge");
        stack.push_with_data(cmd, data);

        assert_eq!(stack.len(), 1, "the newest entry survives its own size");
        assert!(stack.can_undo(), "and stays undoable");
    }

    /// Spec item 9: Disabled mode (and every local project) keeps the count window, and
    /// switching back to it from the budget re-applies the configured number.
    #[test]
    fn undo_budget_reverting_to_count_limit_retrims() {
        let mut stack = UndoStack::new_with_limit(3);
        stack.set_byte_budget(10 * 1024 * 1024);
        for i in 0..10 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }
        assert_eq!(stack.len(), 10);

        stack.use_count_limit();
        assert_eq!(stack.len(), 3, "back on the configured entry window");
        assert_eq!(stack.depth_limit(), DepthLimit::Count(3));
    }

    /// A config reload must not knock a Full-mode project off the byte budget: that
    /// would truncate the history its next push is supposed to carry to the server.
    #[test]
    fn undo_budget_survives_a_history_limit_change() {
        let mut stack = UndoStack::new_with_limit(100);
        stack.set_byte_budget(10 * 1024 * 1024);
        for i in 0..20 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        stack.set_history_limit(5);

        assert_eq!(stack.len(), 20, "still on the byte budget");
        assert!(matches!(stack.depth_limit(), DepthLimit::Bytes(_)));
        // ...and the new count is remembered for when the project goes back to it.
        stack.use_count_limit();
        assert_eq!(stack.len(), 5);
    }

    /// The estimator has to be dominated by the snapshot payloads - they are the only
    /// variants that reach megabytes, and the whole budget rests on that being true.
    #[test]
    fn undo_budget_size_estimate_tracks_the_snapshot_payload() {
        let small = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("small"),
            undo_data: UndoData::InverseCommand(test_delete_line_command(1)),
            timestamp: None,
        };
        let big = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("big"),
            undo_data: UndoData::ProjectSnapshotPair {
                before: "b".repeat(1_000_000),
                after: "a".repeat(1_000_000),
            },
            timestamp: None,
        };
        assert!(UndoStack::estimated_size(&small) < 1024);
        assert!(
            UndoStack::estimated_size(&big) >= 2_000_000,
            "a snapshot PAIR counts both halves - it is two whole-project JSON blobs"
        );
    }

    // === [#303] Persistence (spec item 10) ===

    /// Every save writes EVERY row, including a save with nothing changed since the last
    /// one.
    ///
    /// This is the regression guard for the bug that shipped in the first cut of #303:
    /// the writer used to skip rows it believed were already in the destination, which is
    /// false - `save_project` builds a fresh `.stalx.tmp` and renames it over the target,
    /// so a save that writes nothing produces a file with NO history. Live, that turned
    /// the commit made right after another save into one carrying no undo history at all.
    #[test]
    fn undo_rows_to_persist_writes_every_row_even_when_nothing_changed() {
        let mut stack = UndoStack::new_with_limit(1000);
        stack.set_byte_budget(64 * 1024 * 1024);
        for i in 0..5 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        let first = stack.rows_to_persist(PersistStack::Undo, None);
        assert_eq!(first.len(), 5);

        // The SAME call again, with no mutation in between - an idle autosave, or the
        // push's flush right after another save. It must still carry all five.
        let again = stack.rows_to_persist(PersistStack::Undo, None);
        assert_eq!(
            again.len(),
            5,
            "a save with nothing changed still rewrites the whole stack - the file it \
             writes into is brand new"
        );
        assert_eq!(first, again);
    }

    /// A byte-budget trim evicts from the FRONT, and the next save writes exactly what
    /// survived - in stack order, with no trace of the evicted entry.
    #[test]
    fn undo_rows_to_persist_follows_a_trim() {
        let entry_size = measured_entry_size(1024);
        let mut stack = UndoStack::new_with_limit(1000);
        stack.set_byte_budget(entry_size * 3);
        for i in 0..3 {
            let (cmd, data) = sized_entry(1024, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }
        assert_eq!(stack.rows_to_persist(PersistStack::Undo, None).len(), 3);

        // A fourth push evicts S0.
        let (cmd, data) = sized_entry(1024, "S3");
        stack.push_with_data(cmd, data);
        let rows = stack.rows_to_persist(PersistStack::Undo, None);
        assert_eq!(rows.len(), 3);
        assert!(
            !rows.iter().any(|(cmd_json, ..)| cmd_json.contains("S0")),
            "the evicted entry must not be written back"
        );
        for (i, (cmd_json, ..)) in rows.iter().enumerate() {
            assert!(
                cmd_json.contains(&format!("S{}", i + 1)),
                "the survivors keep stack order, got {cmd_json} at {i}"
            );
        }
    }

    /// An undo pops the tail onto the redo stack, so the next save writes it on the redo
    /// side and no longer on the undo side.
    #[test]
    fn undo_rows_to_persist_retires_a_popped_tail() {
        let mut stack = UndoStack::new_with_limit(1000);
        stack.set_byte_budget(64 * 1024 * 1024);
        for i in 0..3 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        let popped = stack.pop_undo().expect("pop");
        stack.push_redo(popped);

        let undo_rows = stack.rows_to_persist(PersistStack::Undo, None);
        assert_eq!(undo_rows.len(), 2, "the survivors are still written");
        assert!(
            !undo_rows
                .iter()
                .any(|(cmd_json, ..)| cmd_json.contains("S2")),
            "the popped entry is no longer on the undo side"
        );
        // The redo side gained the entry.
        assert_eq!(stack.rows_to_persist(PersistStack::Redo, None).len(), 1);
    }

    /// A count-limited project (local, or Disabled mode) persists a WINDOW over the tail
    /// of the stack, and that window's start moves with every push - so those saves
    /// rewrite it, exactly as they always did.
    #[test]
    fn undo_rows_to_persist_count_limit_writes_the_window() {
        let mut stack = UndoStack::new_with_limit(1000);
        for i in 0..10 {
            let (cmd, data) = sized_entry(64, &format!("S{i}"));
            stack.push_with_data(cmd, data);
        }

        let rows = stack.rows_to_persist(PersistStack::Undo, Some(3));
        assert_eq!(rows.len(), 3, "only the last 3 entries are persisted");
        for (i, (cmd_json, ..)) in rows.iter().enumerate() {
            assert!(
                cmd_json.contains(&format!("S{}", i + 7)),
                "the window is the TAIL, in order, got {cmd_json} at {i}"
            );
        }
    }

    /// The next save after a load writes the loaded entries out in full - the rows on
    /// disk are not the entries in memory (a load FILTERS and TRIMS), so the file is
    /// rebuilt from memory rather than assumed to already match it.
    #[test]
    fn undo_rows_to_persist_after_load_writes_everything() {
        let mut stack = UndoStack::new();
        let entries: Vec<(String, String, Option<String>)> = (0..3)
            .map(|i| {
                UndoStack::serialize_entry_with_timestamp(&UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("S{i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(i)),
                    timestamp: None,
                })
                .expect("serialize")
            })
            .collect();
        stack.load_from_persisted_with_timestamps(entries.clone(), vec![]);

        assert_eq!(
            stack.rows_to_persist(PersistStack::Undo, None),
            entries,
            "a load is followed by a full rewrite, not by a save that assumes the file \
             already holds what memory does"
        );
    }

    // === [#303] clear-on-push (spec items 4, 5, 11) ===

    /// [#303] The stack orders itself: a new entry never lands at or below the one under
    /// it, whatever the wall clock says.
    ///
    /// `drop_entries_up_to` uses `partition_point`, which needs the stamps sorted to mean
    /// anything, and the push-time cut compares entries against a watermark. Both read a
    /// stack whose order can otherwise be broken from outside: a chain step loads entries
    /// stamped by the machine that authored them, and a local clock can be corrected
    /// backwards mid-session.
    #[test]
    fn stamps_stay_strictly_increasing_against_a_clock_that_does_not() {
        let mut stack = UndoStack::new();
        let push = |stack: &mut UndoStack, i: u32| {
            stack.push_with_data(
                test_create_line_command(&format!("S{i}")),
                UndoData::InverseCommand(test_delete_line_command(i)),
            );
        };

        // An entry loaded from a peer whose clock runs a day fast.
        push(&mut stack, 0);
        let future = Utc::now() + chrono::Duration::days(1);
        stack.undo_stack.back_mut().expect("seeded").timestamp = Some(future);

        // Everything the user does now is stamped ABOVE it, even though `now` is a day
        // behind. Without this, the watermark cut would classify their own work as
        // inherited and drop it from the upload.
        for i in 1..4 {
            push(&mut stack, i);
        }

        // `undo_entries` yields most-recent FIRST, so reverse into stack order (oldest
        // first) - the order the invariant is about and the one `partition_point` walks.
        let mut stamps: Vec<_> = stack
            .undo_entries()
            .map(|e| e.timestamp.expect("stamped"))
            .collect();
        stamps.reverse();
        assert!(
            stamps.windows(2).all(|w| w[0] < w[1]),
            "stamps must be strictly increasing in stack order, got {stamps:?}"
        );
        assert!(
            stamps[1] > future,
            "the user's first edit must sit above the peer's future-dated entry"
        );
        assert_eq!(
            stack.newest_stamp(),
            stamps.last().copied(),
            "newest_stamp is the back, which the invariant makes the maximum"
        );
    }

    /// [#303] The byte budget counts the COMMAND too, not just the undo data.
    ///
    /// `estimated_size` charged the command a flat `ENTRY_OVERHEAD`, but `Command::Batch`
    /// and the `shots:` variants are unbounded and `rows_to_persist` writes that
    /// `command_json` - and in Full mode ships it. So an entire import counted as 512
    /// bytes against a 32 MiB ceiling whose own doc names Batch as the reason it exists.
    #[test]
    fn the_budget_counts_a_large_command_not_a_flat_overhead() {
        let mut stack = UndoStack::new();
        // One command carrying a survey's worth of shots, with NO undo payload - so
        // anything the estimate reports above the flat overhead came from the command.
        let shots: Vec<_> = (0..2_000)
            .map(|_| ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 10.0,
                depth_in: 5.0,
                depth_out: 6.0,
                ..Default::default()
            })
            .collect();
        stack.push_with_data(
            Command::CreateLine {
                name: Some("Imported".to_string()),
                surveyor: None,
                date: None,
                color: "#ff0000".to_string(),
                direction: None,
                shots,
                cave_section_id: None,
            },
            UndoData::None,
        );

        let size = stack.estimated_undo_bytes();
        assert!(
            size > 100_000,
            "a 2,000-shot command must count its own payload, got {size} bytes"
        );
    }

    /// Restored entries deliberately retain the 512-byte command estimate. The undo
    /// payload remains separately counted, so this approximation does not erase the
    /// dominant term for snapshot-heavy history.
    #[test]
    fn restored_entries_use_the_documented_flat_command_estimate() {
        let small = UndoEntry {
            command_bytes: None,
            command: test_create_line_command("S0"),
            undo_data: UndoData::None,
            timestamp: None,
        };
        assert_eq!(UndoStack::estimated_size(&small), 512);

        let payload = UndoEntry {
            undo_data: UndoData::ProjectSnapshotPair {
                before: "x".repeat(2_048),
                after: String::new(),
            },
            ..small
        };
        assert_eq!(UndoStack::estimated_size(&payload), 2_048 + 512);
    }

    /// EVERY variant that owns a `previous_offsets` map charges for it.
    ///
    /// `PreviousValues` and `CreatedShotSnapshot` gained theirs late (#413,
    /// finding 27) and were charged as if they had none. The map is the one
    /// field on those two that grows with the SURVEY rather than with the edit -
    /// the rigid connected set of the edited line - and a shot edit records
    /// one per keystroke, so leaving it uncounted let a Full-mode history grow
    /// past its budget while the estimate said it had not.
    #[test]
    fn the_estimate_counts_a_previous_offsets_map_on_the_two_variants_that_gained_one() {
        let offsets: HashMap<LineId, Option<(f64, f64)>> =
            (0..4).map(|id| (id, Some((1.0, 2.0)))).collect();

        let previous_values = |previous_offsets: HashMap<LineId, Option<(f64, f64)>>| UndoEntry {
            command_bytes: None,
            command: test_create_line_command("S0"),
            undo_data: UndoData::PreviousValues {
                line_id: 0,
                shot_id: Some(1),
                shot_uuid: None,
                values_json: "x".repeat(64),
                previous_offsets,
            },
            timestamp: None,
        };
        assert_eq!(
            UndoStack::estimated_size(&previous_values(HashMap::new())),
            64 + 512
        );
        assert_eq!(
            UndoStack::estimated_size(&previous_values(offsets.clone())),
            64 + 512 + 4 * 64,
            "a shot edit's recorded offsets are 64 bytes apiece, as on ZeroDatumInfo"
        );

        let created_shot = |previous_offsets: HashMap<LineId, Option<(f64, f64)>>| {
            let mut entry = created_shot_entry(Some("uuid"), 1);
            if let UndoData::CreatedShotSnapshot {
                previous_offsets: slot,
                ..
            } = &mut entry.undo_data
            {
                *slot = previous_offsets;
            }
            entry
        };
        assert_eq!(
            UndoStack::estimated_size(&created_shot(HashMap::new())),
            420 + 512
        );
        assert_eq!(
            UndoStack::estimated_size(&created_shot(offsets)),
            420 + 512 + 4 * 64,
            "an insert's recorded offsets count on top of the shot itself"
        );
    }

    /// [#303] A load whose stamps are not in time order is refused, not sorted.
    ///
    /// `drop_entries_up_to` finds the post-push cut with `partition_point`, which needs a
    /// monotone predicate; over an unsorted stack it returns an arbitrary boundary and the
    /// drain then deletes a wrong prefix. Sorting would fix the search and break the
    /// meaning - stack order IS undo order - so a stack that cannot be trusted is dropped.
    #[test]
    fn a_load_with_out_of_order_stamps_is_dropped_rather_than_trusted() {
        let now = Utc::now();
        let rows: Vec<(String, String, Option<String>)> = [now + chrono::Duration::minutes(5), now]
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("S{i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(
                        u32::try_from(i).expect("small"),
                    )),
                    timestamp: Some(*ts),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).expect("serializes")
            })
            .collect();

        let mut stack = UndoStack::new();
        stack.load_from_persisted_with_timestamps(rows, Vec::new());

        assert!(
            stack.is_empty(),
            "a non-monotonic undo stack must be refused, not silently reordered"
        );
    }

    /// The ordinary case is untouched: an in-order load keeps every entry.
    #[test]
    fn an_in_order_load_keeps_its_entries() {
        let now = Utc::now();
        let rows: Vec<(String, String, Option<String>)> = [now, now + chrono::Duration::minutes(5)]
            .iter()
            .enumerate()
            .map(|(i, ts)| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("S{i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(
                        u32::try_from(i).expect("small"),
                    )),
                    timestamp: Some(*ts),
                };
                UndoStack::serialize_entry_with_timestamp(&entry).expect("serializes")
            })
            .collect();

        let mut stack = UndoStack::new();
        stack.load_from_persisted_with_timestamps(rows, Vec::new());

        assert_eq!(stack.len(), 2, "an in-order load must be kept intact");
    }

    /// [#303] The watermark is the SECOND floor, and clearing the entry beneath is not
    /// enough to clear it.
    ///
    /// The shape that bit: a peer's clock runs a day fast, so a chain step loads rows
    /// stamped across (T+1d-5min .. T+1d) and the watermark lands at T+1d. Undo back to
    /// the OLDEST loaded row and edit: staying above that neighbour gives T+1d-5min+1ns,
    /// five minutes UNDER the watermark - and `cut_inherited_undo_rows` deletes every row
    /// at or below it, so the user's own entry never reaches the commit.
    #[test]
    fn a_new_entry_clears_the_inherited_watermark_even_from_deep_in_a_walk() {
        let mut stack = UndoStack::new();
        let watermark = Utc::now() + chrono::Duration::days(1);
        let oldest_loaded = watermark - chrono::Duration::minutes(5);

        // The stack as an Unwind leaves it: loaded rows, then undone back to the oldest,
        // so the entry BENEATH a new push is the five-minutes-under one.
        stack.push_with_data(
            test_create_line_command("loaded"),
            UndoData::InverseCommand(test_delete_line_command(0)),
        );
        stack.undo_stack.back_mut().expect("seeded").timestamp = Some(oldest_loaded);
        stack.set_inherited_through(Some(watermark));

        stack.push_with_data(
            test_create_line_command("mine"),
            UndoData::InverseCommand(test_delete_line_command(1)),
        );

        let mine = stack.newest_stamp().expect("stamped");
        assert!(
            mine > watermark,
            "a new entry must sit ABOVE the watermark ({watermark:?}), got {mine:?} - at or \
             below it the push cuts it as inherited history"
        );
    }

    /// Without a watermark nothing changes: the clamp must not invent a floor.
    #[test]
    fn no_watermark_leaves_the_stamp_at_the_real_instant() {
        let mut stack = UndoStack::new();
        stack.set_inherited_through(None);
        let before = Utc::now();
        stack.push_with_data(
            test_create_line_command("S0"),
            UndoData::InverseCommand(test_delete_line_command(0)),
        );
        let after = Utc::now();
        let stamp = stack.newest_stamp().expect("stamped");
        assert!(
            stamp >= before && stamp <= after,
            "unexpected clamp: {stamp:?}"
        );
    }

    /// A watermark already in the past is not a floor at all - the wall clock clears it.
    #[test]
    fn a_past_watermark_does_not_push_the_stamp_forward() {
        let mut stack = UndoStack::new();
        stack.set_inherited_through(Some(Utc::now() - chrono::Duration::days(1)));
        let before = Utc::now();
        stack.push_with_data(
            test_create_line_command("S0"),
            UndoData::InverseCommand(test_delete_line_command(0)),
        );
        let after = Utc::now();
        let stamp = stack.newest_stamp().expect("stamped");
        assert!(
            stamp >= before && stamp <= after,
            "unexpected clamp: {stamp:?}"
        );
    }

    /// The ordinary case must keep telling the truth: with a sane clock the stamp IS the
    /// wall-clock instant, so displayed times stay honest and the clamp is invisible.
    #[test]
    fn an_ordinary_push_is_stamped_at_the_real_instant() {
        let mut stack = UndoStack::new();
        let before = Utc::now();
        stack.push_with_data(
            test_create_line_command("S0"),
            UndoData::InverseCommand(test_delete_line_command(0)),
        );
        let after = Utc::now();
        let stamp = stack.newest_stamp().expect("stamped");
        assert!(
            stamp >= before && stamp <= after,
            "expected the real instant, got {stamp:?} outside {before:?}..{after:?}"
        );
    }

    /// Spec items 4 + 5: the push-start boundary. Entries captured by the push go (they
    /// are on the server now, and keeping them would duplicate them in the next commit);
    /// an edit that landed AFTER the snapshot was taken STAYS, because its dirty bit
    /// stays too and the next push has to carry both the edit and its history.
    #[test]
    fn drop_entries_up_to_keeps_edits_that_landed_after_push_start() {
        let mut stack = UndoStack::new();
        let t =
            |secs: i64| DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp");

        for (i, secs) in [0, 10, 20, 30].into_iter().enumerate() {
            stack.push_with_data(
                test_create_line_command(&format!("S{i}")),
                UndoData::InverseCommand(test_delete_line_command(i as u32)),
            );
            // Overwrite the auto-assigned timestamp so the boundary is deterministic.
            if let Some(entry) = stack.undo_stack.back_mut() {
                entry.timestamp = Some(t(secs));
            }
        }
        // A push that started at t=25: the first three entries are in its snapshot.
        let dropped = stack.drop_entries_up_to(t(25));

        assert_eq!(dropped, 3);
        assert_eq!(stack.len(), 1, "the post-push-start edit survives");
        let survivor = stack.peek_undo().expect("survivor");
        match survivor {
            Command::CreateLine { name, .. } => assert_eq!(*name, Some("S3".to_owned())),
            other => panic!("expected S3, got {other:?}"),
        }
    }

    #[test]
    fn drop_entries_up_to_drops_the_entry_exactly_at_the_cutoff() {
        let mut stack = UndoStack::new();
        let t =
            |secs: i64| DateTime::from_timestamp(1_700_000_000 + secs, 0).expect("valid timestamp");

        for (i, secs) in [10, 20, 30].into_iter().enumerate() {
            stack.push_with_data(
                test_create_line_command(&format!("S{i}")),
                UndoData::InverseCommand(test_delete_line_command(i as u32)),
            );
            stack.undo_stack.back_mut().unwrap().timestamp = Some(t(secs));
        }

        let dropped = stack.drop_entries_up_to(t(20));

        assert_eq!(dropped, 2, "the entry exactly on the upload cutoff is gone");
        assert_eq!(stack.len(), 1);
        assert!(matches!(
            stack.peek_undo(),
            Some(Command::CreateLine { name: Some(name), .. }) if name == "S2"
        ));
    }

    /// Spec item 11: a confirmed push clears the redo stack outright. Redo is never
    /// uploaded, and a redo branch that replays onto a state the server has now
    /// committed is the stale-replay hazard the whole design avoids.
    #[test]
    fn drop_entries_up_to_clears_redo_wholesale() {
        let mut stack = UndoStack::new();
        stack.push_with_data(
            test_create_line_command("a"),
            UndoData::InverseCommand(test_delete_line_command(1)),
        );
        let entry = stack.pop_undo().expect("pop");
        stack.push_redo(entry);
        assert!(stack.can_redo());

        stack.drop_entries_up_to(Utc::now());

        assert!(!stack.can_redo(), "redo goes wholesale on a confirmed push");
        assert!(!stack.can_undo());
    }

    /// [#303] Un-timestamped entries are always a PREFIX of the undo stack - never a
    /// stamped entry with a `None` behind it.
    ///
    /// `drop_entries_up_to` finds its boundary with `partition_point`, which is only
    /// meaningful over a predicate that partitions: `timestamp.is_none_or(|ts| ts <=
    /// cutoff)` must be true for a prefix and false thereafter, and a `None` sitting above
    /// a stamped entry breaks that - `partition_point` would then return a boundary that
    /// keeps rows it should cut, or cuts rows it should keep, with no error either way.
    ///
    /// It holds because of how entries get here, and this pins each leg: a legacy file is
    /// uniformly unstamped (the load is the only source of `None`), every new entry is
    /// stamped by `next_stamp` and appended at the BACK, redo returns entries to the back
    /// in the order they left it, and both trims evict from the FRONT.
    #[test]
    fn untimestamped_entries_stay_a_prefix_of_the_undo_stack() {
        /// True while no un-stamped entry sits above a stamped one.
        fn none_timestamps_are_a_prefix(stack: &UndoStack) -> bool {
            let first_stamped = stack
                .undo_stack
                .iter()
                .position(|e| e.timestamp.is_some())
                .unwrap_or(stack.undo_stack.len());
            stack
                .undo_stack
                .iter()
                .skip(first_stamped)
                .all(|e| e.timestamp.is_some())
        }

        // A legacy file: every restored entry is un-stamped, so the prefix is the lot.
        let mut stack = UndoStack::new_with_limit(4);
        let legacy: Vec<_> = (0..3)
            .map(|i| {
                let entry = UndoEntry {
                    command_bytes: None,
                    command: test_create_line_command(&format!("legacy{i}")),
                    undo_data: UndoData::InverseCommand(test_delete_line_command(i)),
                    timestamp: None,
                };
                UndoStack::serialize_entry_with_timestamp(&entry).expect("serialize")
            })
            .collect();
        stack.load_from_persisted_with_timestamps(legacy, vec![]);
        assert!(stack.undo_entries().all(|e| e.timestamp.is_none()));
        assert!(none_timestamps_are_a_prefix(&stack));

        // A new edit is stamped and lands at the back, so the un-stamped rows stay below.
        stack.push_with_data(
            test_create_line_command("mine"),
            UndoData::InverseCommand(test_delete_line_command(9)),
        );
        assert!(none_timestamps_are_a_prefix(&stack));

        // Undo then redo: the stamped entry leaves the back and comes back to it, so the
        // order is preserved rather than rebuilt.
        let popped = stack.pop_undo().expect("undo the new edit");
        assert!(none_timestamps_are_a_prefix(&stack));
        stack.push_redo(popped);
        let replayed = stack.pop_redo().expect("redo it");
        stack.push_undo(replayed);
        assert!(
            none_timestamps_are_a_prefix(&stack),
            "redo returns an entry to the BACK, so it cannot land beneath a legacy row"
        );

        // And a trim evicts from the FRONT, which can only ever shorten the prefix.
        for i in 0..3 {
            stack.push_with_data(
                test_create_line_command(&format!("more{i}")),
                UndoData::InverseCommand(test_delete_line_command(20 + i)),
            );
        }
        assert_eq!(
            stack.len(),
            4,
            "the count window trimmed the oldest entries"
        );
        assert!(none_timestamps_are_a_prefix(&stack));
        assert!(
            stack.undo_entries().all(|e| e.timestamp.is_some()),
            "the evicted entries are the legacy ones at the front"
        );
    }

    /// A legacy persisted entry carries no timestamp. It necessarily predates this
    /// session's push, so it is dropped rather than kept forever.
    #[test]
    fn drop_entries_up_to_drops_untimestamped_legacy_entries() {
        let mut stack = UndoStack::new();
        stack.push_with_data(
            test_create_line_command("legacy"),
            UndoData::InverseCommand(test_delete_line_command(1)),
        );
        if let Some(entry) = stack.undo_stack.back_mut() {
            entry.timestamp = None;
        }

        assert_eq!(stack.drop_entries_up_to(Utc::now()), 1);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_undo_data_closure_info_serialization() {
        let closure = LoopClosure {
            id: 1,
            station_a: StationRef::new(1, 0).into(),
            station_b: StationRef::new(2, 3).into(),
            path_length: 45.5,
            misclosure_distance: 1.2,
            error_percentage: 2.6,
            horizontal_misclosure: 1.0,
            vertical_misclosure: 0.7,
            strategy: crate::models::ClosureStrategy::ProportionalByLength,
            is_aligned: false,
        };

        let entry = UndoEntry {
            command_bytes: None,
            command: Command::CreateLoopClosure {
                station_a: StationRef::new(1, 0),
                station_b: StationRef::new(2, 3),
                closure_strategy: crate::models::ClosureStrategy::ProportionalByLength,
            },
            undo_data: UndoData::ClosureInfo {
                closure,
                position: None,
            },
            timestamp: None,
        };

        // Serialize
        let (cmd_json, undo_json) =
            UndoStack::serialize_entry(&entry).expect("ClosureInfo should serialize");

        // Deserialize
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json)
            .expect("ClosureInfo should deserialize");

        // Verify roundtrip
        match &restored.undo_data {
            UndoData::ClosureInfo {
                closure: restored_closure,
                ..
            } => {
                // Pure serialization roundtrip (no project / no reanchor): the
                // transitional Index anchors survive verbatim, so compare keys directly.
                assert_eq!(restored_closure.station_a, StationRef::new(1, 0).into());
                assert_eq!(restored_closure.station_b, StationRef::new(2, 3).into());
                assert!((restored_closure.path_length - 45.5).abs() < 1e-6);
                assert!((restored_closure.misclosure_distance - 1.2).abs() < 1e-6);
                assert!((restored_closure.error_percentage - 2.6).abs() < 1e-6);
                assert!((restored_closure.horizontal_misclosure - 1.0).abs() < 1e-6);
                assert!((restored_closure.vertical_misclosure - 0.7).abs() < 1e-6);
            }
            other => panic!("Expected ClosureInfo, got {}", other.variant_name()),
        }

        match &restored.command {
            Command::CreateLoopClosure {
                station_a,
                station_b,
                ..
            } => {
                assert_eq!(*station_a, StationRef::new(1, 0));
                assert_eq!(*station_b, StationRef::new(2, 3));
            }
            _ => panic!("Expected CreateLoopClosure command"),
        }
    }

    // The two tests below moved here from the persistence sqlite tests when
    // persistence became a workspace member: they assert against `UndoStack`'s
    // entry costing and (de)serialization, which that crate cannot see.

    /// [#469] The default ceiling is DERIVED from the byte budget, and the derivation
    /// only holds while a restored entry costs at least `ENTRY_OVERHEAD_BYTES`.
    ///
    /// That is what makes the cap lossless: past `budget / ENTRY_OVERHEAD_BYTES`
    /// entries the byte trim necessarily evicts, so the rows the cap does not read are
    /// rows the restore would have thrown away.
    #[test]
    fn the_default_history_load_ceiling_is_derived_from_the_byte_budget() {
        // The DERIVATION, not the product: `budget == rows * overhead` only holds while
        // the budget happens to divide evenly, and a budget that does not is still a
        // sound ceiling (the truncating division rounds DOWN, which can only be safer).
        assert_eq!(
            MAX_PERSISTED_HISTORY_ROWS,
            UNDO_BYTE_BUDGET_BYTES / ENTRY_OVERHEAD_BYTES,
            "the load ceiling is the byte budget divided by the per-entry floor"
        );
        // The floor itself: a restored entry carries no `command_bytes`, so it is
        // charged the overhead whatever its command was.
        let restored = UndoStack::deserialize_entry_with_timestamp(
            r#"{"type":"close_project"}"#,
            r#"{"v2":"None"}"#,
            None,
        )
        .unwrap();
        assert!(restored.command_bytes.is_none());
        assert!(
            UndoStack::estimated_size(&restored) >= ENTRY_OVERHEAD_BYTES,
            "a restored entry must cost at least the overhead the ceiling is derived from"
        );
        // ...and the same holds for the emptiest entry that can be constructed at all.
        let bare = UndoEntry {
            command_bytes: None,
            command: Command::CloseProject,
            undo_data: UndoData::None,
            timestamp: None,
        };
        assert_eq!(UndoStack::estimated_size(&bare), ENTRY_OVERHEAD_BYTES);
    }

    #[test]
    fn test_new_format_undo_history_survives_reload() {
        use std::collections::HashMap;

        use crate::models::{LineData, Project, ShotUpdates};
        use crate::persistence::sqlite::Database;
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("new_format_undo.stalx");

        let mut project = Project::new("New Undo".to_string(), None);
        project.add_line(LineData {
            shots: vec![ShotData {
                azimuth_in: 45.0,
                azimuth_out: 225.0,
                length: 2.0,
                depth_in: 10.0,
                depth_out: 11.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        let saved = db.load_project().unwrap();
        let line_id = saved.lines[0].id;
        let shot_id = saved.lines[0].shots[0].id;

        // New-format entry: only the edited field is present (untouched nullable
        // fields are omitted thanks to skip_serializing_if).
        let prev_values = ShotUpdates {
            comment: Some(Some("original".to_string())),
            ..Default::default()
        };
        let values_json = serde_json::to_string(&prev_values).unwrap();
        assert!(
            !values_json.contains("left") && !values_json.contains("pitch_in"),
            "untouched nullable fields must serialize as absent, got: {values_json}"
        );
        let entry = UndoEntry {
            command_bytes: None,
            command: Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    comment: Some(Some("edited".to_string())),
                    ..Default::default()
                },
            },
            undo_data: UndoData::PreviousValues {
                line_id,
                shot_id: Some(shot_id),
                shot_uuid: None,
                values_json,
                previous_offsets: HashMap::new(),
            },
            timestamp: None,
        };
        let persisted = UndoStack::serialize_entry_with_timestamp(&entry).unwrap();
        db.save_undo_stack_with_timestamps(vec![persisted]).unwrap();
        drop(db);

        // Reload: the v7-stamped file keeps its history.
        let db = Database::open_existing(&path).unwrap();
        let loaded = db.load_undo_stack_with_timestamps().unwrap();
        assert_eq!(loaded.len(), 1, "new-format undo entry must survive reload");

        let (_, cmd_json, undo_json, ts) = &loaded[0];
        let restored =
            UndoStack::deserialize_entry_with_timestamp(cmd_json, undo_json, ts.as_deref())
                .unwrap();
        match &restored.undo_data {
            UndoData::PreviousValues { values_json, .. } => {
                let prev: ShotUpdates = serde_json::from_str(values_json).unwrap();
                // Exactly the edited field round-trips...
                assert_eq!(prev.comment, Some(Some("original".to_string())));
                // ...and untouched nullable fields stay untouched (None), so
                // replaying this entry cannot clear them.
                assert_eq!(prev.left, None);
                assert_eq!(prev.pitch_in, None);
                assert_eq!(prev.marker_index, None);
                assert_eq!(prev.temperature, None);
            }
            other => panic!("Expected PreviousValues, got {}", other.variant_name()),
        }
    }
}
