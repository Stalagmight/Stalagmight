//! Add .stalx / Merge Projects (plan 5.8, 4.4): copy a whole source survey
//! into this one.
//!
//! - Ids: station ids (and archived records' ids, whose lines are remapped)
//!   are offset by this project's `next_station_id`, so both keep theirs and
//!   none collides; lines get the next line ids in source order; closures
//!   are re-minted and keep their archived end (the epilogue's C1/C2
//!   normalisation runs over them, RS 4.4); Sections are
//!   appended with fresh ids and names made unique (`Upper level (2)`); a shot
//!   uuid this project already holds is re-minted for the copy (newcomers
//!   yield, as today); a repeated line name gets ` (k)`.
//! - Each copied line carries its resolved azimuth reference, so a line that
//!   inherited its source's default keeps that value here.
//! - No cross-source junction is created: the copy is its own pieces.
//! - The copy moves rigidly by today's `place_by_datum` vector: the source's
//!   frame station goes to `gps_to_local(its GPS, this frame)`, or to the
//!   origin when this project has no frame (the copied datum then becomes the
//!   frame). Copied placements are translated by that vector; each copied
//!   placement datum's correction is recomputed with the frame-change formula
//!   (`correction_to_hold`), so every copied piece, free ones included, keeps
//!   its place relative to its cave even where the tangent planes disagree.
//!   Nothing pre-existing moves. A source with no datum is not moved.
//! - The entrance elevation: the first one wins, a disagreeing source is
//!   reported; the location is adopted or appended.
//!
//! Import hashes and the file identity are the command layer's to extend.

use std::collections::BTreeSet;
use std::ops::Range;

use crate::station_naming::dedupe_line_name;
use crate::{
    clamp_section_name, gps_to_local, reminted_shot_uuid_avoiding, LineId, Section, SectionId,
    StationId, MAX_LINE_ID, MAX_STATION_ID,
};

use super::ops::{OpError, OpOutcome, Sweep};
use super::{
    BuildError, ClosureId, EditContext, LoopClosure, Placement, Project, RawPositions, ZeroDatum,
};

/// A translation smaller than this on both axes is no move (today's
/// `OFFSET_DUST_M`).
const DUST_M: f64 = 1e-6;

/// The entrance elevations closer than this agree (today's tolerance).
const ELEVATION_TOLERANCE_M: f64 = 0.005;

/// A Section name an automatic path gives when the name is empty.
const FALLBACK_SECTION_NAME: &str = "Section";

/// A copied shot whose uuid was re-minted because this project held it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShotUuidRename {
    /// The copy's line (target id).
    pub line: LineId,
    /// The uuid the shot has in the source.
    pub source: String,
    /// The uuid it has here.
    pub target: String,
}

/// What an Add copied, for the history label, undo and the receipt.
#[derive(Debug, Clone, PartialEq)]
pub struct AppendSummary {
    /// `(source line, target line)`, in source order.
    pub line_map: Vec<(LineId, LineId)>,
    /// Target station id = source station id + this.
    pub station_offset: StationId,
    /// The copied stations, ascending.
    pub stations: Vec<StationId>,
    /// `(source closure, target closure)`, in source order.
    pub closure_map: Vec<(ClosureId, ClosureId)>,
    /// `(source Section, target Section)`, in source order.
    pub section_map: Vec<(SectionId, SectionId)>,
    /// The copied datums' list indices.
    pub datums: Range<usize>,
    /// The copied placements' stations.
    pub placements: Vec<StationId>,
    /// Copied shots whose uuid changed.
    pub shot_uuid_renames: Vec<ShotUuidRename>,
    /// Copied lines whose name was made unique.
    pub renamed_lines: Vec<LineId>,
    /// The rigid translation applied to the copy.
    pub translation: [f64; 2],
    /// `(incoming, kept)` when the source's entrance elevation disagreed.
    pub elevation_conflict: Option<(f64, f64)>,
}

impl AppendSummary {
    /// The copied lines, in source order.
    pub fn new_lines(&self) -> Vec<LineId> {
        self.line_map.iter().map(|&(_, target)| target).collect()
    }
}

impl Project {
    /// Copy `source` into this project (see the module docs). Refused when the
    /// source is not a valid model or an id space would run out; nothing is
    /// changed then.
    pub fn append_project(
        &mut self,
        source: &Project,
    ) -> Result<OpOutcome<AppendSummary>, OpError> {
        let violations = source.validate();
        if !violations.is_empty() {
            return Err(OpError::SourceInvalid(violations));
        }
        let first_line = self.next_line_id();
        let line_count = source.lines().len() as u64;
        if line_count > 0 && u64::from(first_line) + line_count - 1 > u64::from(MAX_LINE_ID) {
            return Err(BuildError::LineIdsExhausted.into());
        }
        let base = self.counters.next_station_id;
        if u64::from(base) + u64::from(source.counters.next_station_id) > u64::from(MAX_STATION_ID)
        {
            return Err(BuildError::StationIdsExhausted.into());
        }
        let positions = source.raw_positions();
        let translation = self.append_translation(source, &positions);
        self.run_op(Sweep::Skip, |p| {
            p.append_copy(source, first_line, base, translation, &positions)
        })
    }

    /// Today's `place_by_datum` vector: the source's frame station onto its
    /// GPS in this frame (or the origin), zero without a source datum, for a
    /// non-finite result, or within dust.
    fn append_translation(&self, source: &Project, positions: &RawPositions) -> [f64; 2] {
        let frame_before = self.frame_datum().map(|i| self.datums[i].gps);
        let Some(datum) = source.frame_datum().and_then(|i| source.datums.get(i)) else {
            return [0.0, 0.0];
        };
        let Some([x, y]) = positions.xy(datum.station) else {
            return [0.0, 0.0];
        };
        let target = frame_before.map_or((0.0, 0.0), |frame| gps_to_local(&datum.gps, &frame));
        let (dx, dy) = (target.0 - x, target.1 - y);
        if !dx.is_finite() || !dy.is_finite() || (dx.abs() <= DUST_M && dy.abs() <= DUST_M) {
            [0.0, 0.0]
        } else {
            [dx, dy]
        }
    }

    fn append_copy(
        &mut self,
        source: &Project,
        first_line: LineId,
        base: StationId,
        translation: [f64; 2],
        positions: &RawPositions,
    ) -> Result<(AppendSummary, EditContext), OpError> {
        let shift = |station: StationId| station + base;
        let section_map = self.append_sections(&source.sections)?;
        let section_of = |id: SectionId| {
            section_map
                .iter()
                .find(|&&(from, _)| from == id)
                .map(|&(_, to)| to)
        };

        // The copied lines' ids, in the source's list order (each source line
        // becomes `first_line + position`): archived records' lines are
        // remapped by it.
        let record_line = |id: LineId| {
            source
                .lines()
                .iter()
                .position(|l| l.id == id)
                .and_then(|i| LineId::try_from(i).ok())
                .and_then(|i| first_line.checked_add(i))
        };
        let mut stations = Vec::with_capacity(source.stations.len());
        for station in source.stations.values() {
            let mut copy = station.clone();
            copy.id = shift(station.id);
            for record in &mut copy.archived {
                record.id = shift(record.id);
                record.into = record.into.map(shift);
                record.lines = record
                    .lines
                    .iter()
                    .filter_map(|&l| record_line(l))
                    .collect();
            }
            stations.push(copy.id);
            self.insert_station(copy)?;
        }
        if source.counters.next_station_id > 0 {
            self.observe_station_id(shift(source.counters.next_station_id - 1));
        }

        let line_map: Vec<(LineId, LineId)> = source
            .lines()
            .iter()
            .zip(first_line..)
            .map(|(line, id)| (line.id, id))
            .collect();
        let line_of = |id: LineId| {
            line_map
                .iter()
                .find(|&&(from, _)| from == id)
                .map(|&(_, to)| to)
        };
        let target_uuids: BTreeSet<String> = self
            .lines()
            .iter()
            .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone()))
            .collect();
        let mut in_use = target_uuids.clone();
        in_use.extend(
            source
                .lines()
                .iter()
                .flat_map(|l| l.shots.iter().map(|s| s.uuid.clone())),
        );
        let mut line_names: BTreeSet<String> =
            self.lines().iter().filter_map(|l| l.name.clone()).collect();
        let mut shot_uuid_renames = Vec::new();
        let mut renamed_lines = Vec::new();
        for (line, &(_, id)) in source.lines().iter().zip(&line_map) {
            let mut copy = line.clone();
            copy.id = id;
            copy.start = shift(line.start);
            copy.azimuth_reference = Some(source.azimuth_reference_for(line));
            copy.section_id = line.section_id.and_then(section_of);
            copy.hand_moves = line
                .hand_moves
                .iter()
                .map(|(&station, &hand)| (shift(station), hand))
                .collect();
            for shot in &mut copy.shots {
                shot.to = shift(shot.to);
                if target_uuids.contains(&shot.uuid) {
                    let fresh = reminted_shot_uuid_avoiding(id, &shot.uuid, |c| in_use.contains(c));
                    in_use.insert(fresh.clone());
                    shot_uuid_renames.push(ShotUuidRename {
                        line: id,
                        source: std::mem::replace(&mut shot.uuid, fresh.clone()),
                        target: fresh,
                    });
                }
            }
            if let Some(name) = copy.name.take() {
                let name = if line_names.contains(&name) {
                    renamed_lines.push(id);
                    dedupe_line_name(&name, &|n| line_names.contains(n))
                } else {
                    name
                };
                line_names.insert(name.clone());
                copy.name = Some(name);
            }
            self.insert_line(copy)?;
        }

        let datum_start = self.datums.len();
        for datum in &source.datums {
            self.datums.push(ZeroDatum {
                station: shift(datum.station),
                via: datum.via.and_then(line_of),
                correction: None,
                ..*datum
            });
        }
        let mut ctx = EditContext::none();
        let mut placements = Vec::with_capacity(source.placements.len());
        for placement in &source.placements {
            let station = shift(placement.station);
            self.placements.push(Placement {
                station,
                x: placement.x + translation[0],
                y: placement.y + translation[1],
            });
            placements.push(station);
            ctx = ctx.with_placed(station);
        }
        let mut closure_map = Vec::with_capacity(source.closures.len());
        for closure in &source.closures {
            let id = self.mint_closure_id();
            self.closures.push(LoopClosure {
                id,
                a: shift(closure.a),
                a_via: closure.a_via.and_then(line_of),
                b: shift(closure.b),
                b_via: closure.b_via.and_then(line_of),
                strategy: closure.strategy,
                archived: closure.archived,
            });
            closure_map.push((closure.id, id));
        }
        // Corrections last, against the frame as it is now (the copied frame
        // when this project had none).
        for datum in source.placement_datums().into_iter().flatten() {
            let Some([x, y]) = positions.xy(source.datums[datum].station) else {
                continue;
            };
            let index = datum_start + datum;
            self.datums[index].correction =
                self.correction_to_hold(index, [x + translation[0], y + translation[1]]);
            ctx = ctx.with_corrected(index);
        }

        let elevation_conflict = self.combine_surface_elevation(source);
        self.combine_location(source);
        Ok((
            AppendSummary {
                line_map,
                station_offset: base,
                stations,
                closure_map,
                section_map,
                datums: datum_start..self.datums.len(),
                placements,
                shot_uuid_renames,
                renamed_lines,
                translation,
                elevation_conflict,
            },
            ctx,
        ))
    }

    /// Append copies of `sections` with fresh ids and names unique among this
    /// project's Sections and the copies before them (today's
    /// `append_sections`). Returns `(source id, new id)` pairs.
    fn append_sections(
        &mut self,
        sections: &[Section],
    ) -> Result<Vec<(SectionId, SectionId)>, OpError> {
        let mut taken: BTreeSet<String> = self.sections.iter().map(|s| s.name.clone()).collect();
        let mut map = Vec::with_capacity(sections.len());
        for section in sections {
            let base = clamp_section_name(&section.name);
            let base = if base.is_empty() {
                FALLBACK_SECTION_NAME.to_string()
            } else {
                base
            };
            let name = if taken.contains(&base) {
                // `taken` is finite, so some suffix is free.
                let mut n = 2u64;
                loop {
                    let candidate = format!("{base} ({n})");
                    if !taken.contains(&candidate) {
                        break candidate;
                    }
                    n += 1;
                }
            } else {
                base
            };
            taken.insert(name.clone());
            let id = self.mint_section_id()?;
            self.sections.push(Section {
                id,
                name,
                color: section.color.clone(),
            });
            map.push((section.id, id));
        }
        Ok(map)
    }

    /// The entrance elevation: the first finite one wins; `(incoming, kept)`
    /// when the source's disagrees by more than the tolerance.
    fn combine_surface_elevation(&mut self, source: &Project) -> Option<(f64, f64)> {
        let incoming = source
            .settings
            .surface_elevation
            .filter(|v| v.is_finite())?;
        match self.settings.surface_elevation {
            None => {
                self.settings.surface_elevation = Some(incoming);
                None
            }
            Some(existing) if (existing - incoming).abs() > ELEVATION_TOLERANCE_M => {
                Some((incoming, existing))
            }
            Some(_) => None,
        }
    }

    /// The location: adopted when this project has none, else appended unless
    /// already mentioned.
    fn combine_location(&mut self, source: &Project) {
        let Some(incoming) = source.location.as_deref() else {
            return;
        };
        match &mut self.location {
            None => self.location = Some(incoming.to_string()),
            Some(existing) => {
                if !existing.contains(incoming) {
                    existing.push_str(", ");
                    existing.push_str(incoming);
                }
            }
        }
    }
}
