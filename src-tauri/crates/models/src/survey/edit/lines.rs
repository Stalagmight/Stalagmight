//! Line operations: create (with its shots; optionally starting at an
//! existing station, the junction placement imports make), delete, metadata,
//! reorder.
//!
//! - **Create**: today's refusals (too many shots, the line-id budget, a
//!   non-finite value, a zero or negative length). New stations are named by
//!   the seed rule (their chain index, letter-suffixed when taken on the
//!   line). A line that starts a new piece gets its `Placement` at the given
//!   point (today's new line sits with station 0 at the origin); one that
//!   starts at an existing station hangs off it and moves nothing.
//! - **Delete** (plan 5.4): every station of the line that another line also
//!   passes stays for it (with its comment, datums and closures); every other
//!   station is deleted with its comment, datums and closures. Then the
//!   orphan-closure sweep runs.
//! - **Metadata**: today's `LineUpdates` (name, legacy surveyor and date on
//!   every shot, colour, direction, quality), with today's rename refusals.
//!   A changed azimuth reference is a geometry edit.
//! - **Reorder**: the display order; positions do not depend on it (D2).

use std::collections::BTreeSet;

use crate::project::station_naming::name_characters_refusal;
use crate::{
    legacy_halves, legacy_people, AzimuthReference, LineId, LineUpdates, ShotData, StationId,
    SurveyCredit, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};

use super::super::{EditContext, Line, LineMeta, LoopClosure, Project, StartAt, ZeroDatum};
use super::shots::{max_shots_per_line, stored_credit};
use super::{EditError, Edited, Epilogue};

/// What deleting a line took away.
#[derive(Debug, Clone, PartialEq)]
pub struct DeletedLine {
    /// The line, as it was.
    pub line: Line,
    /// Its list position.
    pub position: usize,
    /// Its stations that left the project (on no other line).
    pub stations_deleted: Vec<StationId>,
    /// The datums, closures and comments deleted with them.
    pub datums_removed: Vec<ZeroDatum>,
    pub closures_removed: Vec<LoopClosure>,
    pub comments_removed: Vec<(StationId, String)>,
}

/// What [`Project::delete_stations_off_lines`] deleted.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Gone {
    pub(super) stations: Vec<StationId>,
    pub(super) datums: Vec<ZeroDatum>,
    pub(super) closures: Vec<LoopClosure>,
    pub(super) comments: Vec<(StationId, String)>,
}

impl Project {
    /// Delete every station of `candidates` that is on no line any more, with
    /// its datums, closures and comment (plan 5.4). A station still on a line
    /// is left alone.
    pub(super) fn delete_stations_off_lines(&mut self, candidates: &[StationId]) -> Gone {
        let topology = self.topology_arc();
        let doomed: BTreeSet<StationId> = candidates
            .iter()
            .copied()
            .filter(|s| !topology.contains(*s) && self.stations.contains_key(s))
            .collect();
        let mut gone = Gone::default();
        if doomed.is_empty() {
            return gone;
        }
        for &station in &doomed {
            if let Some(record) = self.stations.remove(&station) {
                if let Some(comment) = record.comment.filter(|c| !c.is_empty()) {
                    gone.comments.push((station, comment));
                }
            }
            gone.stations.push(station);
        }
        self.datums.retain(|d| {
            if doomed.contains(&d.station) {
                gone.datums.push(*d);
                false
            } else {
                true
            }
        });
        self.closures.retain(|c| {
            if doomed.contains(&c.a) || doomed.contains(&c.b) {
                gone.closures.push(*c);
                false
            } else {
                true
            }
        });
        self.placements.retain(|p| !doomed.contains(&p.station));
        gone
    }

    /// Create a line (module docs). `placement` is where a line that starts a
    /// new piece puts its start station (the origin when `None`); it is
    /// ignored for a line that starts at an existing station.
    pub fn create_line(
        &mut self,
        meta: LineMeta,
        start: StartAt,
        shots: Vec<ShotData>,
        placement: Option<[f64; 2]>,
    ) -> Result<Edited<LineId>, EditError> {
        let max = max_shots_per_line();
        if shots.len() > max {
            return Err(EditError::refused(format!(
                "Line exceeds maximum shots per line (limit = {max})"
            )));
        }
        if self.next_line_id() > MAX_LINE_ID {
            return Err(EditError::refused(format!(
                "Project has reached the maximum number of lines (limit = \
                 {MAX_LINES_IN_ID_SPACE}); shot ids encode as line_id * {FEATURE_ID_STRIDE} + \
                 position in 32 bits, so line ids past {MAX_LINE_ID} cannot hold shots"
            )));
        }
        if shots.iter().any(|s| !s.all_finite()) {
            return Err(EditError::refused(
                "Line has a shot with a non-finite (NaN/inf) value",
            ));
        }
        let line_name = meta
            .name
            .as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .unwrap_or("New line")
            .to_string();
        for (i, shot) in shots.iter().enumerate() {
            let label = format!("{line_name}: {i} \u{2192} {}", i + 1);
            if shot.length == 0.0 {
                return Err(EditError::refused(format!("Shot {label} has zero length")));
            }
            if shot.length < 0.0 {
                return Err(EditError::refused(format!(
                    "Shot {label} has negative length: {:.1}m",
                    shot.length
                )));
            }
        }
        if let (Some([x, y]), StartAt::New) = (placement, start) {
            if !x.is_finite() || !y.is_finite() {
                return Err(EditError::refused("The line's placement is not finite"));
            }
        }
        self.apply_edit(|p| {
            let id = p.add_line(meta, start, shots)?;
            if let Some(line) = p.line_mut(id) {
                absorb_legacy_credit(line);
            }
            let ctx = match start {
                StartAt::New => {
                    let at = placement.unwrap_or([0.0, 0.0]);
                    let station = p.line(id).map(|l| l.start).unwrap_or_default();
                    p.set_placement(station, at[0], at[1])?;
                    EditContext::line(id).with_placed(station)
                }
                StartAt::Station(_) => EditContext::line(id),
            };
            Ok((id, Epilogue::settle(ctx)))
        })
    }

    /// Delete a line (module docs).
    pub fn delete_line(&mut self, line: LineId) -> Result<Edited<DeletedLine>, EditError> {
        let position = self.line_position(line).ok_or_else(EditError::line_gone)?;
        self.apply_edit(|p| {
            let removed = p.lines_mut().remove(position);
            let chain: Vec<StationId> = removed.stations().collect();
            let gone = p.delete_stations_off_lines(&chain);
            let deleted = DeletedLine {
                line: removed,
                position,
                stations_deleted: gone.stations,
                datums_removed: gone.datums,
                closures_removed: gone.closures,
                comments_removed: gone.comments,
            };
            let epilogue = Epilogue {
                ctx: EditContext::none(),
                sweep: true,
                ..Epilogue::default()
            };
            Ok((deleted, epilogue))
        })
    }

    /// Why `name` cannot name a line (other than `replacing`), if it cannot:
    /// empty, a forbidden character, or another line's (today's words).
    /// Returns it trimmed.
    pub fn validate_line_name(
        &self,
        name: &str,
        replacing: &[LineId],
    ) -> Result<String, EditError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(EditError::refused("A line name cannot be empty"));
        }
        if let Some(refusal) = name_characters_refusal("line", name) {
            return Err(EditError::Refused(refusal));
        }
        let taken = self.lines().iter().any(|line| {
            !replacing.contains(&line.id) && line.name.as_deref().map(str::trim) == Some(name)
        });
        if taken {
            return Err(EditError::refused(format!(
                "A line named \"{name}\" already exists"
            )));
        }
        Ok(name.to_string())
    }

    /// Apply today's line metadata updates (module docs). A rename is refused
    /// as today (empty, forbidden character, another line's name).
    pub fn update_line_meta(
        &mut self,
        line: LineId,
        updates: &LineUpdates,
    ) -> Result<Edited<()>, EditError> {
        self.line_or_refuse(line)?;
        let name = match &updates.name {
            Some(name) => {
                Some(self.validate_line_name(name.as_deref().unwrap_or_default(), &[line])?)
            }
            None => None,
        };
        self.apply_edit(|p| {
            let Some(l) = p.line_mut(line) else {
                return Err(EditError::line_gone());
            };
            if let Some(name) = name {
                l.name = Some(name);
            }
            apply_legacy_credit_updates(l, updates);
            if let Some(color) = &updates.color {
                l.color.clone_from(color);
            }
            if let Some(direction) = updates.direction {
                l.direction = direction;
            }
            if let Some(quality) = updates.quality {
                // Today's rule: a non-finite or non-positive quality is the
                // default (1.0).
                l.quality = if quality.is_finite() && quality > 0.0 {
                    quality
                } else {
                    1.0
                };
            }
            Ok(((), Epilogue::settle(EditContext::none())))
        })
    }

    /// Set (or clear, to follow the project default) a line's azimuth
    /// reference: a geometry edit of the line (its bearings turn).
    pub fn set_line_azimuth_reference(
        &mut self,
        line: LineId,
        reference: Option<AzimuthReference>,
    ) -> Result<Edited<()>, EditError> {
        self.line_or_refuse(line)?;
        if reference.is_some_and(|r| !r.correction.is_finite()) {
            return Err(EditError::refused(
                "The azimuth correction must be a finite number",
            ));
        }
        self.apply_edit(|p| {
            let Some(l) = p.line_mut(line) else {
                return Err(EditError::line_gone());
            };
            l.azimuth_reference = reference;
            Ok(((), Epilogue::settle(EditContext::geometry(line))))
        })
    }

    /// Put the lines in `order` (every line once). Positions do not depend on
    /// the order, so nothing moves.
    pub fn reorder_lines(&mut self, order: &[LineId]) -> Result<Edited<()>, EditError> {
        let mut wanted: Vec<LineId> = order.to_vec();
        wanted.sort_unstable();
        let mut have: Vec<LineId> = self.lines().iter().map(|l| l.id).collect();
        have.sort_unstable();
        if wanted != have {
            return Err(EditError::refused(
                "The new line order must name every line exactly once",
            ));
        }
        self.apply_edit(|p| {
            let mut lines = std::mem::take(p.lines_mut());
            let mut ordered = Vec::with_capacity(lines.len());
            for id in order {
                if let Some(i) = lines.iter().position(|l| l.id == *id) {
                    ordered.push(lines.swap_remove(i));
                }
            }
            *p.lines_mut() = ordered;
            Ok(((), Epilogue::settle(EditContext::none())))
        })
    }
}

/// A line's legacy surveyor and date go to every shot whose credit is empty,
/// then leave the line (today's `Line::absorb_legacy_credit`); a shot-less
/// line keeps them.
pub(super) fn absorb_legacy_credit(line: &mut Line) {
    if line.shots.is_empty() {
        return;
    }
    let template = stored_credit(SurveyCredit::from_legacy(
        line.legacy_surveyor.take().as_deref(),
        line.legacy_date.take(),
    ));
    let Some(template) = template else {
        return;
    };
    for shot in &mut line.shots {
        if shot.credit.as_deref().is_none_or(SurveyCredit::is_empty) {
            shot.credit = Some(std::sync::Arc::clone(&template));
        }
    }
}

/// Today's `LineUpdates::surveyor` / `date`: set on every shot (a shot-less
/// line keeps the legacy pair). The surveyors are replaced by the text's;
/// the explorers only when the text is in the Ariane `"explorers /
/// surveyors"` form.
fn apply_legacy_credit_updates(line: &mut Line, updates: &LineUpdates) {
    if line.shots.is_empty() {
        if let Some(v) = &updates.surveyor {
            line.legacy_surveyor = v.clone().filter(|t| !t.trim().is_empty());
        }
        if let Some(v) = &updates.date {
            line.legacy_date = *v;
        }
    }
    if let Some(text) = &updates.surveyor {
        let text = text.as_deref().unwrap_or_default();
        let has_explorer_half = legacy_halves(text).is_some();
        let (explorers, surveyors) = legacy_people(text);
        for shot in &mut line.shots {
            let mut credit = shot.credit.as_deref().cloned().unwrap_or_default();
            credit.surveyors.clone_from(&surveyors);
            if has_explorer_half {
                credit.explorers.clone_from(&explorers);
            }
            shot.credit = stored_credit(credit);
        }
    }
    if let Some(date) = &updates.date {
        for shot in &mut line.shots {
            let mut credit = shot.credit.as_deref().cloned().unwrap_or_default();
            credit.date = *date;
            shot.credit = stored_credit(credit);
        }
    }
}
