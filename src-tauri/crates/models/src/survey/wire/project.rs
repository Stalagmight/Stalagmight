//! The wire `Project`, `Line` and `ProjectState` (plan 9.2).
//!
//! The legacy model serialized itself for the IPC through a hand-written
//! view (`ProjectIpc`); these structs are that view as types, so their
//! derived serde IS the payload and their derived TS IS the declaration.
//! They carry the legacy TS names (`#[ts(rename)]`) and no `export`: no test
//! writes them, and `src/types/ipc/` keeps being generated from the legacy
//! types until the core switch points the generator here (`ts_tests.rs`
//! pins that the two generate the same file).

use std::sync::Arc;

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use ts_rs::TS;

use crate::config::ValidationConfig;
use crate::{
    AzimuthReference, FrozenShotRef, LineId, LoopClosureWire, ProjectStats, Section, SectionId,
    ShotId, StationMergeWire, SurveyDirection, ZeroDatumWire,
};

use super::super::{Line, Project, RawPositions, Shot};

/// The TS `Project`: today's payload without `line_offsets`.
///
/// Field order is the legacy payload's (`ProjectIpc`), which is also the
/// order of the generated declaration.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(rename = "Project")]
pub struct ProjectWire {
    pub name: String,
    pub location: Option<String>,
    #[ts(type = "string")]
    pub created_at: DateTime<Utc>,
    #[ts(type = "string")]
    pub modified_at: DateTime<Utc>,
    pub lines: Vec<LineWire>,
    /// Derived: for each junction (ascending station id), its first slot
    /// paired with each of its other slots.
    pub station_merges: Vec<StationMergeWire>,
    /// Every datum in list order, on its slot (its `via` line, else its
    /// station's first line), with its derived role, entrance and gap.
    pub zero_datums: Vec<ZeroDatumWire>,
    /// Every closure in list order, ends on their slots, metrics and
    /// alignment derived from the raw layout.
    pub loop_closures: Vec<LoopClosureWire>,
    pub file_path: Option<String>,
    pub next_line_id: LineId,
    pub imported_file_hashes: Vec<String>,
    pub imported_line_hashes: Vec<String>,
    pub next_closure_id: u64,
    pub validation: ValidationConfig,
    pub surface_elevation: Option<f64>,
    /// Always on the wire (`null` when unset), declared optional, as today.
    #[ts(optional)]
    pub default_azimuth_reference: Option<AzimuthReference>,
    pub cave_sections: Vec<Section>,
    pub next_cave_section_id: SectionId,
}

/// The TS `Line`: a line's metadata and its shots in order.
///
/// `reverse_parity`, `gauge_shot_uuid` and `gauge_neighbour_uuid` were the
/// legacy gauge bookkeeping (the least-squares gauge is now the piece's
/// `Placement` station): always `false` / `null`, kept so the shape does
/// not change. No frontend code reads them.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(rename = "Line")]
pub struct LineWire {
    pub id: LineId,
    pub name: Option<String>,
    /// A shot-less line's legacy surveyor text, on the wire only when set
    /// (absent from the TS shape), as today.
    #[serde(rename = "surveyor", skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub legacy_surveyor: Option<String>,
    /// A shot-less line's legacy survey date, likewise.
    #[serde(rename = "date", skip_serializing_if = "Option::is_none")]
    #[ts(skip)]
    pub legacy_date: Option<NaiveDate>,
    pub color: String,
    pub direction: SurveyDirection,
    pub quality: f64,
    pub shots: Vec<crate::Shot>,
    pub next_shot_id: ShotId,
    pub reverse_parity: bool,
    pub gauge_shot_uuid: Option<String>,
    pub gauge_neighbour_uuid: Option<String>,
    /// Always on the wire (`null` when unset), declared optional, as today.
    #[ts(optional)]
    pub azimuth_reference: Option<AzimuthReference>,
    #[serde(rename = "cave_section_id", skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub section_id: Option<SectionId>,
}

/// The TS `ProjectState`: `get_project_state`'s payload.
#[derive(Debug, Clone, Serialize, TS)]
#[ts(rename = "ProjectState")]
pub struct ProjectStateWire {
    pub project: ProjectWire,
    /// One entry per slot, lines in list order then chain index, raw
    /// (measured) coordinates.
    pub stations: Vec<crate::Station>,
    /// The stats: `station_count` counts distinct stations (a junction
    /// once), `junction_count` the stations on two or more lines.
    pub stats: ProjectStats,
    pub frozen_shots: Vec<FrozenShotRef>,
}

impl LineWire {
    /// The wire form of one line.
    pub fn of(line: &Line) -> Self {
        Self {
            id: line.id,
            name: line.name.clone(),
            legacy_surveyor: line.legacy_surveyor.clone(),
            legacy_date: line.legacy_date,
            color: line.color.clone(),
            direction: line.direction,
            quality: line.quality,
            shots: line.shots.iter().map(shot_wire).collect(),
            next_shot_id: line.next_shot_id,
            reverse_parity: false,
            gauge_shot_uuid: None,
            gauge_neighbour_uuid: None,
            azimuth_reference: line.azimuth_reference,
            section_id: line.section_id,
        }
    }
}

/// The wire (and legacy) `Shot` of a station-graph shot: the same
/// measurement fields, the session id, the uuid; no `to` (the slot order
/// says where it ends).
pub(super) fn shot_wire(shot: &Shot) -> crate::Shot {
    crate::Shot {
        id: shot.id,
        uuid: Some(shot.uuid.clone()),
        azimuth_in: shot.azimuth_in,
        azimuth_out: shot.azimuth_out,
        length: shot.length,
        depth_in: shot.depth_in,
        depth_out: shot.depth_out,
        pitch_in: shot.pitch_in,
        pitch_out: shot.pitch_out,
        left: shot.left,
        right: shot.right,
        up: shot.up,
        down: shot.down,
        comment: shot.comment.clone(),
        kind: shot.kind,
        marker_index: shot.marker_index,
        temperature: shot.temperature,
        credit: shot.credit.as_ref().map(Arc::clone),
    }
}

impl Project {
    /// The wire `Project` of this state (computes the raw layout once).
    pub fn project_wire(&self) -> ProjectWire {
        let raw = self.raw_positions();
        self.project_wire_with(&raw)
    }

    /// [`Self::project_wire`] from this state's raw layout `raw` (for a
    /// caller that memoizes it per revision).
    pub fn project_wire_with(&self, raw: &RawPositions) -> ProjectWire {
        self.project_wire_aligned(raw, &self.closure_alignment(raw))
    }

    /// The wire project from `raw` and its closure alignment `aligned`.
    fn project_wire_aligned(&self, raw: &RawPositions, aligned: &[bool]) -> ProjectWire {
        ProjectWire {
            name: self.name.clone(),
            location: self.location.clone(),
            created_at: self.created_at,
            modified_at: self.modified_at,
            lines: self.lines().iter().map(LineWire::of).collect(),
            station_merges: self.station_merges_wire(),
            zero_datums: self.zero_datums_wire_with(raw),
            loop_closures: self.loop_closures_wire(raw, aligned),
            file_path: self
                .file_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            next_line_id: self.next_line_id(),
            imported_file_hashes: self.imported_file_hashes.clone(),
            imported_line_hashes: self.imported_line_hashes.clone(),
            next_closure_id: self.counters.next_closure_id,
            validation: self.settings.validation.clone(),
            surface_elevation: self.settings.surface_elevation,
            default_azimuth_reference: self.settings.default_azimuth_reference,
            cave_sections: self.sections.clone(),
            next_cave_section_id: self.counters.next_section_id,
        }
    }

    /// `get_project_state`'s payload: the wire project, the raw per-slot
    /// stations, today's stats (`include_reference_legs` is the
    /// `gas.include_reference_legs` opt-in of the penetration figure) and the
    /// frozen shots.
    pub fn project_state_wire(&self, include_reference_legs: bool) -> ProjectStateWire {
        let raw = self.raw_positions();
        self.project_state_wire_with(&raw, include_reference_legs)
    }

    /// [`Self::project_state_wire`] from this state's raw layout `raw`.
    pub fn project_state_wire_with(
        &self,
        raw: &RawPositions,
        include_reference_legs: bool,
    ) -> ProjectStateWire {
        let aligned = self.closure_alignment(raw);
        ProjectStateWire {
            project: self.project_wire_aligned(raw, &aligned),
            stations: self.stations_wire(raw),
            stats: self
                .survey_stats(raw, include_reference_legs)
                .to_project_stats(),
            frozen_shots: self.frozen_shots(&aligned),
        }
    }
}
