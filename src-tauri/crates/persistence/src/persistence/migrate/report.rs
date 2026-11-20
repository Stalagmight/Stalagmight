//! What a migration did (plan 7.3): every normalisation and conversion rule
//! that fired, with counts, and the verification of the result against the
//! legacy positions. Logged and returned to tests; never shown raw to users.
//!
//! Nothing here holds survey content: only counts, displacements in metres
//! and, for an unexplained mismatch, the slot (line id and station index) it
//! was found at. [`MigrationReport::summary`] renders counts only.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::models::LineId;

/// The counts of one migration.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MigrationReport {
    /// The schema version stamped in the legacy file.
    pub schema_version: i32,
    pub lines: usize,
    pub shots: usize,
    /// Station slots of the legacy project (`shots + 1` per line).
    pub slots: usize,
    /// Stations of the migrated project.
    pub stations: usize,

    // Merges (plan 7.2.1).
    /// Merges whose two members became one station.
    pub merges_collapsed: usize,
    /// Merges that would have closed a cycle, kept as `Tie` closures.
    pub merges_demoted_cycle: usize,
    /// Merges with both ends on one line, kept as `Tie` closures.
    pub merges_demoted_same_line: usize,
    /// Merges of a slot with itself, dropped.
    pub merges_dropped_self: usize,
    /// Merges whose ends were already one station (repeats), dropped.
    pub merges_dropped_redundant: usize,
    /// Merges with an end that does not resolve, dropped.
    pub merges_dropped_dangling: usize,

    // Station identity.
    /// Retired member ids kept as aliases of their station.
    pub aliases_created: usize,
    /// Member ids dropped because they were out of range or already used.
    pub station_ids_dropped: usize,
    /// Stations that had no usable id and were given a fresh one.
    pub station_ids_minted: usize,
    /// Stations renamed (letter suffix) to keep names unique on every line.
    pub stations_renamed: usize,
    /// Stations whose members' distinct comments were joined.
    pub comments_joined: usize,
    /// Name, id, comment or hand-move keys that resolve to no station.
    pub station_keys_dangling: usize,
    /// Keys dropped because another key of the same map named the same slot.
    pub station_keys_duplicate: usize,

    // Shots.
    /// Shots whose uuid was missing, too long or repeated, minted here.
    pub shot_uuids_minted: usize,
    /// Non-finite numbers (shot readings, line quality, hand moves) replaced
    /// or dropped, as the 0.6.0 save heal did.
    pub numbers_sanitized: usize,

    // Closures (plan 7.2.4).
    pub closures_kept: usize,
    /// Legacy aligned non-`Move` closures converted to `Tie`.
    pub aligned_closures_to_tie: usize,
    /// Closures with an end that does not resolve, dropped.
    pub closures_dropped_dangling: usize,
    /// Closures whose two ends became one station, dropped.
    pub closures_dropped_one_station: usize,
    /// Closures across two pieces not both held by a datum, dropped.
    pub closures_dropped_unanchored: usize,
    /// Closures whose id was zero or repeated, given a fresh id.
    pub closure_ids_reminted: usize,

    // Datums and anchors (plan 7.2.5, 7.2.6).
    pub datums_kept: usize,
    /// Datums whose station does not resolve, dropped.
    pub datums_dropped_dangling: usize,
    /// Placement datums given a correction (their piece sits off its GPS).
    pub corrections_stored: usize,
    /// Datum-less pieces given a `Placement`, by where its station came from.
    pub placements_held_gauge: usize,
    pub placements_default_gauge: usize,
    /// No legacy gauge applied (lowest station at its legacy position).
    pub placements_fallback: usize,
    /// Datums given an explicit entrance flag (new default differs).
    pub entrance_overrides_written: usize,

    // Hand moves (plan 7.2.3).
    pub hand_moves_carried: usize,

    // Per-file state (plan 7.2.9).
    /// Ignored warnings kept under the uuid they were stored with.
    pub ignores_kept: usize,
    /// Ignored warnings re-keyed (a renamed or minted uuid, a pre-v8 row
    /// keyed by session shot id).
    pub ignores_rekeyed: usize,
    /// Ignored warnings whose shot no longer exists, or repeats, dropped.
    pub ignores_dropped: usize,
    /// Legacy-only state with no place in the new model: line gauge pairs.
    pub gauge_pairs_dropped: usize,
    /// Legacy held gauges (they became `Placement` stations).
    pub held_gauges_dropped: usize,

    // History (plan 7.2.8).
    /// Undo and redo rows the file held; history is never migrated.
    pub history_rows_dropped: u64,

    // What the legacy load itself changed (for the record).
    pub legacy_derived_shot_uuids: usize,
    pub legacy_renamed_shot_uuids: usize,
    pub legacy_healed_offsets: usize,
    pub legacy_merges_created: usize,
    pub legacy_integrity_findings: usize,

    /// The comparison with the legacy positions (plan 7.2.7).
    pub verification: Verification,
}

/// Per-slot comparison of the migrated positions with the legacy ones.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Verification {
    /// Legacy slots compared.
    pub slots_checked: usize,
    /// Largest horizontal displacement of any slot, metres.
    pub max_xy_m: f64,
    /// Largest depth difference of any slot, metres.
    pub max_z_m: f64,
    /// Differences that a known conversion rule explains, by class.
    pub expected: BTreeMap<ExpectedClass, ClassStats>,
    /// Differences nothing explains. Must be empty.
    pub mismatches: Vec<Mismatch>,
    /// The Closures Computed View comparison.
    pub adjusted: AdjustedCheck,
}

impl Verification {
    /// Whether every difference is explained.
    pub fn is_clean(&self) -> bool {
        self.mismatches.is_empty()
    }
}

/// A class of expected difference between a migrated and a legacy position.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ExpectedClass {
    /// A legacy merge the old build kept torn (its two members shown apart)
    /// is one station now: the lines behind it move by up to the gap.
    TornMergeCollapsed,
    /// A snap (`Move` closure) whose alignment the new solver derives
    /// differently from the legacy flag (its gap sits on the 1 cm boundary),
    /// so the depth tie through it changed.
    SnapAlignmentChanged,
    /// The legacy position is not finite (a non-finite reading the 0.6.0
    /// load kept and its save would have zeroed); nothing to compare.
    LegacyNotFinite,
}

impl ExpectedClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TornMergeCollapsed => "torn-merge-collapsed",
            Self::SnapAlignmentChanged => "snap-alignment-changed",
            Self::LegacyNotFinite => "legacy-not-finite",
        }
    }
}

/// How many slots a class explains and by how much they moved.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ClassStats {
    pub slots: usize,
    pub max_displacement_m: f64,
}

/// One unexplained difference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mismatch {
    pub line_id: LineId,
    pub station_index: u32,
    pub axis: Axis,
    /// The difference, metres (infinite when the slot has no new position).
    pub displacement_m: f64,
    /// The tolerance it exceeded, metres.
    pub tolerance_m: f64,
}

/// Which coordinate a [`Mismatch`] is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Depth,
    /// The slot has no position in the migrated project.
    Missing,
}

/// The state of the Closures Computed View comparison
/// ([`super::verify::verify_adjusted`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub enum AdjustedCheck {
    /// Not compared: the station-graph least-squares solve (stream M3) is
    /// not on this branch yet.
    #[default]
    AwaitingSolver,
    /// Not compared: the legacy open did not compute adjusted positions
    /// (`OpenOptions::with_adjusted` off).
    NoLegacyPositions,
}

impl MigrationReport {
    /// The report as `name=count` lines (counts and displacements only, no
    /// survey content), for logs and the corpus runner.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        let mut put = |name: &str, value: usize| {
            if value > 0 {
                let _ = writeln!(out, "{name}={value}");
            }
        };
        put("lines", self.lines);
        put("shots", self.shots);
        put("slots", self.slots);
        put("stations", self.stations);
        put("merges_collapsed", self.merges_collapsed);
        put("merges_demoted_cycle", self.merges_demoted_cycle);
        put("merges_demoted_same_line", self.merges_demoted_same_line);
        put("merges_dropped_self", self.merges_dropped_self);
        put("merges_dropped_redundant", self.merges_dropped_redundant);
        put("merges_dropped_dangling", self.merges_dropped_dangling);
        put("aliases_created", self.aliases_created);
        put("station_ids_dropped", self.station_ids_dropped);
        put("station_ids_minted", self.station_ids_minted);
        put("stations_renamed", self.stations_renamed);
        put("comments_joined", self.comments_joined);
        put("station_keys_dangling", self.station_keys_dangling);
        put("station_keys_duplicate", self.station_keys_duplicate);
        put("shot_uuids_minted", self.shot_uuids_minted);
        put("numbers_sanitized", self.numbers_sanitized);
        put("closures_kept", self.closures_kept);
        put("aligned_closures_to_tie", self.aligned_closures_to_tie);
        put("closures_dropped_dangling", self.closures_dropped_dangling);
        put(
            "closures_dropped_one_station",
            self.closures_dropped_one_station,
        );
        put(
            "closures_dropped_unanchored",
            self.closures_dropped_unanchored,
        );
        put("closure_ids_reminted", self.closure_ids_reminted);
        put("datums_kept", self.datums_kept);
        put("datums_dropped_dangling", self.datums_dropped_dangling);
        put("corrections_stored", self.corrections_stored);
        put("placements_held_gauge", self.placements_held_gauge);
        put("placements_default_gauge", self.placements_default_gauge);
        put("placements_fallback", self.placements_fallback);
        put(
            "entrance_overrides_written",
            self.entrance_overrides_written,
        );
        put("hand_moves_carried", self.hand_moves_carried);
        put("ignores_kept", self.ignores_kept);
        put("ignores_rekeyed", self.ignores_rekeyed);
        put("ignores_dropped", self.ignores_dropped);
        put("gauge_pairs_dropped", self.gauge_pairs_dropped);
        put("held_gauges_dropped", self.held_gauges_dropped);
        put(
            "history_rows_dropped",
            usize::try_from(self.history_rows_dropped).unwrap_or(usize::MAX),
        );
        put("legacy_derived_shot_uuids", self.legacy_derived_shot_uuids);
        put("legacy_renamed_shot_uuids", self.legacy_renamed_shot_uuids);
        put("legacy_healed_offsets", self.legacy_healed_offsets);
        put("legacy_merges_created", self.legacy_merges_created);
        put("legacy_integrity_findings", self.legacy_integrity_findings);
        let v = &self.verification;
        let _ = writeln!(out, "verify.slots={}", v.slots_checked);
        let _ = writeln!(out, "verify.max_xy_m={:e}", v.max_xy_m);
        let _ = writeln!(out, "verify.max_z_m={:e}", v.max_z_m);
        for (class, stats) in &v.expected {
            let _ = writeln!(
                out,
                "verify.expected.{}={} max_m={:e}",
                class.as_str(),
                stats.slots,
                stats.max_displacement_m
            );
        }
        let _ = writeln!(out, "verify.mismatches={}", v.mismatches.len());
        if let Some(worst) = v
            .mismatches
            .iter()
            .map(|m| m.displacement_m)
            .max_by(f64::total_cmp)
        {
            let _ = writeln!(out, "verify.mismatch_max_m={worst:e}");
        }
        let _ = writeln!(out, "verify.adjusted={:?}", v.adjusted);
        out
    }
}
