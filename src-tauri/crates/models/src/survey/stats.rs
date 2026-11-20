//! Survey statistics (plan 3.8) from measured geometry (today's
//! `Project::stats` plus the station-derived figures the caller filled in:
//! `depth_range` and `calculate_penetration_with`).
//!
//! `station_count` counts stations (a junction once), as the file's summary
//! does; `slot_count` counts slots (a junction once per line), which was
//! 0.6.0's `station_count`; `junction_count` counts the stations on two or
//! more lines. The wire's [`ProjectStats`] carries the station and junction
//! counts (review finding UX-8: the panel showed slots and the wire's merge
//! pairs, so a Split Line Here raised both).

use chrono::NaiveDate;

use crate::{ProjectStats, ShotKind};

use super::{Project, RawPositions};

/// Statistics of one project state.
#[derive(Debug, Clone, PartialEq)]
pub struct SurveyStats {
    pub line_count: usize,
    /// Every shot, reference shots included.
    pub shot_count: usize,
    /// Surveyed length: every shot but reference ties (#689).
    pub total_length: f64,
    /// Shallowest and deepest absolute slot depth (#481), `0` with no station.
    pub min_depth: f64,
    pub max_depth: f64,
    /// Distinct stations on lines.
    pub station_count: usize,
    /// Slots (0.6.0's station count: shots + one per line).
    pub slot_count: usize,
    /// Stations on two or more lines.
    pub junction_count: usize,
    /// Loop closures.
    pub loop_count: usize,
    /// The longest along-cave distance from an entrance.
    pub penetration_distance: f64,
    pub first_survey_date: Option<NaiveDate>,
    pub last_survey_date: Option<NaiveDate>,
}

impl SurveyStats {
    /// The wire's `ProjectStats`: distinct stations and junctions.
    pub fn to_project_stats(&self) -> ProjectStats {
        ProjectStats {
            line_count: self.line_count,
            shot_count: self.shot_count,
            total_length: self.total_length,
            min_depth: self.min_depth,
            max_depth: self.max_depth,
            station_count: self.station_count,
            junction_count: self.junction_count,
            loop_count: self.loop_count,
            penetration_distance: self.penetration_distance,
            first_survey_date: self.first_survey_date,
            last_survey_date: self.last_survey_date,
        }
    }
}

impl Project {
    /// Statistics from the measured layout `positions` of this state;
    /// `include_reference` is the `gas.include_reference_legs` opt-in of the
    /// penetration figure (#27).
    pub fn survey_stats(&self, positions: &RawPositions, include_reference: bool) -> SurveyStats {
        let mut shot_count = 0;
        let mut total_length = 0.0;
        let mut first: Option<NaiveDate> = None;
        let mut last: Option<NaiveDate> = None;
        for line in self.lines() {
            let mut line_length = 0.0;
            for shot in &line.shots {
                match shot.kind {
                    ShotKind::Normal | ShotKind::Jump => line_length += shot.length,
                    ShotKind::Reference => {}
                }
                if let Some(date) = shot.credit.as_ref().and_then(|c| c.date) {
                    first = Some(first.map_or(date, |d| d.min(date)));
                    last = Some(last.map_or(date, |d| d.max(date)));
                }
            }
            shot_count += line.shots.len();
            total_length += line_length;
        }
        let slots = positions.slots(self);
        let (min_depth, max_depth) = if slots.is_empty() {
            (0.0, 0.0)
        } else {
            slots.iter().fold((f64::MAX, f64::MIN), |(lo, hi), s| {
                (lo.min(s.z), hi.max(s.z))
            })
        };
        let topology = self.topology();
        SurveyStats {
            line_count: self.lines().len(),
            shot_count,
            total_length,
            min_depth,
            max_depth,
            station_count: topology.stations().len(),
            slot_count: shot_count + self.lines().len(),
            junction_count: topology
                .stations()
                .iter()
                .filter(|&&s| topology.is_junction(s))
                .count(),
            loop_count: self.closures.len(),
            penetration_distance: self.penetration(include_reference),
            first_survey_date: first,
            last_survey_date: last,
        }
    }
}
