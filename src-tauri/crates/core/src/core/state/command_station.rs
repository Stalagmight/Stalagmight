//! The station topology family of the execute pipeline (plan 9.1, 5.1-5.7):
//! merge, unmerge (detach), loop closures and zero datums, on the
//! station-graph model (`survey::ops_*`).
//!
//! The survey operations settle anchors, run the orphan-closure sweep
//! (merge, detach, datum removal) and validate; this layer adds what is the
//! command's, not the model's: 0.6.0's refusals that the model does not word
//! (a merge on one line, a datum removal on a station without one), the
//! entrance answer that rides Set Zero Datum, and the warnings that name the
//! loop closures a sweep removed because their ends became one station
//! (#738), each named by the stations it had BEFORE the command (E2).

use std::collections::BTreeMap;

use super::commands::{Applied, Record};
use crate::core::{Command, ValidationWarning, WarningSeverity};
use crate::models::survey::{ClosureId, LoopClosure, OpError, Project, SweepReport};
use crate::models::{StationRef, CLOSURE_REMOVED_MERGED_ENDS};

/// 0.6.0's refusal of a merge between two stations of one line.
const MERGE_SAME_LINE: &str = "Cannot merge stations from the same line";

/// 0.6.0's refusal of a datum removal on a station that holds none.
const NOT_A_ZERO_DATUM: &str = "Station is not a zero datum";

/// Why a closure the sweep after a user merge removed has one point for its
/// two ends (0.6.0's `OnePointCause::Merge`).
const ONE_POINT_BY_MERGE: &str = "the merge made its two stations one point.";

/// Why a closure the sweep after an unmerge or a datum removal removed has
/// one point for its two ends: another closure's promotion joined them
/// (0.6.0's `OnePointCause::Promotion`).
const ONE_POINT_BY_PROMOTION: &str =
    "another loop closure became a merge, which made its two stations one point.";

/// Every loop closure's two stations as the user read them before a command
/// (`Main Line / 4`, each end on its `via` line), so a warning for a closure
/// the command removed names the stations the user created it between: the
/// command may retire a station into an alias (a merge) and a name read
/// afterwards would be the survivor's. Empty on a project without closures.
#[derive(Debug, Default)]
struct ClosureNames(BTreeMap<ClosureId, (String, String)>);

impl ClosureNames {
    fn of(project: &Project) -> Self {
        Self(
            project
                .closures
                .iter()
                .map(|c| {
                    (
                        c.id,
                        (
                            project.station_text(c.a, c.a_via),
                            project.station_text(c.b, c.b_via),
                        ),
                    )
                })
                .collect(),
        )
    }

    /// `closure`'s two names as recorded, else as `after` reads them.
    fn pair(&self, after: &Project, closure: &LoopClosure) -> (String, String) {
        self.0.get(&closure.id).cloned().unwrap_or_else(|| {
            (
                after.station_text(closure.a, closure.a_via),
                after.station_text(closure.b, closure.b_via),
            )
        })
    }
}

/// The warnings for the closures a sweep removed because their two ends
/// became one station (#738), as 0.6.0 worded them, ending with `why`. The
/// frontend shows them in one toast (`notifyRemovedClosures`). Promotions are
/// logged, not reported (0.6.0).
fn swept_closure_warnings(
    names: &ClosureNames,
    after: &Project,
    sweep: &SweepReport,
    why: &str,
    context: &str,
) -> Vec<ValidationWarning> {
    if !sweep.promoted.is_empty() {
        tracing::info!(
            promoted = sweep.promoted.len(),
            context,
            purpose = "a loop closure left as the only connection is a merge",
            "Promoted loop closures to merges"
        );
    }
    if !sweep.removed.is_empty() {
        tracing::info!(
            removed = sweep.removed.len(),
            context,
            purpose = "a loop closure between merged stations closes no loop",
            "Removed loop closures whose two stations are one point"
        );
        tracing::debug!(
            closure_ids = ?sweep.removed.iter().map(|c| c.id).collect::<Vec<_>>(),
            context,
            "Loop closures removed for merged ends"
        );
    }
    sweep
        .removed
        .iter()
        .map(|c| {
            let (a, b) = names.pair(after, c);
            ValidationWarning {
                code: CLOSURE_REMOVED_MERGED_ENDS.to_string(),
                message: format!("Removed loop closure {a} to {b}: {why}"),
                severity: WarningSeverity::Warning,
            }
        })
        .collect()
}

fn text(error: OpError) -> String {
    error.to_string()
}

/// The execute pipeline's mutation for the station topology family
/// (`commands.rs` dispatches here; plan 9.1). Runs inside `run_recorded`,
/// which snapshots, validates, captures and records around it; `record` is
/// the entry being recorded (worded before the edit).
pub(super) fn apply(
    project: &mut Project,
    record: &mut Record,
    cmd: &Command,
) -> Result<Applied, String> {
    let names = ClosureNames::of(project);
    match cmd {
        Command::MergeStations {
            station_a,
            station_b,
        } => merge(project, &names, *station_a, *station_b),
        Command::UnmergeStations { station_ref } => {
            let outcome = project.unmerge_station(*station_ref).map_err(text)?;
            if let Some(detached) = &outcome.value {
                tracing::info!(
                    station = detached.station,
                    line = detached.line,
                    new_station = detached.new_station,
                    purpose = "disconnect merged stations",
                    "Line detached from its junction"
                );
                record.refs.stations.push(detached.new_station);
            }
            Ok(Applied {
                warnings: swept_closure_warnings(
                    &names,
                    project,
                    &outcome.sweep,
                    ONE_POINT_BY_PROMOTION,
                    "unmerge",
                ),
                ..Applied::default()
            })
        }
        Command::UnmergeStationPair {
            station_a,
            station_b,
        } => {
            // 0.6.0 answered one refusal for any pair it found no direct
            // merge for, a slot that names nothing included.
            if project.resolve_slot_pair(*station_a, *station_b).is_err() {
                return Err(OpError::NotDirectlyMerged.to_string());
            }
            let outcome = project
                .unmerge_station_pair(*station_a, *station_b)
                .map_err(text)?;
            tracing::info!(
                station = outcome.value.station,
                line = outcome.value.line,
                new_station = outcome.value.new_station,
                purpose = "disconnect selected merged station pair",
                "Line detached from its junction"
            );
            record.refs.stations.push(outcome.value.new_station);
            Ok(Applied {
                warnings: swept_closure_warnings(
                    &names,
                    project,
                    &outcome.sweep,
                    ONE_POINT_BY_PROMOTION,
                    "unmerge pair",
                ),
                ..Applied::default()
            })
        }
        Command::CreateLoopClosure {
            station_a,
            station_b,
            closure_strategy,
        } => {
            let outcome = project
                .create_closure_at(*station_a, *station_b, (*closure_strategy).into())
                .map_err(text)?;
            let closure = outcome.value.closure;
            tracing::debug!(
                closure_id = closure.id,
                ?station_a,
                ?station_b,
                purpose = "add loop closure to improve accuracy",
                "Loop closure created"
            );
            record.refs.closures.push(closure.id);
            Ok(Applied::default())
        }
        Command::RemoveLoopClosure { closure_id } => {
            let outcome = project.remove_closure(*closure_id).map_err(text)?;
            tracing::debug!(
                closure_id,
                station_a = outcome.value.a,
                station_b = outcome.value.b,
                purpose = "remove loop closure from project",
                "Loop closure removed"
            );
            Ok(Applied::default())
        }
        Command::SetZeroDatum {
            station_ref,
            coordinates,
            accuracy_m,
            entrance,
        } => {
            let outcome = project
                .set_datum_at(*station_ref, *coordinates, *accuracy_m)
                .map_err(text)?;
            // `None` keeps the stored answer (the default rule for a new
            // datum); the flag moves nothing (#741).
            if entrance.is_some() {
                project
                    .set_datum_entrance(outcome.value.index, *entrance)
                    .map_err(text)?;
            }
            tracing::info!(
                ?station_ref,
                change = ?outcome.value.change,
                role = ?outcome.value.role,
                purpose = "anchor survey to GPS coordinates",
                "Zero datum set"
            );
            Ok(Applied::default())
        }
        Command::RemoveZeroDatum { station_ref } => {
            let Some(outcome) = project.remove_datum_at(*station_ref).map_err(text)? else {
                return Err(NOT_A_ZERO_DATUM.to_string());
            };
            tracing::debug!(
                ?station_ref,
                remaining_zero_datums = project.datums.len(),
                purpose = "remove GPS anchor",
                "Zero datum removed"
            );
            Ok(Applied {
                warnings: swept_closure_warnings(
                    &names,
                    project,
                    &outcome.sweep,
                    ONE_POINT_BY_PROMOTION,
                    "remove zero datum",
                ),
                ..Applied::default()
            })
        }
        _ => Err(format!(
            "{} is not a station topology command",
            crate::core::command_kind(cmd)
        )),
    }
}

/// Merge Stations (plan 5.1): `a`'s piece moves onto `b` unless `a`'s is
/// placed and `b`'s is not (plan 4.2). Two stations of one line are refused
/// first, in 0.6.0's words.
fn merge(
    project: &mut Project,
    names: &ClosureNames,
    station_a: StationRef,
    station_b: StationRef,
) -> Result<Applied, String> {
    if station_a.line_id == station_b.line_id {
        return Err(MERGE_SAME_LINE.to_string());
    }
    let (a, b) = project
        .resolve_slot_pair(station_a, station_b)
        .map_err(text)?;
    let outcome = project.merge_stations(a.station, b.station).map_err(|e| {
        tracing::warn!(
            ?station_a,
            ?station_b,
            error = %e,
            purpose = "connect survey lines",
            "Failed to merge stations"
        );
        e.to_string()
    })?;
    tracing::debug!(
        ?station_a,
        ?station_b,
        survivor = outcome.value.survivor,
        retired = outcome.value.retired,
        purpose = "connect survey lines",
        "Stations merged"
    );
    Ok(Applied {
        warnings: swept_closure_warnings(names, project, &outcome.sweep, ONE_POINT_BY_MERGE, "merge"),
        ..Applied::default()
    })
}
