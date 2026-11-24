//! Project warnings and their ignores on the station-graph model.
//!
//! The warnings of a project, every one hung on a shot (the list, Ignore and
//! "Go to" are all shot-keyed):
//!
//! - **Per shot**: [`Validator::validate_project_shot`], against the
//!   project's own thresholds (`settings.validation`, #57).
//! - **GPS gaps** (`DATUM_GPS_GAP` / `DATUM_GPS_GAP_START`, #741): one per GPS
//!   fix further from where the survey puts it than its threshold, on the
//!   fix's slot ([`Project::gps_gap_warnings_from`]: station `i` -> shot `i - 1`,
//!   station 0 -> shot 0 under the `_START` code). A fix on a line with no
//!   shot gets no row (logged).
//! - **Closures across one shot** (`CLOSURE_ACROSS_ONE_SHOT`, #737): one error
//!   per closure the Closures Computed View skips, on the shot it collapses,
//!   carrying the closure id.
//! - **Computed View failures** (`CLOSURE_ADJUSTMENT_FAILED`, #737): one per
//!   component the memoised solve of THIS revision could not adjust. Read from
//!   the cache only, never solved here (0.6.0's rule): it appears once the
//!   Computed View, Plan or Export has solved the current revision, and an
//!   edit drops it until the next solve, which is why it is advisory.
//! - **Unconnected pieces** (`LINE_NOT_CONNECTED`, #814): one advisory per
//!   piece that holds no datum, in a project where some piece does, on the
//!   first shot of its first line.
//! - **Junction depth readings that disagree** (`JUNCTION_DEPTH_SPREAD` /
//!   `JUNCTION_DEPTH_SPREAD_START`, demo issue I-4): one advisory per station
//!   on two or more lines whose depth readings span more than
//!   [`JUNCTION_DEPTH_SPREAD_M`], on the shot arriving at it (else the one
//!   leaving it, under the `_START` code), naming the station so "Go to
//!   station" finds it.
//!
//! A warning that names a shot, a station or a line carries the message's
//! words as prose parts too (RS-PROSE, [`ProjectWarning::message_parts`]):
//! each producer words its message with marked names (`prose::mark`) and the
//! row splits it (`prose::split`), so the plain `message` is the string the
//! producer always wrote and the marked string never leaves this module.
//!
//! Ignored warnings are survey content now (plan 6.5): the file's
//! `ignored_warnings` member, held in memory (`FileState`), keyed by the
//! shot's persistent uuid and the code. A shot keeps its uuid through every
//! edit that moves it (insert, delete, reverse, split, combine), so an ignore
//! follows its shot without the position-keyed rows and their re-pinning
//! 0.6.0 needed. An ignore marks the project dirty and rides the next save
//! (declared change: "Don't save" discards it; 0.6.0 wrote it into the file
//! at once). Errors are never ignorable ([`warning_is_ignorable`]), whatever
//! the file holds. A datum gap's ignore is stored under a code that names the
//! datum's GPS ([`datum_gap_ignore_code`], #741 U2): a new reading brings the
//! warning back, and undoing it brings the ignore back.
//!
//! The IPC keeps its `ResolvedWarning` shape, converted here through the
//! open project's shots.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::open_file::FileState;
use super::AppState;
use crate::core::{Validator, WarningSeverity};
use crate::models::prose;
use crate::models::survey::{AdjustmentFailure, Project, RawPositions, SlotTexts};
use crate::models::{
    datum_gap_ignore_code, severity_for_code, warning_is_ignorable, DatumGap, DatumRole, LineId,
    ProjectWarning, ShotId, StationRef, CLOSURE_ACROSS_ONE_SHOT, CLOSURE_ADJUSTMENT_FAILED,
    DATUM_GPS_GAP, DATUM_GPS_GAP_START, JUNCTION_DEPTH_SPREAD, JUNCTION_DEPTH_SPREAD_M,
    JUNCTION_DEPTH_SPREAD_START, LINE_NOT_CONNECTED,
};
use crate::persistence::stalx_v2::IgnoredWarning;
use crate::persistence::ResolvedWarning;

/// The ignores of one file, by `(shot uuid, stored code)`.
struct Ignored<'a>(HashSet<(&'a str, &'a str)>);

impl<'a> Ignored<'a> {
    fn of(ignores: &'a [IgnoredWarning]) -> Self {
        Self(
            ignores
                .iter()
                .map(|i| (i.shot.as_str(), i.code.as_str()))
                .collect(),
        )
    }

    /// Whether a warning of `code` on the shot `uuid` is ignored, its ignore
    /// being stored under `stored` (the code itself, or a datum gap's pinned
    /// code). Never for an error.
    fn covers(&self, uuid: &str, code: &str, stored: &str) -> bool {
        warning_is_ignorable(code) && self.0.contains(&(uuid, stored))
    }
}

/// Every warning of `project`, sorted (unresolved first, then errors first,
/// then by line and shot): its per-shot checks, its GPS gaps, its closures
/// across one shot and the Computed View `failures` given (the memoised
/// solve's, or none), with `ignores` applied. `raw` is its raw layout.
fn project_warnings(
    project: &Project,
    raw: &RawPositions,
    ignores: &[IgnoredWarning],
    failures: &[AdjustmentFailure],
) -> Vec<ProjectWarning> {
    let ignored = Ignored::of(ignores);
    let mut all = shot_warnings(project, &ignored);
    all.extend(gps_gap_warnings(project, raw, &ignored));
    all.extend(closure_across_one_shot_warnings(project, raw));
    all.extend(adjustment_failure_warnings(project, failures, &ignored));
    all.extend(unconnected_piece_warnings(project, &ignored));
    all.extend(junction_depth_spread_warnings(project, raw, &ignored));
    // Stable: the rows of one shot keep the order of the kinds above.
    all.sort_by(|a, b| {
        let severity_order = |s: WarningSeverity| match s {
            WarningSeverity::Error => 0,
            WarningSeverity::Warning => 1,
        };
        a.is_resolved
            .cmp(&b.is_resolved)
            .then_with(|| severity_order(a.severity).cmp(&severity_order(b.severity)))
            .then_with(|| a.line_id.cmp(&b.line_id))
            .then_with(|| a.shot_index.cmp(&b.shot_index))
    });
    all
}

/// The warnings of a project being opened (the load result's list): checked
/// against its own file's ignores, from its raw layout `raw`. No Computed View
/// has been solved for it yet, so it has no failure warning.
pub(super) fn opened_project_warnings(
    project: &Project,
    raw: &RawPositions,
    file: &FileState,
) -> Vec<ProjectWarning> {
    project_warnings(project, raw, file.ignored_warnings(), &[])
}

/// A warning row on shot `shot_index` of `line_id`, about nothing else.
/// `marked` is its message with any name it says marked (`prose::mark`):
/// split here into the plain `message` and its `message_parts`.
fn row(
    project: &Project,
    line_id: LineId,
    shot_index: usize,
    code: &str,
    marked: &str,
) -> Option<ProjectWarning> {
    let shot = project.line(line_id)?.shots.get(shot_index)?;
    let (message, parts) = prose::split(marked);
    Some(ProjectWarning {
        line_id,
        shot_id: shot.id,
        shot_index,
        code: code.to_string(),
        message,
        severity: severity_for_code(code),
        is_resolved: false,
        shot_uuid: Some(shot.uuid.clone()),
        datum_station_index: None,
        datum_refs: None,
        closure_id: None,
        message_parts: prose::named(parts),
    })
}

/// A marked name (one part) bounded as file text (`sanitize_display_text`
/// to `MAX_FILE_NAME_LEN`, as every receipt string): the part as it is when
/// its words are already within bounds, else the bounded words alone, so
/// the plain message is the bounded text either way.
fn bounded_name(marked: &str) -> String {
    let plain = prose::unmark(marked);
    let bounded =
        crate::models::sanitize_display_text(&plain, crate::core::command::MAX_FILE_NAME_LEN);
    if bounded == plain {
        marked.to_owned()
    } else {
        bounded
    }
}

/// Every shot's own checks, in line list order. The shots are named through
/// one [`SlotTexts`], so a hub named on each of its lines costs its lines,
/// not its lines times its records (RS-FX9); each as its part, which the
/// validator splits out of the message (`message_parts`).
fn shot_warnings(project: &Project, ignored: &Ignored) -> Vec<ProjectWarning> {
    let mut out = Vec::new();
    let mut texts: Option<SlotTexts<'_>> = None;
    for line in project.lines() {
        for (shot_index, shot) in line.shots.iter().enumerate() {
            let named = || {
                texts
                    .get_or_insert_with(|| SlotTexts::new(project))
                    .shot_marked(line.id, shot_index)
            };
            for warning in Validator::validate_project_shot_named(project, line, shot_index, named)
            {
                let is_resolved = ignored.covers(&shot.uuid, &warning.code, &warning.code);
                out.push(ProjectWarning {
                    line_id: line.id,
                    shot_id: shot.id,
                    shot_index,
                    code: warning.code,
                    message: warning.message,
                    severity: warning.severity,
                    is_resolved,
                    shot_uuid: Some(shot.uuid.clone()),
                    datum_station_index: None,
                    datum_refs: None,
                    closure_id: None,
                    message_parts: warning.parts,
                });
            }
        }
    }
    out
}

/// One advisory warning per GPS fix whose GPS lies further from its
/// surveyed position than 15 m plus 2% of the survey distance from its
/// piece's placement datum (#741 decision 4), sorted by slot.
/// `datum_station_index` names the fix's slot on the row's line for "Go to
/// station".
fn gps_gap_warnings(
    project: &Project,
    raw: &RawPositions,
    ignored: &Ignored,
) -> Vec<ProjectWarning> {
    // A GPS fix needs a placement datum before it, and the gaps walk the
    // survey from every placement datum: a project with one datum or none
    // has nothing to walk for.
    if project.datums.len() < 2 {
        return Vec::new();
    }
    project
        .gps_gap_warnings_from(raw)
        .into_iter()
        .filter_map(|gap| {
            let Some(shot_index) = gap.shot_index else {
                tracing::warn!(
                    line_id = gap.slot.line_id,
                    station_index = gap.slot.station_index,
                    gap_m = gap.gap.gap_m,
                    purpose = "report a GPS fix far from its survey",
                    "GPS fix exceeds the gap threshold but its line has no shot to carry the warning"
                );
                return None;
            };
            let datum = project.datums.get(gap.datum)?;
            let mut warning = row(
                project,
                gap.slot.line_id,
                shot_index,
                gap.code,
                &datum_gps_gap_message(&gap.gap),
            )?;
            // The ignore names this datum's GPS too (#741 U2).
            let stored = datum_gap_ignore_code(gap.code, &datum.gps);
            warning.is_resolved = warning
                .shot_uuid
                .as_deref()
                .is_some_and(|uuid| ignored.covers(uuid, gap.code, &stored));
            warning.datum_station_index = Some(gap.slot.station_index);
            Some(warning)
        })
        .collect()
}

/// One error per loop closure that ties the two ends of one shot together
/// (#737), on the shot it collapses: the Closures Computed View skips such a
/// closure, and without this the user would only see the loop stay open.
/// Carries the closure id so the Resolve view can remove exactly it. Never
/// ignorable. The ends are named on their `via` lines, the shot and the ends
/// as parts.
fn closure_across_one_shot_warnings(project: &Project, raw: &RawPositions) -> Vec<ProjectWarning> {
    if project.closures.is_empty() {
        return Vec::new();
    }
    let aligned = project.closure_alignment(raw);
    let mut texts: Option<SlotTexts<'_>> = None;
    project
        .closures_across_one_shot_with(&aligned)
        .into_iter()
        .filter_map(|skipped| {
            let (line_id, shot_index) = skipped.collapsed_shot;
            let closure = project.closures.get(skipped.index)?;
            let texts = texts.get_or_insert_with(|| SlotTexts::new(project));
            let message = format!(
                "Shot {} has both ends joined by loop closure {} to {}, which would squeeze the shot to zero length, so the Closures Computed View skips that closure. Remove the closure, or close the loop at two stations that are not the ends of a single shot.",
                texts.shot_marked(line_id, shot_index),
                texts.station_marked(closure.a, closure.a_via),
                texts.station_marked(closure.b, closure.b_via),
            );
            let mut warning = row(project, line_id, shot_index, CLOSURE_ACROSS_ONE_SHOT, &message)?;
            warning.closure_id = Some(skipped.closure_id);
            Some(warning)
        })
        .collect()
}

/// One advisory warning per component in `failures` (#737), on the shot at
/// the solve's anchor station (station 0 -> shot 0, station `i` -> shot
/// `i - 1`), else the first shot of the component's first line that has one.
/// The primary surface is the notice drawn with the adjusted view itself;
/// this row is where the user reads which part and why. The shot is a part
/// ([`Project::shot_marked`]: the words of [`Validator::shot_label`]).
fn adjustment_failure_warnings(
    project: &Project,
    failures: &[AdjustmentFailure],
    ignored: &Ignored,
) -> Vec<ProjectWarning> {
    failures
        .iter()
        .filter_map(|failure| {
            let anchor = failure.anchor_slot;
            let at_anchor = project.line(anchor.line_id).and_then(|line| {
                let index = (anchor.station_index as usize).saturating_sub(1);
                line.shots.get(index).map(|_| (line.id, index))
            });
            let (line_id, shot_index) = at_anchor.or_else(|| {
                failure.lines.iter().find_map(|&id| {
                    let line = project.line(id)?;
                    line.shots.first().map(|_| (line.id, 0))
                })
            })?;
            let lines = failure.lines.len();
            let message = format!(
                "Shot {} is in a part of the survey ({lines} {}) the Closures Computed View could not adjust, so that part is shown as measured: {}",
                project.shot_marked(line_id, shot_index),
                if lines == 1 { "line" } else { "lines" },
                failure.error.user_reason(),
            );
            let mut warning =
                row(project, line_id, shot_index, CLOSURE_ADJUSTMENT_FAILED, &message)?;
            warning.is_resolved = warning.shot_uuid.as_deref().is_some_and(|uuid| {
                ignored.covers(uuid, CLOSURE_ADJUSTMENT_FAILED, CLOSURE_ADJUSTMENT_FAILED)
            });
            Some(warning)
        })
        .collect()
}

/// One advisory warning per piece of the survey that nothing places (#814,
/// [`LINE_NOT_CONNECTED`]): a piece (stations joined by shots; a junction is
/// one station) holding no datum, in a project where some piece holds one.
/// An import whose connection could not be made leaves such a piece, and
/// while the project holds a datum this is where the user finds it again
/// once the receipt is closed (in a project with none, every piece floats
/// and the receipt is the only record). On the first shot of the piece's
/// first line (list order) that has one, so an Ignore keys on that shot; a
/// piece of shot-less lines gets no row. The line is named in quotes, as its
/// part (`"Main"`, the pill without the quotes).
///
/// 0.6.0's pieces were its rigid groups, which also followed aligned loop
/// closures; on the station graph a closure joins two pieces only when both
/// hold a datum (invariant 7), so it never changes which piece floats.
fn unconnected_piece_warnings(project: &Project, ignored: &Ignored) -> Vec<ProjectWarning> {
    if project.datums.is_empty() {
        return Vec::new();
    }
    let topology = project.topology();
    let placed = project.placement_datums();
    let order: HashMap<LineId, usize> = project
        .lines()
        .iter()
        .enumerate()
        .map(|(i, l)| (l.id, i))
        .collect();
    let mut floating: Vec<&[LineId]> = topology
        .components()
        .iter()
        .enumerate()
        .filter(|&(c, component)| {
            placed.get(c).copied().flatten().is_none() && !component.lines.is_empty()
        })
        .map(|(_, component)| component.lines.as_slice())
        .collect();
    // Components come by lowest station; the list reads in line order.
    floating.sort_by_key(|lines| lines.first().and_then(|id| order.get(id).copied()));
    floating
        .into_iter()
        .filter_map(|lines| {
            let line = lines
                .iter()
                .filter_map(|&id| project.line(id))
                .find(|l| !l.shots.is_empty())?;
            // File-controlled text, so the same bound and filter as every
            // receipt string: no bidi overrides, no unbounded length. Named as
            // the user knows the line, never by its id.
            let name = crate::models::sanitize_display_text(
                &project.line_label_of(line.id),
                crate::core::command::MAX_FILE_NAME_LEN,
            );
            let others = lines.len() - 1;
            // `"Main"`: the quoted words, as a line part (RS-PROSE).
            let line_name = prose::mark(&prose::line_part(Some(line.id), &name, true));
            let piece = match others {
                0 => format!("Line {line_name} is"),
                1 => format!("Line {line_name} and 1 line merged with it are"),
                n => format!("Line {line_name} and {n} lines merged with it are"),
            };
            let message = format!(
                "{piece} not connected to the rest of the survey and {} no zero datum, so \
                 nothing places {}: merge a station onto the survey or give it a zero datum",
                if others == 0 { "has" } else { "have" },
                if others == 0 { "it" } else { "them" },
            );
            let mut warning = row(project, line.id, 0, LINE_NOT_CONNECTED, &message)?;
            warning.is_resolved = warning
                .shot_uuid
                .as_deref()
                .is_some_and(|uuid| ignored.covers(uuid, LINE_NOT_CONNECTED, LINE_NOT_CONNECTED));
            Some(warning)
        })
        .collect()
}

/// One advisory warning per junction (a station on two or more lines) whose
/// depth readings span more than [`JUNCTION_DEPTH_SPREAD_M`] (demo issue
/// I-4): the station's depth is the mean of its readings, so a reading off by
/// metres, or two lines surveyed with gauges that disagree, moves it without
/// a word. On the shot arriving at the junction on its first line (slot
/// order) that has one, under [`JUNCTION_DEPTH_SPREAD`], else on the shot
/// leaving it, under [`JUNCTION_DEPTH_SPREAD_START`]: a shot arrives at one
/// station and only shot 0 leaves a line's start, so no two rows share an
/// Ignore key. `datum_station_index` names the junction's slot on the row's
/// line for "Go to station". A non-finite reading has its own error
/// (`NON_FINITE_VALUE`) and raises none here. The station is a part.
fn junction_depth_spread_warnings(
    project: &Project,
    raw: &RawPositions,
    ignored: &Ignored,
) -> Vec<ProjectWarning> {
    let topology = project.topology();
    let mut out = Vec::new();
    let mut texts: Option<SlotTexts<'_>> = None;
    for &station in topology.stations() {
        let slots = topology.slots_of(station);
        if slots.len() < 2 {
            continue;
        }
        let mut slots = slots.to_vec();
        slots.sort_unstable();
        let readings = project.depth_readings_at(&slots);
        let (low, high) = readings
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), &d| {
                (lo.min(d), hi.max(d))
            });
        let spread = high - low;
        if !spread.is_finite() || spread <= JUNCTION_DEPTH_SPREAD_M {
            continue;
        }
        let has_shot = |slot: &StationRef, index: usize| {
            project
                .line(slot.line_id)
                .is_some_and(|line| line.shots.get(index).is_some())
        };
        let arriving = slots.iter().find_map(|slot| {
            let index = (slot.station_index as usize).checked_sub(1)?;
            has_shot(slot, index).then_some((*slot, index, JUNCTION_DEPTH_SPREAD))
        });
        let Some((slot, shot_index, code)) = arriving.or_else(|| {
            slots
                .iter()
                .find(|slot| slot.station_index == 0 && has_shot(slot, 0))
                .map(|slot| (*slot, 0, JUNCTION_DEPTH_SPREAD_START))
        }) else {
            continue;
        };
        let name = bounded_name(
            &texts
                .get_or_insert_with(|| SlotTexts::new(project))
                .station_marked(station, Some(slot.line_id)),
        );
        let mean = raw.z(station).unwrap_or(f64::NAN);
        let message = format!(
            "The depth readings at {name}, where {} lines meet, differ by {spread:.1} m \
             ({low:.1} m to {high:.1} m). The station's depth is their mean, {mean:.1} m. \
             Check the depths of the shots that meet here.",
            slots.len(),
        );
        let Some(mut warning) = row(project, slot.line_id, shot_index, code, &message) else {
            continue;
        };
        warning.is_resolved = warning
            .shot_uuid
            .as_deref()
            .is_some_and(|uuid| ignored.covers(uuid, code, code));
        warning.datum_station_index = Some(slot.station_index);
        out.push(warning);
    }
    out
}

/// The `datum_gps_gap` warning text: the gap in metres, which way the GPS lies
/// from the surveyed station, and the threshold it exceeds.
fn datum_gps_gap_message(gap: &DatumGap) -> String {
    format!(
        "The GPS position of this zero datum is {:.1} m from where the survey puts it \
         (bearing {:.0} degrees), more than the {:.1} m expected. Check the GPS reading \
         and the survey between the datums.",
        gap.gap_m, gap.bearing_deg, gap.threshold_m
    )
}

/// The datum a gap warning shown on `slot` is about: the first GPS fix on
/// the slot's station whose own slot is `slot` (the one the warnings pass
/// hangs the row on), else the station's datum for that line
/// ([`Project::datum_index_for`]). Two fixes shown on one slot share their
/// row's (shot, code), so the first one names the ignore.
fn gap_datum(project: &Project, slot: StationRef) -> Option<usize> {
    let station = project.station_at(slot)?;
    let roles = project.datum_roles();
    project
        .datums
        .iter()
        .enumerate()
        .find(|&(index, datum)| {
            datum.station == station
                && roles.get(index).copied().flatten() == Some(DatumRole::GpsFix)
                && project.datum_slot(datum) == Some(slot)
        })
        .map(|(index, _)| index)
        .or_else(|| project.datum_index_for(station, Some(slot.line_id)))
}

/// The code an ignore of `code` on shot `shot_id` of `line_id` is stored and
/// matched under: for a datum gap warning, [`datum_gap_ignore_code`] with the
/// GPS of the datum the row is about (on station 0 for
/// [`DATUM_GPS_GAP_START`], on the station the shot arrives at for
/// [`DATUM_GPS_GAP`]); every other code, or a station that holds no datum, as
/// it is.
fn ignore_code_in(project: &Project, line_id: LineId, shot_id: ShotId, code: &str) -> String {
    let datum_gps = || {
        let line = project.line(line_id)?;
        let station_index = match code {
            DATUM_GPS_GAP_START => 0,
            DATUM_GPS_GAP => line.shots.iter().position(|s| s.id == shot_id)? + 1,
            _ => return None,
        };
        let slot = StationRef::new(line_id, u32::try_from(station_index).ok()?);
        let datum = gap_datum(project, slot)?;
        project.datums.get(datum).map(|d| d.gps)
    };
    match datum_gps() {
        Some(gps) => datum_gap_ignore_code(code, &gps),
        None => code.to_string(),
    }
}

/// `(line, session shot id)` of the shot with `uuid` in `project`.
fn locate(project: &Project, uuid: &str) -> Option<(LineId, ShotId)> {
    let at = project.shot_by_uuid(uuid)?;
    let line = project.line(at.line)?;
    let shot = line.shots.get(at.index)?;
    Some((line.id, shot.id))
}

impl AppState {
    /// The open project's warnings (the `get_project_warnings` IPC): its
    /// file's ignores, its memoised raw layout and the failures of the
    /// memoised Computed View of this revision. Empty when no project is open.
    /// Takes the project lock; never call it holding one.
    pub fn open_project_warnings(&self) -> Vec<ProjectWarning> {
        // Before the project lock: the config lock is never taken inside it.
        let config_key = self.adjusted_config_key();
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some(project) => self.slot_warnings(project, config_key),
            None => Vec::new(),
        }
    }

    /// The warnings of `project`, checked against the open file's ignores.
    /// When `project` is the open project as it is now (a copy from
    /// `get_project_state`), this is [`Self::open_project_warnings`],
    /// Computed View failures included; any other project is laid out on
    /// its own and gets none. A project being opened is checked against its
    /// own file instead ([`opened_project_warnings`]). Takes the project
    /// lock; never call it holding one.
    pub fn compute_project_warnings(&self, project: &Project) -> Vec<ProjectWarning> {
        let config_key = self.adjusted_config_key();
        {
            let guard = self
                .project
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(open) = guard.as_ref().filter(|open| *open == project) {
                return self.slot_warnings(open, config_key);
            }
        }
        let ignores = self.with_file(|file| file.ignored_warnings().to_vec());
        project_warnings(project, &project.raw_positions(), &ignores, &[])
    }

    /// The warnings of `project`, the project in the slot (the caller holds
    /// its lock), under the closure-adjustment config keyed `config_key`.
    fn slot_warnings(&self, project: &Project, config_key: u64) -> Vec<ProjectWarning> {
        let raw = self.raw_positions_of(project);
        let ignores = self.with_file(|file| file.ignored_warnings().to_vec());
        let failures = self.memoised_adjustment_failures(config_key);
        project_warnings(project, &raw, &ignores, &failures)
    }

    /// The cache key of the closure-adjustment config in force now. Takes
    /// the config lock, so a caller that also needs the project lock reads
    /// this first.
    pub(super) fn adjusted_config_key(&self) -> u64 {
        super::app_state::adjusted_config_hash(&self.get_config().closure_adjustment)
    }

    /// The components the memoised Computed View of the current revision
    /// could not adjust, when it was solved under the config keyed
    /// `config_key`; empty otherwise. Never solves.
    pub(super) fn memoised_adjustment_failures(&self, config_key: u64) -> Vec<AdjustmentFailure> {
        let revision = self.current_revision();
        let cache = self
            .derived
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        cache
            .as_ref()
            .filter(|entry| entry.revision == revision)
            .and_then(|entry| entry.adjusted.as_ref())
            .filter(|(key, _)| *key == config_key)
            .map(|(_, adjusted)| adjusted.failures().to_vec())
            .unwrap_or_default()
    }

    /// Every ignore of the open project, in the IPC's shape (an ignore whose
    /// shot is gone reads line and shot 0). A datum gap's code is the stored,
    /// GPS-pinned one.
    pub fn load_resolved_warnings(&self) -> Result<Vec<ResolvedWarning>> {
        let ignores = self.with_file(|file| file.ignored_warnings().to_vec());
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let project = guard
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("No project open"))?;
        Ok(ignores
            .into_iter()
            .map(|i| {
                let (line_id, shot_id) = locate(project, &i.shot).unwrap_or((0, 0));
                ResolvedWarning {
                    line_id,
                    shot_id,
                    warning_code: i.code,
                    resolved_at: i.at,
                    resolved_comment: i.comment,
                    shot_uuid: Some(i.shot),
                }
            })
            .collect())
    }

    /// Record an ignore as given (replacing one for the same shot and code),
    /// ungated and unpinned: the user-facing path is
    /// [`Self::save_resolved_warning_gated`]. Marks the project dirty; the
    /// next save writes it.
    pub fn save_resolved_warning(&self, warning: &ResolvedWarning) -> Result<()> {
        let uuid = match warning.shot_uuid.clone() {
            Some(uuid) => uuid,
            None => self
                .shot_uuid_for(warning.line_id, warning.shot_id)
                .ok_or_else(|| anyhow::anyhow!("the shot no longer exists"))?,
        };
        self.with_file(|file| {
            let list = &mut file.extras.ignored_warnings;
            list.retain(|i| !(i.shot == uuid && i.code == warning.warning_code));
            list.push(IgnoredWarning {
                shot: uuid,
                code: warning.warning_code.clone(),
                at: warning.resolved_at,
                comment: warning.resolved_comment.clone(),
            });
            list.sort();
            file.dirty = true;
        });
        Ok(())
    }

    /// Every shot uuid is durable with the survey now (ignores ride the same
    /// save): true when the shot exists in the open project.
    pub fn shot_uuid_is_persisted(&self, _line_id: u32, uuid: &str) -> bool {
        self.project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .is_some_and(|p| p.shot_by_uuid(uuid).is_some())
    }

    /// Drop an ignore as given (ungated, unpinned). Marks the project dirty
    /// when one was dropped.
    pub fn delete_resolved_warning(
        &self,
        line_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<()> {
        let uuid = match shot_uuid {
            Some(uuid) => Some(uuid.to_string()),
            None => self.shot_uuid_for(line_id, shot_id),
        };
        let Some(uuid) = uuid else {
            return Ok(());
        };
        self.with_file(|file| {
            let list = &mut file.extras.ignored_warnings;
            let before = list.len();
            list.retain(|i| !(i.shot == uuid && i.code == warning_code));
            if list.len() != before {
                file.dirty = true;
            }
        });
        Ok(())
    }

    /// The user-facing **ignore** (the `resolve_warning` IPC's persistence
    /// step): refused on a SpeleoDB project open read-only (#197: an ignore
    /// is pushed, so it is a remote-dirtying edit) and on a file this build
    /// opened read-only (its save is refused, plan 6.4); stored under
    /// [`Self::ignore_code_for`] its warning, so a datum gap's ignore names
    /// the datum's GPS (#741 U2).
    pub fn save_resolved_warning_gated(&self, warning: &ResolvedWarning) -> Result<(), String> {
        self.guard_ignore("Resolve warning")?;
        let warning = ResolvedWarning {
            warning_code: self.ignore_code_for(
                warning.line_id,
                warning.shot_id,
                &warning.warning_code,
            ),
            ..warning.clone()
        };
        self.save_resolved_warning(&warning)
            .map_err(|e| e.to_string())?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// The user-facing **un-ignore**, gated and pinned like
    /// [`Self::save_resolved_warning_gated`].
    pub fn delete_resolved_warning_gated(
        &self,
        line_id: u32,
        shot_id: u32,
        warning_code: &str,
        shot_uuid: Option<&str>,
    ) -> Result<(), String> {
        self.guard_ignore("Unresolve warning")?;
        let warning_code = self.ignore_code_for(line_id, shot_id, warning_code);
        self.delete_resolved_warning(line_id, shot_id, &warning_code, shot_uuid)
            .map_err(|e| e.to_string())?;
        self.mark_remote_forward_edit();
        Ok(())
    }

    /// The gates of an ignore change: the SpeleoDB read-only gate
    /// ([`Self::guard_mutation`]), then the read-only file (an unknown
    /// `requires`), whose save would be refused.
    fn guard_ignore(&self, what: &str) -> Result<(), String> {
        self.guard_mutation(what)
            .map_err(|f| f.error.unwrap_or_default())?;
        if self.with_file(|file| file.extras.read_only.is_some()) {
            return Err(format!(
                "{}: {what} is disabled: this project was made with a newer Stalagmight and is \
                 open read-only",
                super::open_file::READ_ONLY_FILE_CODE
            ));
        }
        Ok(())
    }

    /// The code an ignore of `warning_code` on `(line_id, shot_id)` is stored
    /// and matched under in the open project ([`ignore_code_in`]). Takes the
    /// project lock; never call it holding one.
    pub fn ignore_code_for(&self, line_id: LineId, shot_id: ShotId, warning_code: &str) -> String {
        let guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match guard.as_ref() {
            Some(project) => ignore_code_in(project, line_id, shot_id, warning_code),
            None => warning_code.to_string(),
        }
    }

    /// The persistent uuid of a shot of the open project.
    pub fn shot_uuid_for(&self, line_id: LineId, shot_id: ShotId) -> Option<String> {
        let guard = self.project.lock().ok()?;
        let project = guard.as_ref()?;
        let line = project.line(line_id)?;
        line.shots
            .iter()
            .find(|s| s.id == shot_id)
            .map(|s| s.uuid.clone())
    }
}
