//! Binary IPC format for `load_project_full`.
//!
//! The default Tauri IPC serializes the response as JSON. For a project with
//! 14k sections + 75k shots + 89k stations that produces ~25 MB of JSON, and
//! both `serde_json::to_string` on the Rust side and `JSON.parse` on the JS
//! side run synchronously on threads the UI needs. We measured a 5.6 s
//! post-handler stall on the load path; ~5 s of it is the JSON
//! serialize/transport/parse round-trip.
//!
//! This module defines an alternative shape that:
//!   1. Keeps the small/sparse metadata (project header, section metadata,
//!      station_merges, zero_datums, loop_closures, …) as ordinary
//!      `serde`-encoded fields. They're small; msgpack handles them fine.
//!   2. Packs the bulk numeric data (shot fields, station coordinates) into
//!      typed-array byte buffers. Each buffer is a flat `Vec<u8>` whose layout
//!      is a multiple of the underlying scalar (Float32Array, Float64Array,
//!      Uint32Array). The JS side reconstructs `TypedArray` views over those
//!      bytes - no per-element parsing.
//!   3. Sends sparse extras (comments, GPS coordinates, merged_with relations,
//!      closures_with relations) as small `(global_index, value)` lists.
//!
//! The whole struct is encoded with `rmp-serde` (msgpack) and the resulting
//! `Vec<u8>` is returned via `tauri::ipc::Response::new(bytes)` - Tauri
//! passes raw bytes to JS as an `ArrayBuffer`, no JSON parse on either side.

use std::collections::HashMap;

use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_bytes::ByteBuf;

use crate::core::loading::{DisplaySettings, LoadProjectFullResult};
use crate::models::{
    AzimuthReference, ClosureRef, FrozenShotRef, GpsCoordinates, LoopClosureWire, Person,
    ProjectStats, ProjectWarning, SectionId, ShotId, StationMergeWire, StationRef, SurveyCredit,
    SurveyDirection, ZeroDatumWire,
};

/// f32 sentinel encoding `None` for nullable numeric fields. `f32::NAN`
/// would also work but `f32::from_bits(0x7FC0_0001)` is a signaling-like
/// quiet NaN that round-trips identically through DataView reads in JS.
/// We just use f32::NAN; both Rust's `f32::NAN` and JS's `NaN` compare
/// false to themselves so the JS side does `Number.isNaN(v) ? null : v`.
fn nan_or(value: Option<f64>) -> f32 {
    value.map(|v| v as f32).unwrap_or(f32::NAN)
}

/// Number of f32 fields per shot in `shots_floats`. Keep this in sync with
/// `pack_shots` and the JS decoder. Order matters: it's the wire format.
pub const SHOT_FLOAT_FIELDS: usize = 12;
/// Per-shot stride in `shots_floats` (in bytes).
pub const SHOT_FLOAT_STRIDE: usize = SHOT_FLOAT_FIELDS * 4;

/// Number of u32 fields per shot in `shots_uints`.
/// Layout: section_id, shot_id, marker_index_or_u32_max.
pub const SHOT_UINT_FIELDS: usize = 3;
pub const SHOT_UINT_STRIDE: usize = SHOT_UINT_FIELDS * 4;

/// Layout of `stations_floats`: x, y, z (Float64). 3 doubles per station.
pub const STATION_FLOAT_FIELDS: usize = 3;
pub const STATION_FLOAT_STRIDE: usize = STATION_FLOAT_FIELDS * 8;

/// Layout of `stations_uints`: section_id, station_index. 2 u32 per station.
pub const STATION_UINT_FIELDS: usize = 2;
pub const STATION_UINT_STRIDE: usize = STATION_UINT_FIELDS * 4;

/// Wire-format version. Bumped if the byte layout changes. The JS decoder
/// rejects mismatches so a stale frontend can't silently misread.
/// Bumped from 1 → 2 to include a `ProjectHeaderBinary.collab` field, which
/// v4 removed again (see below).
/// Bumped from 2 → 3 for two independent layout changes that landed in the
/// same unreleased version, so one bump covers both:
/// - #27: the per-shot flags byte now carries a 2-bit `ShotKind` in bits 0-1
///   instead of a single `is_reference` bit, so a stale frontend would read a
///   Reference (discriminant 2) as "not virtual".
/// - #270: the envelope includes `is_remote`, so the frontend's SpeleoDB
///   read-only gate resolves synchronously with the load instead of defaulting
///   open until `speleodb_link_status` answers.
/// Bumped from 3 → 4 when Ariane collaboration mode was removed: the header
/// no longer carries a `collab` field, so a stale frontend would misread the
/// envelope from that field onwards.
///
/// Bumped 4 → 5 for the #438 north references: the project header gained
/// `default_azimuth_reference` and every section header gained
/// `azimuth_reference`. Both are APPENDED, so a stale frontend reading a
/// msgpack map would simply ignore them - but the section field sits inside the
/// per-section struct that the decoder walks positionally, and a frontend that
/// does not know it exists would render every corrected section uncorrected
/// while believing it had the whole project.
///
/// Bumped 5 → 6 for station names: the envelope gained the sparse
/// `station_names` list. A map field a stale frontend would ignore, but it
/// would then show every imported mNemo station number as missing - the same
/// "believes it has the whole project" case as 4 → 5.
///
/// Bumped 6 → 7 for per-shot survey credit: the section header lost its
/// `surveyor`/`date` (a section keeps none - its shots do), the project header
/// gained one `credit_people` table, every section header its distinct
/// `credits`, and the envelope a `shots_credit` index per shot. A stale
/// frontend would read the section headers positionally wrong and show every
/// shot uncredited.
pub const BINARY_FORMAT_VERSION: u32 = 7;

#[derive(Serialize)]
pub struct LoadProjectFullBinaryResult {
    pub version: u32,
    pub loading_id: u64,
    /// Is this a SpeleoDB project? Mirrors `LoadProjectFullResult::is_remote`
    /// (the same value the backend edit gate was set from), so the frontend can
    /// settle its read-only gate before the project renders (#270).
    pub is_remote: bool,
    pub project_header: ProjectHeaderBinary,

    pub shot_count: u32,
    /// Per-shot f32 fields in fixed order - see SHOT_FLOAT_FIELDS comment.
    /// Order: azimuth_in, azimuth_out, length, depth_in, depth_out,
    ///        pitch_in, pitch_out, left, right, up, down, temperature.
    /// NaN means `None` for the nullable fields.
    pub shots_floats: ByteBuf,
    /// Per-shot u32 fields. Order: section_id, shot_id, marker_or_max
    /// where marker_or_max = u32::MAX when `marker_index` is None.
    pub shots_uints: ByteBuf,
    /// Per-shot flags: bits 0-1 = `ShotKind` discriminant (0 Normal, 1 Jump,
    /// 2 Reference). One byte per shot. Bits 2-7 are reserved.
    pub shots_flags: ByteBuf,
    /// Sparse map of (global_shot_index, comment). global_shot_index is the
    /// row index into the per-shot arrays above (NOT a Shot.id).
    pub shot_comments: Vec<(u32, String)>,
    /// Per-shot u32: the index of the shot's credit in its SECTION's
    /// [`SectionHeaderBinary::credits`], `u32::MAX` for a shot with none. EMPTY
    /// (zero bytes, not `shot_count * 4`) when no shot of the project carries a
    /// credit, so an uncredited project pays nothing.
    pub shots_credit: ByteBuf,

    pub station_count: u32,
    /// Per-station f64 coordinates: x, y, z (24 bytes per station).
    pub stations_floats: ByteBuf,
    /// Per-station u32 fields: section_id, station_index.
    pub stations_uints: ByteBuf,
    /// Per-station flags: bit 0 = is_zero_datum. One byte per station.
    pub stations_flags: ByteBuf,
    /// Sparse: stations with `gps_coordinates`.
    pub station_gps: Vec<(u32, GpsCoordinates)>,
    /// Sparse: stations with non-empty `merged_with`.
    pub station_merged_with: Vec<(u32, Vec<StationRef>)>,
    /// Sparse: stations with non-empty `closures_with`.
    pub station_closures_with: Vec<(u32, Vec<ClosureRef>)>,
    /// Sparse: stations with non-empty `comment`.
    pub station_comments: Vec<(u32, String)>,
    /// Sparse: stations with a non-empty `name` (e.g. an mNemo station number).
    /// Dense on a DMP-imported project, but still keyed by station so the
    /// common unnamed station costs nothing.
    pub station_names: Vec<(u32, String)>,

    pub stats: ProjectStats,
    pub warnings: Vec<ProjectWarning>,
    pub display_settings: DisplaySettings,
    pub frozen_shots: Vec<FrozenShotRef>,
}

/// Mirror of `Project` minus `sections[i].shots` and minus `station_offsets`/
/// `station_comments` HashMaps. The shots are in the binary blobs above;
/// the HashMaps are converted to `Vec<(StationRef, ...)>` for msgpack.
#[derive(Serialize)]
pub struct ProjectHeaderBinary {
    pub name: String,
    pub location: Option<String>,
    pub created_at: DateTime<Utc>,
    pub modified_at: DateTime<Utc>,
    pub file_path: Option<String>,
    pub next_section_id: SectionId,
    pub next_closure_id: u64,
    /// Section metadata in declaration order. `shot_offset` and `shot_count`
    /// index into the per-shot binary arrays.
    pub sections: Vec<SectionHeaderBinary>,
    pub station_merges: Vec<StationMergeWire>,
    pub zero_datums: Vec<ZeroDatumWire>,
    pub loop_closures: Vec<LoopClosureWire>,
    pub section_offsets: Vec<(SectionId, (f64, f64))>,
    pub station_offsets: Vec<(StationRef, (f64, f64, f64))>,
    pub station_comments_map: Vec<(StationRef, String)>,
    pub imported_file_hashes: Vec<String>,
    pub imported_section_hashes: Vec<String>,
    /// Per-project validation thresholds (#57), so binary-loaded projects keep
    /// their own config instead of falling back to the global defaults.
    pub validation: crate::config::ValidationConfig,
    /// Entrance elevation (#431), so binary-loaded projects carry it too.
    pub surface_elevation: Option<f64>,
    /// The project-level north reference a section inherits when it declares
    /// none (#438). Without it a binary load resolved every inheriting section
    /// to "nothing recorded" and drew the survey uncorrected - the JSON path
    /// has carried it since the field existed.
    pub default_azimuth_reference: Option<AzimuthReference>,
    /// Every distinct person any shot credits, once: the table the sections'
    /// [`CreditBinary`] lists index into.
    pub credit_people: Vec<Person>,
}

/// One distinct survey credit of a section, its people as indices into
/// [`ProjectHeaderBinary::credit_people`].
#[derive(Serialize)]
pub struct CreditBinary {
    pub surveyors: Vec<u32>,
    pub explorers: Vec<u32>,
    pub date: Option<NaiveDate>,
}

#[derive(Serialize)]
pub struct SectionHeaderBinary {
    pub id: SectionId,
    pub name: Option<String>,
    pub color: String,
    pub direction: SurveyDirection,
    pub quality: f64,
    pub shot_offset: u32,
    pub shot_count: u32,
    pub next_shot_id: ShotId,
    /// This section's own north reference (#438); `None` inherits
    /// [`ProjectHeaderBinary::default_azimuth_reference`].
    pub azimuth_reference: Option<AzimuthReference>,
    /// The section's distinct shot credits, indexed by
    /// [`LoadProjectFullBinaryResult::shots_credit`].
    pub credits: Vec<CreditBinary>,
}

/// Interns people project-wide and credits per section for the envelope.
#[derive(Default)]
struct CreditTable {
    people: Vec<Person>,
    person_index: HashMap<Person, u32>,
}

impl CreditTable {
    fn person(&mut self, person: &Person) -> u32 {
        if let Some(index) = self.person_index.get(person) {
            return *index;
        }
        let index = u32::try_from(self.people.len()).unwrap_or(u32::MAX);
        self.people.push(person.clone());
        self.person_index.insert(person.clone(), index);
        index
    }

    fn credit(&mut self, credit: &SurveyCredit) -> CreditBinary {
        CreditBinary {
            surveyors: credit.surveyors.iter().map(|p| self.person(p)).collect(),
            explorers: credit.explorers.iter().map(|p| self.person(p)).collect(),
            date: credit.date,
        }
    }
}

/// Convert a `LoadProjectFullResult` into the binary-friendly layout.
/// All bulk fields are written as packed little-endian bytes; the rest is
/// passed through into the msgpack envelope.
pub fn encode(result: LoadProjectFullResult) -> Result<Vec<u8>> {
    let start = std::time::Instant::now();
    let LoadProjectFullResult {
        loading_id,
        project,
        stations,
        stats,
        warnings,
        display_settings,
        is_remote,
    } = result;

    // Compute the frozen shots while the whole project is still in scope (its
    // fields are moved into project_header below). The binary envelope used to
    // ship an empty list, leaving the frontend's frozen-shot gating stale on a
    // binary load until the next get_project_state refresh (issue #57 / Codex).
    let frozen_shots = project.compute_frozen_shots();

    // Sum shot count up front so we can pre-allocate exactly.
    let shot_count: usize = project.sections.iter().map(|s| s.shots.len()).sum();
    let mut shots_floats = Vec::with_capacity(shot_count * SHOT_FLOAT_STRIDE);
    let mut shots_uints = Vec::with_capacity(shot_count * SHOT_UINT_STRIDE);
    let mut shots_flags = Vec::with_capacity(shot_count);
    let mut shot_comments: Vec<(u32, String)> = Vec::new();
    let mut sections_meta: Vec<SectionHeaderBinary> = Vec::with_capacity(project.sections.len());
    let mut credit_table = CreditTable::default();
    let mut shots_credit = Vec::with_capacity(shot_count * 4);
    let mut any_credit = false;

    let mut shot_cursor: u32 = 0;
    for section in &project.sections {
        let shot_offset = shot_cursor;
        let shot_count = section.shots.len() as u32;
        let mut credits: Vec<CreditBinary> = Vec::new();
        let mut credit_index: HashMap<&SurveyCredit, u32> = HashMap::new();
        for shot in &section.shots {
            // Credit: the section's distinct credits, one index per shot.
            let index = match shot.credit.as_ref().filter(|c| !c.is_empty()) {
                Some(credit) => {
                    any_credit = true;
                    *credit_index.entry(credit).or_insert_with(|| {
                        credits.push(credit_table.credit(credit));
                        u32::try_from(credits.len() - 1).unwrap_or(u32::MAX)
                    })
                }
                None => u32::MAX,
            };
            shots_credit.extend_from_slice(&index.to_le_bytes());
            // Floats (f32): azimuth_in, azimuth_out, length, depth_in, depth_out,
            //               pitch_in, pitch_out, left, right, up, down, temperature.
            //
            // PRECISION: shot fields are stored as f64 in the in-memory
            // `Shot` struct but emitted as f32 on the wire to halve the
            // shot-floats buffer (48 vs 96 bytes per shot - ~3.6 MB
            // saved on a 75 k-shot project, with matching memory in
            // Float32Array views on the frontend).  f32 gives ~7
            // significant decimal digits, which exceeds the precision
            // of any field surveying instrument we know of in practice;
            // but a loaded shot's geometry IS rounded, so editing one
            // and saving (e.g. through EditShotDialog, which currently
            // re-sends all fields) writes the rounded value back to
            // SQLite.  Acceptable trade-off for now; revisit if a
            // workflow needs lossless round-tripping (bump f32→f64
            // here and in SurveyData's shot-float arrays, bump
            // BINARY_FORMAT_VERSION).
            //
            // Hot loop - pack into a single [u8; SHOT_FLOAT_STRIDE]
            // buffer and emit with one extend_from_slice rather than 12
            // separate calls. Saves ~10 % of the encode wall-clock on
            // large projects.
            let floats: [f32; SHOT_FLOAT_FIELDS] = [
                shot.azimuth_in as f32,
                shot.azimuth_out as f32,
                shot.length as f32,
                shot.depth_in as f32,
                shot.depth_out as f32,
                nan_or(shot.pitch_in),
                nan_or(shot.pitch_out),
                nan_or(shot.left),
                nan_or(shot.right),
                nan_or(shot.up),
                nan_or(shot.down),
                nan_or(shot.temperature),
            ];
            let mut float_bytes = [0u8; SHOT_FLOAT_STRIDE];
            for (i, v) in floats.iter().enumerate() {
                float_bytes[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            shots_floats.extend_from_slice(&float_bytes);
            // Uints: section_id, shot_id, marker_or_max. Same batched
            // packing trick.
            let marker = shot.marker_index.unwrap_or(u32::MAX);
            let mut uint_bytes = [0u8; SHOT_UINT_STRIDE];
            uint_bytes[0..4].copy_from_slice(&section.id.to_le_bytes());
            uint_bytes[4..8].copy_from_slice(&shot.id.to_le_bytes());
            uint_bytes[8..12].copy_from_slice(&marker.to_le_bytes());
            shots_uints.extend_from_slice(&uint_bytes);
            // Flags.
            shots_flags.push(shot.kind.as_u8());
            // Sparse comment.
            if let Some(comment) = &shot.comment {
                if !comment.is_empty() {
                    shot_comments.push((shot_cursor, comment.clone()));
                }
            }
            shot_cursor += 1;
        }
        sections_meta.push(SectionHeaderBinary {
            id: section.id,
            name: section.name.clone(),
            color: section.color.clone(),
            direction: section.direction,
            quality: section.quality,
            shot_offset,
            shot_count,
            next_shot_id: section.next_shot_id,
            azimuth_reference: section.azimuth_reference,
            credits,
        });
    }
    if !any_credit {
        shots_credit.clear();
    }

    // Stations.
    let station_count = stations.len();
    let mut stations_floats = Vec::with_capacity(station_count * STATION_FLOAT_STRIDE);
    let mut stations_uints = Vec::with_capacity(station_count * STATION_UINT_STRIDE);
    let mut stations_flags = Vec::with_capacity(station_count);
    let mut station_gps: Vec<(u32, GpsCoordinates)> = Vec::new();
    let mut station_merged_with: Vec<(u32, Vec<StationRef>)> = Vec::new();
    let mut station_closures_with: Vec<(u32, Vec<ClosureRef>)> = Vec::new();
    let mut station_comments: Vec<(u32, String)> = Vec::new();
    let mut station_names: Vec<(u32, String)> = Vec::new();

    for (idx, station) in stations.into_iter().enumerate() {
        let idx_u32 = idx as u32;
        // Same batched-write pattern as the shot loop above.
        let mut float_bytes = [0u8; STATION_FLOAT_STRIDE];
        float_bytes[0..8].copy_from_slice(&station.coordinates.x.to_le_bytes());
        float_bytes[8..16].copy_from_slice(&station.coordinates.y.to_le_bytes());
        float_bytes[16..24].copy_from_slice(&station.coordinates.z.to_le_bytes());
        stations_floats.extend_from_slice(&float_bytes);
        let mut uint_bytes = [0u8; STATION_UINT_STRIDE];
        uint_bytes[0..4].copy_from_slice(&station.station_ref.section_id.to_le_bytes());
        uint_bytes[4..8].copy_from_slice(&station.station_ref.station_index.to_le_bytes());
        stations_uints.extend_from_slice(&uint_bytes);
        stations_flags.push(u8::from(station.is_zero_datum));
        if let Some(gps) = station.gps_coordinates {
            station_gps.push((idx_u32, gps));
        }
        if !station.merged_with.is_empty() {
            station_merged_with.push((idx_u32, station.merged_with));
        }
        if let Some(closures) = station.closures_with.filter(|c| !c.is_empty()) {
            station_closures_with.push((idx_u32, closures));
        }
        if let Some(comment) = station.comment {
            if !comment.is_empty() {
                station_comments.push((idx_u32, comment));
            }
        }
        if let Some(name) = station.name {
            if !name.is_empty() {
                station_names.push((idx_u32, name));
            }
        }
    }

    // Resolve the stored StationKey-based cross-references to their positional wire
    // form (the frontend reasons by station_index) before moving the rest of
    // `project` out. A dangling ref (its anchoring shot was deleted) is OMITTED
    // rather than collapsed to a phantom station-0 relationship; the integrity
    // banner surfaces it and offers repair.
    let station_merges = project
        .station_merges
        .iter()
        .filter_map(|m| project.merge_to_wire(m))
        .collect();
    let zero_datums = project
        .zero_datums
        .iter()
        .filter_map(|zd| project.datum_to_wire(zd))
        .collect();
    let loop_closures = project
        .loop_closures
        .iter()
        .filter_map(|c| project.closure_to_wire(c))
        .collect();
    let station_offsets = project
        .station_offsets
        .iter()
        .filter_map(|(k, v)| Some((project.resolve_key(k)?, *v)))
        .collect();
    let station_comments_map = project
        .station_comments
        .iter()
        .filter_map(|(k, v)| Some((project.resolve_key(k)?, v.clone())))
        .collect();

    let project_header = ProjectHeaderBinary {
        name: project.name,
        location: project.location,
        created_at: project.created_at,
        modified_at: project.modified_at,
        file_path: project
            .file_path
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned()),
        next_section_id: project.next_section_id,
        next_closure_id: project.next_closure_id,
        sections: sections_meta,
        station_merges,
        zero_datums,
        loop_closures,
        section_offsets: hashmap_to_pairs(project.section_offsets),
        station_offsets,
        station_comments_map,
        imported_file_hashes: project.imported_file_hashes,
        imported_section_hashes: project.imported_section_hashes,
        validation: project.validation,
        surface_elevation: project.surface_elevation,
        default_azimuth_reference: project.default_azimuth_reference,
        credit_people: credit_table.people,
    };

    let result = LoadProjectFullBinaryResult {
        version: BINARY_FORMAT_VERSION,
        loading_id,
        is_remote,
        project_header,
        shot_count: shot_count as u32,
        shots_floats: ByteBuf::from(shots_floats),
        shots_uints: ByteBuf::from(shots_uints),
        shots_flags: ByteBuf::from(shots_flags),
        shot_comments,
        shots_credit: ByteBuf::from(shots_credit),
        station_count: station_count as u32,
        stations_floats: ByteBuf::from(stations_floats),
        stations_uints: ByteBuf::from(stations_uints),
        stations_flags: ByteBuf::from(stations_flags),
        station_gps,
        station_merged_with,
        station_closures_with,
        station_comments,
        station_names,
        stats,
        warnings,
        display_settings,
        frozen_shots,
    };

    let t_pack = start.elapsed();
    let bytes = rmp_serde::to_vec_named(&result)
        .map_err(|e| anyhow::anyhow!("msgpack encode of load result: {e}"))?;
    let t_total = start.elapsed();
    tracing::debug!(
        shots = shot_count,
        stations = station_count,
        bytes = bytes.len(),
        pack_ms = t_pack.as_millis() as u64,
        msgpack_ms = (t_total - t_pack).as_millis() as u64,
        total_ms = t_total.as_millis() as u64,
        "load_project_full_binary encode"
    );
    Ok(bytes)
}

fn hashmap_to_pairs<K: Eq + std::hash::Hash, V>(map: HashMap<K, V>) -> Vec<(K, V)> {
    map.into_iter().collect()
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::core::loading::DisplaySettings;
    use crate::models::ShotKind;
    use crate::models::{Coordinates, Project, ProjectStats, Section, Shot, Station, StationRef};
    use chrono::Utc;
    use serde::Deserialize;

    fn empty_stats() -> ProjectStats {
        ProjectStats {
            section_count: 0,
            shot_count: 0,
            total_length: 0.0,
            min_depth: 0.0,
            max_depth: 0.0,
            station_count: 0,
            loop_count: 0,
            penetration_distance: 0.0,
            first_survey_date: None,
            last_survey_date: None,
        }
    }

    fn new_project(name: &str) -> Project {
        Project::new(name.to_string(), None)
    }

    fn new_section(id: u32, name: &str, color: &str) -> Section {
        use crate::models::SectionData;
        Section::new(
            id,
            SectionData {
                name: Some(name.to_string()),
                surveyor: None,
                date: None,
                color: color.to_string(),
                direction: None,
                quality: 1.0,
                shots: Vec::new(),
            },
        )
    }

    /// Mirror of `LoadProjectFullBinaryResult` for decoding inside tests.
    /// Keeping it in this file (rather than exposing public `Deserialize`)
    /// matches the role: the encoder writes; the JS decoder reads. This
    /// shadow type lets us round-trip in Rust without changing the public
    /// API.
    #[derive(Deserialize)]
    struct DecodedResult {
        version: u32,
        loading_id: u64,
        project_header: DecodedHeader,
        shot_count: u32,
        shots_floats: ByteBuf,
        shots_uints: ByteBuf,
        shots_flags: ByteBuf,
        shot_comments: Vec<(u32, String)>,
        #[serde(default)]
        shots_credit: ByteBuf,
        station_count: u32,
        stations_floats: ByteBuf,
        stations_uints: ByteBuf,
        stations_flags: ByteBuf,
    }
    #[derive(Deserialize)]
    struct DecodedHeader {
        name: String,
        sections: Vec<DecodedSection>,
        default_azimuth_reference: Option<AzimuthReference>,
        #[serde(default)]
        credit_people: Vec<Person>,
    }
    #[derive(Deserialize)]
    struct DecodedSection {
        id: SectionId,
        shot_offset: u32,
        shot_count: u32,
        azimuth_reference: Option<AzimuthReference>,
        #[serde(default)]
        credits: Vec<DecodedCredit>,
    }
    #[derive(Deserialize)]
    struct DecodedCredit {
        surveyors: Vec<u32>,
        explorers: Vec<u32>,
        date: Option<NaiveDate>,
    }

    fn shot(id: u32, azimuth: f64, length: f64, depth_in: f64, depth_out: f64) -> Shot {
        Shot {
            id,
            uuid: None,
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            depth_in,
            depth_out,
            pitch_in: None,
            pitch_out: None,
            left: None,
            right: None,
            up: None,
            down: None,
            comment: None,
            kind: ShotKind::Normal,
            marker_index: None,
            temperature: None,
            credit: None,
        }
    }

    #[test]
    fn round_trip_two_sections() {
        let mut project = new_project("test");
        let mut sec_a = new_section(1, "A", "#ff0000");
        sec_a.shots.push(shot(101, 0.0, 10.0, 0.0, -1.0));
        sec_a.shots.push(shot(102, 90.0, 5.0, -1.0, -2.0));
        sec_a.next_shot_id = 103;
        let mut sec_b = new_section(2, "B", "#00ff00");
        sec_b.shots.push(shot(201, 45.0, 7.0, 0.0, -0.5));
        sec_b.next_shot_id = 202;
        project.sections.push(sec_a);
        project.sections.push(sec_b);
        project.next_section_id = 3;

        let stations = vec![
            Station::with_coordinates(StationRef::new(1, 0), Coordinates::new(0.0, 0.0, 0.0)),
            Station::with_coordinates(StationRef::new(1, 1), Coordinates::new(10.0, 0.0, -1.0)),
            Station::with_coordinates(StationRef::new(1, 2), Coordinates::new(10.0, 5.0, -2.0)),
            Station::with_coordinates(StationRef::new(2, 0), Coordinates::new(0.0, 0.0, 0.0)),
            Station::with_coordinates(StationRef::new(2, 1), Coordinates::new(4.95, 4.95, -0.5)),
        ];

        let result = LoadProjectFullResult {
            loading_id: 42,
            project,
            stations,
            stats: empty_stats(),
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
        };

        let bytes = encode(result).unwrap();
        let decoded: DecodedResult = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(decoded.version, BINARY_FORMAT_VERSION);
        assert_eq!(decoded.loading_id, 42);
        assert_eq!(decoded.project_header.name, "test");
        assert_eq!(decoded.project_header.sections.len(), 2);
        assert_eq!(decoded.shot_count, 3);
        assert_eq!(decoded.shots_floats.len(), 3 * SHOT_FLOAT_STRIDE);
        assert_eq!(decoded.shots_uints.len(), 3 * SHOT_UINT_STRIDE);
        assert_eq!(decoded.shots_flags.len(), 3);
        assert_eq!(decoded.station_count, 5);
        assert_eq!(decoded.stations_floats.len(), 5 * STATION_FLOAT_STRIDE);
        assert_eq!(decoded.stations_uints.len(), 5 * STATION_UINT_STRIDE);
        assert_eq!(decoded.stations_flags.len(), 5);
        assert!(decoded.shot_comments.is_empty());

        // Section offsets index into the flat shots array
        assert_eq!(decoded.project_header.sections[0].id, 1);
        assert_eq!(decoded.project_header.sections[0].shot_offset, 0);
        assert_eq!(decoded.project_header.sections[0].shot_count, 2);
        assert_eq!(decoded.project_header.sections[1].id, 2);
        assert_eq!(decoded.project_header.sections[1].shot_offset, 2);
        assert_eq!(decoded.project_header.sections[1].shot_count, 1);

        // Verify a known float at a known offset: section 1, shot index 1
        // (the second shot, azimuth=90, length=5). Floats live at byte
        // offset (global_shot_index 1 * SHOT_FLOAT_STRIDE) of shots_floats.
        let base = SHOT_FLOAT_STRIDE;
        let azimuth_in_bytes: [u8; 4] = decoded.shots_floats[base..base + 4].try_into().unwrap();
        let azimuth_in = f32::from_le_bytes(azimuth_in_bytes);
        assert!((azimuth_in - 90.0).abs() < 1e-3);

        let _ = Utc::now(); // silence unused-import on chrono::Utc
    }

    fn result_for(project: Project) -> LoadProjectFullResult {
        LoadProjectFullResult {
            loading_id: 1,
            project,
            stations: Vec::new(),
            stats: empty_stats(),
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
        }
    }

    #[test]
    fn credit_rides_as_one_people_table_and_per_section_indices() {
        let person = |n: &str| Person::named(n).unwrap();
        let day = NaiveDate::from_ymd_opt(2024, 2, 3);
        let ann_bo = SurveyCredit {
            surveyors: vec![person("Ann"), person("Bo")],
            explorers: vec![person("Ex")],
            date: day,
        };
        let bo = SurveyCredit {
            surveyors: vec![person("Bo")],
            explorers: Vec::new(),
            date: None,
        };
        let mut project = new_project("credit");
        let mut a = new_section(1, "A", "#ff0000");
        for (i, credit) in [Some(&ann_bo), None, Some(&ann_bo), Some(&bo)]
            .into_iter()
            .enumerate()
        {
            let mut s = shot(100 + i as u32, 0.0, 1.0, 0.0, 0.0);
            s.credit = credit.cloned();
            a.shots.push(s);
        }
        let mut b = new_section(2, "B", "#00ff00");
        let mut s = shot(200, 0.0, 1.0, 0.0, 0.0);
        s.set_credit(bo);
        b.shots.push(s);
        project.sections.push(a);
        project.sections.push(b);

        let decoded: DecodedResult =
            rmp_serde::from_slice(&encode(result_for(project)).unwrap()).unwrap();
        let names: Vec<&str> = decoded
            .project_header
            .credit_people
            .iter()
            .map(|p| p.name.as_str())
            .collect();
        // Each person once, project-wide, in first-appearance order.
        assert_eq!(names, ["Ann", "Bo", "Ex"]);
        let index: Vec<u32> = decoded
            .shots_credit
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        // Section A: two distinct credits; section B its own list.
        assert_eq!(index, [0, u32::MAX, 0, 1, 0]);
        let a = &decoded.project_header.sections[0].credits;
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0].surveyors.as_slice(), a[0].explorers.as_slice()),
            (&[0, 1][..], &[2][..])
        );
        assert_eq!(a[0].date, day);
        assert_eq!(a[1].surveyors, [1]);
        let b = &decoded.project_header.sections[1].credits;
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].surveyors, [1]);
    }

    #[test]
    fn an_uncredited_project_sends_no_credit_bytes() {
        let mut project = new_project("bare");
        let mut a = new_section(1, "A", "#ff0000");
        a.shots.push(shot(100, 0.0, 1.0, 0.0, 0.0));
        a.shots.push(shot(101, 0.0, 1.0, 0.0, 0.0));
        project.sections.push(a);
        let decoded: DecodedResult =
            rmp_serde::from_slice(&encode(result_for(project)).unwrap()).unwrap();
        assert!(decoded.shots_credit.is_empty());
        assert!(decoded.project_header.credit_people.is_empty());
        assert!(decoded.project_header.sections[0].credits.is_empty());
    }

    #[test]
    fn nan_round_trips_for_optional_fields() {
        let mut project = new_project("t");
        let mut sec = new_section(1, "S", "#abcdef");
        sec.shots.push(Shot {
            id: 1,
            uuid: None,
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 1.0,
            depth_in: 0.0,
            depth_out: 0.0,
            pitch_in: Some(10.0),
            pitch_out: None,
            left: Some(0.5),
            right: None,
            up: None,
            down: None,
            comment: Some("hello".to_string()),
            kind: ShotKind::Reference,
            marker_index: Some(7),
            temperature: None,
            credit: None,
        });
        sec.next_shot_id = 2;
        project.sections.push(sec);

        let result = LoadProjectFullResult {
            loading_id: 1,
            project,
            stations: vec![],
            stats: empty_stats(),
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
        };
        let bytes = encode(result).unwrap();
        let decoded: DecodedResult = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(decoded.shot_count, 1);
        // pitch_out at f32 index 6
        let bytes_at =
            |i: usize| -> [u8; 4] { decoded.shots_floats[i * 4..i * 4 + 4].try_into().unwrap() };
        let pitch_in = f32::from_le_bytes(bytes_at(5));
        let pitch_out = f32::from_le_bytes(bytes_at(6));
        assert!((pitch_in - 10.0).abs() < 1e-3);
        assert!(pitch_out.is_nan(), "pitch_out should be NaN for None");

        // Shot kind occupies bits 0-1 of the flags byte (#27).
        assert_eq!(
            ShotKind::from_u8(decoded.shots_flags[0] & 0b11),
            ShotKind::Reference
        );

        // Comment carried over
        assert_eq!(decoded.shot_comments.len(), 1);
        assert_eq!(decoded.shot_comments[0], (0u32, "hello".to_string()));

        // marker_index encoded as u32 (third uint slot)
        let marker_bytes: [u8; 4] = decoded.shots_uints[8..12].try_into().unwrap();
        assert_eq!(u32::from_le_bytes(marker_bytes), 7);

        // Survey direction (unused suffix, just to ensure import is referenced)
        let _ = SurveyDirection::default();
    }

    /// #438: the binary envelope carries BOTH halves of the north reference.
    ///
    /// It used to stop at `surface_elevation`, so a binary load - the fast path
    /// every real open takes - silently dropped the project default and every
    /// section's own, and the survey rendered uncorrected while the JSON path
    /// showed it right. Nothing failed; the map was just rotated.
    #[test]
    fn azimuth_references_survive_the_binary_envelope() {
        use crate::models::{AzimuthReference as Ref, CorrectionSource};

        let mut project = new_project("declination");
        project.default_azimuth_reference =
            Some(Ref::magnetic(-4.25, CorrectionSource::FileDeclared));

        let mut declared = new_section(1, "Declared", "#ff0000");
        declared.shots.push(shot(101, 0.0, 10.0, 0.0, 0.0));
        declared.azimuth_reference = Some(Ref::magnetic(
            10.5,
            CorrectionSource::IgrfAuto { epoch_year: 2024 },
        ));
        // Declares nothing of its own: it inherits, and the INHERITANCE is what
        // the project-level field has to preserve.
        let mut inheriting = new_section(2, "Inheriting", "#00ff00");
        inheriting.shots.push(shot(201, 0.0, 10.0, 0.0, 0.0));
        assert!(inheriting.azimuth_reference.is_none(), "premise");
        project.sections.push(declared);
        project.sections.push(inheriting);

        let result = LoadProjectFullResult {
            loading_id: 1,
            project,
            stations: vec![],
            stats: empty_stats(),
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
        };
        let bytes = encode(result).unwrap();
        let decoded: DecodedResult = rmp_serde::from_slice(&bytes).unwrap();

        assert_eq!(
            decoded.version, BINARY_FORMAT_VERSION,
            "adding header fields has to move the version the decoder pins"
        );
        assert_eq!(
            decoded.project_header.default_azimuth_reference,
            Some(Ref::magnetic(-4.25, CorrectionSource::FileDeclared)),
            "the project default is what an inheriting section resolves to"
        );
        assert_eq!(
            decoded.project_header.sections[0].azimuth_reference,
            Some(Ref::magnetic(
                10.5,
                CorrectionSource::IgrfAuto { epoch_year: 2024 }
            )),
            "a section's own reference, provenance and all"
        );
        assert_eq!(
            decoded.project_header.sections[1].azimuth_reference, None,
            "a section that declares nothing must not be given a value here"
        );
    }

    #[test]
    fn empty_project_round_trips() {
        let project = new_project("empty");
        let result = LoadProjectFullResult {
            loading_id: 0,
            project,
            stations: vec![],
            stats: empty_stats(),
            warnings: Vec::new(),
            display_settings: DisplaySettings::default(),
            is_remote: false,
        };
        let bytes = encode(result).unwrap();
        let decoded: DecodedResult = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded.shot_count, 0);
        assert_eq!(decoded.station_count, 0);
        assert!(decoded.shots_floats.is_empty());
        assert!(decoded.stations_floats.is_empty());
        assert!(decoded.project_header.sections.is_empty());
    }
}
