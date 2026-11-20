//! Survey geometry command for GPS-transformed survey data.
//!
//! Provides `get_survey_geometry_for_file`, which returns shot data with
//! GPS-transformed coordinates for lazy loading geometry from arbitrary
//! project files (used by GlobalView for on-demand loading).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::models::survey::Project;
use crate::models::{GpsCoordinates, LineId, ShotKind, StationId};
use crate::persistence::stalx_v2::{self, DecodeOptions, Fingerprint, Format};

/// Minimum cosine value to avoid division by near-zero at extreme latitudes.
/// This corresponds to latitudes beyond ±89.4° where the approximation breaks down.
const MIN_COS_LAT: f64 = 0.01;

/// A self-contained shot with GPS coordinates and metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct GeometryShot {
    /// GPS coordinates of the from station [lng, lat] (MapLibre order)
    pub from: [f64; 2],
    /// GPS coordinates of the to station [lng, lat] (MapLibre order)
    pub to: [f64; 2],
    /// Permanent, jump, or reference (#27). The map styles jumps and
    /// references differently, so the full kind crosses the IPC boundary, not
    /// just a virtual-vs-not bool.
    pub kind: ShotKind,
    /// Line ID this shot belongs to (for coloring/filtering)
    pub line_id: LineId,
}

/// Survey geometry response containing GPS-transformed shot data.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SurveyGeometry {
    /// Array of shots with GPS coordinates
    pub shots: Vec<GeometryShot>,
    /// GPS origin used for the transformation [lng, lat]
    pub origin: [f64; 2],
}

/// Error response for survey geometry command.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SurveyGeometryError {
    pub message: String,
}

/// Convert local meters (east, north) to GPS coordinates relative to origin.
///
/// Uses a simplified linear approximation for small distances (<100km):
/// - latitude_delta = north_meters / meters_per_degree_lat
/// - longitude_delta = east_meters / meters_per_degree_lng
///
/// where:
/// - meters_per_degree_lat ≈ 111320 (constant for WGS84)
/// - meters_per_degree_lng = 111320 * cos(origin_lat_rad)
///
/// # Errors
/// Returns an error if the origin latitude is too close to ±90° (beyond ±89.4°).
pub fn local_to_gps(
    east: f64,
    north: f64,
    origin: &GpsCoordinates,
) -> Result<(f64, f64), SurveyGeometryError> {
    const METERS_PER_DEGREE_LAT: f64 = 111_320.0;

    let origin_lat_rad = origin.latitude.to_radians();
    let cos_lat = origin_lat_rad.cos();

    // Guard against division by near-zero at extreme latitudes
    if cos_lat.abs() < MIN_COS_LAT {
        return Err(SurveyGeometryError {
            message: format!(
                "Origin latitude {:.2}° is too close to the poles for accurate coordinate conversion",
                origin.latitude
            ),
        });
    }

    let meters_per_degree_lng = METERS_PER_DEGREE_LAT * cos_lat;

    let lat = origin.latitude + (north / METERS_PER_DEGREE_LAT);
    let lng = origin.longitude + (east / meters_per_degree_lng);

    Ok((lng, lat)) // MapLibre order
}

/// Get survey geometry for a specific file path without loading into AppState.
///
/// This is used by GlobalView for lazy loading geometry from arbitrary projects.
/// Reads the file without writing to it, computes geometry, and drops it.
///
/// # Returns
/// - `Ok(SurveyGeometry)` with shots and origin
/// - `Err(SurveyGeometryError)` if file doesn't exist or has no GPS reference
#[tauri::command]
pub async fn get_survey_geometry_for_file(
    path: PathBuf,
) -> Result<SurveyGeometry, SurveyGeometryError> {
    // Use spawn_blocking for UI responsiveness
    tauri::async_runtime::spawn_blocking(move || get_survey_geometry_for_file_impl(&path))
        .await
        .map_err(|e| SurveyGeometryError {
            message: format!("Internal error: {e}"),
        })?
}

/// Internal implementation for file-based geometry loading.
fn get_survey_geometry_for_file_impl(path: &Path) -> Result<SurveyGeometry, SurveyGeometryError> {
    let project = read_survey(path).map_err(|message| SurveyGeometryError {
        message: format!("Failed to open project: {message}"),
    })?;
    get_survey_geometry_for_project(&project)
}

/// The survey in the file at `path`, read the way the load reads it and
/// never written: a version 2 file through the full decode (an invalid one is
/// refused, as the load refuses it), a legacy SQLite file through the frozen
/// 0.6.0 open and the conversion (in memory only).
fn read_survey(path: &Path) -> Result<Project, String> {
    if path.extension().and_then(|s| s.to_str()) != Some("stalx") {
        return Err(format!(
            "Not a Stalagmight project file: {}",
            path.display()
        ));
    }
    let options = DecodeOptions::default();
    let (bytes, _) = Fingerprint::read(path, options.max_file_bytes).map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("File does not exist: {}", path.display())
        } else {
            e.to_string()
        }
    })?;
    match stalx_v2::sniff(&bytes) {
        Format::V2 => stalx_v2::decode(&bytes, &options)
            .map(|decoded| decoded.project)
            .map_err(|e| e.to_string()),
        Format::Legacy => {
            drop(bytes);
            crate::persistence::migrate::migrate_path(path)
                .map(|migrated| migrated.project)
                .map_err(|e| e.to_string())
        }
        Format::Unknown => Err(format!(
            "Not a Stalagmight project file: {}",
            path.display()
        )),
    }
}

/// Compute survey geometry from a project struct.
/// Shared implementation for the file-based command and its tests.
///
/// Positions are the measured layout (solver v1, `Project::raw_positions`)
/// in local metres from the frame datum's GPS, which is exactly how the
/// workspace map places stations (`metersToMapCoords` from the first datum's
/// GPS), so a preview and the opened project agree.
fn get_survey_geometry_for_project(
    project: &Project,
) -> Result<SurveyGeometry, SurveyGeometryError> {
    let frame = project
        .frame_datum()
        .and_then(|index| project.datums.get(index))
        .ok_or_else(|| SurveyGeometryError {
            message: "Project has no GPS reference. Set a zero datum with GPS coordinates first."
                .to_string(),
        })?;
    let origin_gps = &frame.gps;
    let origin = [origin_gps.longitude, origin_gps.latitude];
    let positions = project.raw_positions();
    let at = |station: StationId| {
        positions.xy(station).ok_or_else(|| SurveyGeometryError {
            message: format!("Station {station} has no computed position"),
        })
    };

    let mut shots = Vec::new();
    for line in project.lines() {
        let mut from = line.start;
        for shot in &line.shots {
            let [from_east, from_north] = at(from)?;
            let [to_east, to_north] = at(shot.to)?;
            let from_gps = local_to_gps(from_east, from_north, origin_gps)?;
            let to_gps = local_to_gps(to_east, to_north, origin_gps)?;
            shots.push(GeometryShot {
                from: [from_gps.0, from_gps.1],
                to: [to_gps.0, to_gps.1],
                kind: shot.kind,
                line_id: line.id,
            });
            from = shot.to;
        }
    }

    Ok(SurveyGeometry { shots, origin })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::models::gps_to_local;
    use crate::models::survey::{LineMeta, StartAt, ZeroDatum};
    use crate::models::ShotData;
    use crate::persistence::stalx_v2::{Extras, Mode};
    use tempfile::TempDir;

    fn shot(length: f64, azimuth: f64, kind: ShotKind) -> ShotData {
        ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            depth_in: 0.0,
            depth_out: 0.0,
            kind,
            ..Default::default()
        }
    }

    /// One line of `shots` from a fresh station, placed (no datum) at the origin.
    fn one_line(name: &str, shots: Vec<ShotData>) -> (Project, LineId) {
        let mut project = Project::new(name, None);
        let line = project
            .add_line(LineMeta::default(), StartAt::New, shots)
            .unwrap();
        let start = project.line(line).unwrap().start;
        project.set_placement(start, 0.0, 0.0).unwrap();
        (project, line)
    }

    /// Replace `line`'s placement with a datum at its start station.
    fn tie_to_gps(
        project: &mut Project,
        line: LineId,
        gps: GpsCoordinates,
        correction: Option<[f64; 2]>,
    ) {
        let start = project.line(line).unwrap().start;
        project.placements.clear();
        project.push_datum(ZeroDatum {
            station: start,
            via: None,
            gps,
            accuracy_m: None,
            entrance: None,
            correction,
        });
        assert!(project.validate().is_empty(), "{:?}", project.validate());
    }

    fn write_v2(path: &Path, project: &Project) {
        let bytes = stalx_v2::encode(project, &Extras::default(), Mode::Normal).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    // Test vector data from coordinate_transform_vectors.json
    // These tests verify the local_to_gps function matches expected values

    #[test]
    fn test_local_to_gps_at_equator() {
        // At equator, 1 degree ≈ 111320 meters in both directions
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();

        // 1000m east at equator
        let (lng, lat) = local_to_gps(1000.0, 0.0, &origin).unwrap();
        let expected_lng_delta = 1000.0 / 111_320.0;
        assert!(
            (lng - expected_lng_delta).abs() < 1e-6,
            "lng: {lng} expected: {expected_lng_delta}"
        );
        assert!(lat.abs() < 1e-9, "lat should be ~0");

        // 1000m north at equator
        let (lng, lat) = local_to_gps(0.0, 1000.0, &origin).unwrap();
        let expected_lat_delta = 1000.0 / 111_320.0;
        assert!(lng.abs() < 1e-9, "lng should be ~0");
        assert!(
            (lat - expected_lat_delta).abs() < 1e-6,
            "lat: {lat} expected: {expected_lat_delta}"
        );
    }

    #[test]
    fn test_local_to_gps_at_45_degrees() {
        // At 45°N, longitude degrees are shorter by cos(45°) ≈ 0.7071
        let origin = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();

        // 1000m east at 45°N
        let (lng, lat) = local_to_gps(1000.0, 0.0, &origin).unwrap();
        let meters_per_deg_lng = 111_320.0 * (45.0_f64.to_radians().cos());
        let expected_lng = -90.0 + (1000.0 / meters_per_deg_lng);
        assert!(
            (lng - expected_lng).abs() < 1e-6,
            "lng: {lng} expected: {expected_lng}"
        );
        assert!((lat - 45.0).abs() < 1e-9, "lat should be ~45.0");

        // 1000m north at 45°N
        let (lng, lat) = local_to_gps(0.0, 1000.0, &origin).unwrap();
        let expected_lat = 45.0 + (1000.0 / 111_320.0);
        assert!((lng - (-90.0)).abs() < 1e-9, "lng should be ~-90.0");
        assert!(
            (lat - expected_lat).abs() < 1e-6,
            "lat: {lat} expected: {expected_lat}"
        );
    }

    #[test]
    fn test_local_to_gps_roundtrip() {
        // Test that gps_to_local and local_to_gps are approximate inverses
        let origin = GpsCoordinates::new(30.0, -80.0, 0.0).unwrap();
        let target = GpsCoordinates::new(30.01, -79.99, 0.0).unwrap();

        // Convert target GPS to local meters
        let (east, north) = gps_to_local(&target, &origin);

        // Convert back to GPS
        let (lng, lat) = local_to_gps(east, north, &origin).unwrap();

        // Should be close to original (within ~1m tolerance for small distances)
        assert!(
            (lat - target.latitude).abs() < 0.0001,
            "lat roundtrip error: {} vs {}",
            lat,
            target.latitude
        );
        assert!(
            (lng - target.longitude).abs() < 0.0001,
            "lng roundtrip error: {} vs {}",
            lng,
            target.longitude
        );
    }

    #[test]
    fn test_local_to_gps_diagonal() {
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();

        // 1000m NE (45 degrees)
        let east = 1000.0 * (45.0_f64.to_radians().sin());
        let north = 1000.0 * (45.0_f64.to_radians().cos());
        let (lng, lat) = local_to_gps(east, north, &origin).unwrap();

        let expected_lng = east / 111_320.0;
        let expected_lat = north / 111_320.0;

        assert!((lng - expected_lng).abs() < 1e-6);
        assert!((lat - expected_lat).abs() < 1e-6);
    }

    #[test]
    fn test_local_to_gps_negative_coords() {
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();

        // 1000m west (negative east)
        let (lng, lat) = local_to_gps(-1000.0, 0.0, &origin).unwrap();
        let expected_lng = -1000.0 / 111_320.0;
        assert!((lng - expected_lng).abs() < 1e-6);
        assert!(lat.abs() < 1e-9);

        // 1000m south (negative north)
        let (lng, lat) = local_to_gps(0.0, -1000.0, &origin).unwrap();
        let expected_lat = -1000.0 / 111_320.0;
        assert!(lng.abs() < 1e-9);
        assert!((lat - expected_lat).abs() < 1e-6);
    }

    #[test]
    fn test_local_to_gps_rejects_extreme_latitude() {
        // Near poles, cos(lat) approaches 0, causing division issues
        let near_pole = GpsCoordinates::new(89.5, 0.0, 0.0).unwrap();
        let result = local_to_gps(1000.0, 0.0, &near_pole);
        assert!(result.is_err(), "Should reject latitude near poles");
        assert!(
            result.unwrap_err().message.contains("poles"),
            "Error should mention poles"
        );

        // -89.5 should also fail
        let near_south_pole = GpsCoordinates::new(-89.5, 0.0, 0.0).unwrap();
        let result = local_to_gps(1000.0, 0.0, &near_south_pole);
        assert!(result.is_err(), "Should reject latitude near south pole");
    }

    #[test]
    fn test_local_to_gps_accepts_high_but_valid_latitude() {
        // 89° should still work (cos(89°) ≈ 0.0175 > MIN_COS_LAT)
        let high_lat = GpsCoordinates::new(89.0, 0.0, 0.0).unwrap();
        let result = local_to_gps(1000.0, 0.0, &high_lat);
        assert!(result.is_ok(), "89° latitude should be acceptable");
    }

    #[test]
    fn test_get_survey_geometry_requires_zero_datum() {
        let (project, _) = one_line("No Datum", vec![shot(10.0, 0.0, ShotKind::Normal)]);

        let result = get_survey_geometry_for_project(&project);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no GPS reference"));
    }

    /// The preview places stations where the workspace map does: local metres
    /// from the frame datum's GPS. A frame datum with a stored correction
    /// (a converted file whose datum piece sat off its GPS) puts its station
    /// at that offset, not on the GPS.
    #[test]
    fn test_get_survey_geometry_keeps_the_frame_datums_correction() {
        let (mut project, line) = one_line("Corrected", vec![shot(10.0, 90.0, ShotKind::Normal)]);
        let gps = GpsCoordinates::new(20.0, -87.0, 0.0).unwrap();
        tie_to_gps(&mut project, line, gps, Some([3.0, 4.0]));

        let geometry = get_survey_geometry_for_project(&project).unwrap();

        assert_eq!(geometry.origin, [-87.0, 20.0]);
        let (lng, lat) = local_to_gps(3.0, 4.0, &gps).unwrap();
        assert!((geometry.shots[0].from[0] - lng).abs() < 1e-9);
        assert!((geometry.shots[0].from[1] - lat).abs() < 1e-9);
        let (lng, lat) = local_to_gps(13.0, 4.0, &gps).unwrap();
        assert!((geometry.shots[0].to[0] - lng).abs() < 1e-9);
        assert!((geometry.shots[0].to[1] - lat).abs() < 1e-9);
    }

    #[test]
    fn test_get_survey_geometry_includes_reference_shots_and_origin() {
        let (mut project, line_id) = one_line("Geometry", vec![shot(10.0, 0.0, ShotKind::Jump)]);
        let gps = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        tie_to_gps(&mut project, line_id, gps, None);

        let geometry = get_survey_geometry_for_project(&project).unwrap();
        assert_eq!(geometry.origin, [0.0, 0.0]);
        assert_eq!(geometry.shots.len(), 1);

        let geom_shot = &geometry.shots[0];
        assert_eq!(geom_shot.kind, ShotKind::Jump);
        assert_eq!(geom_shot.line_id, line_id);
        assert!((geom_shot.from[0]).abs() < 1e-9);
        assert!((geom_shot.from[1]).abs() < 1e-9);

        // 10 m due north, level.
        let (lng, lat) = local_to_gps(0.0, 10.0, &gps).unwrap();
        assert!((geom_shot.to[0] - lng).abs() < 1e-6);
        assert!((geom_shot.to[1] - lat).abs() < 1e-6);
    }

    /// Every shot of every line, from the station before it to its `to`, a
    /// junction shared by both lines.
    #[test]
    fn test_get_survey_geometry_walks_every_line_through_a_junction() {
        let (mut project, main) = one_line(
            "Junction",
            vec![
                shot(10.0, 0.0, ShotKind::Normal),
                shot(10.0, 0.0, ShotKind::Normal),
            ],
        );
        let junction = project.line(main).unwrap().station(1).unwrap();
        let branch = project
            .add_line(
                LineMeta::default(),
                StartAt::Station(junction),
                vec![shot(5.0, 90.0, ShotKind::Reference)],
            )
            .unwrap();
        let gps = GpsCoordinates::new(10.0, 10.0, 0.0).unwrap();
        tie_to_gps(&mut project, main, gps, None);

        let geometry = get_survey_geometry_for_project(&project).unwrap();

        assert_eq!(geometry.shots.len(), 3);
        let on_branch = geometry.shots.iter().find(|s| s.line_id == branch).unwrap();
        assert_eq!(on_branch.kind, ShotKind::Reference);
        assert_eq!(
            on_branch.from, geometry.shots[0].to,
            "the junction is one point"
        );
    }

    #[test]
    fn test_get_survey_geometry_for_file_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let missing = temp_dir.path().join("missing.stalx");

        let result = get_survey_geometry_for_file_impl(&missing);
        assert!(result.is_err());
        let message = result.unwrap_err().message;
        assert!(message.contains("Failed to open project"));
        assert!(message.contains("does not exist"));
    }

    #[test]
    fn test_get_survey_geometry_for_file_rejects_corrupted_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("corrupted.stalx");
        std::fs::write(&path, b"not a sqlite database").unwrap();

        let result = get_survey_geometry_for_file_impl(&path);
        assert!(result.is_err());
        let message = result.unwrap_err().message;
        assert!(message.contains("Failed to open project"), "{message}");
        assert!(
            message.contains("Not a Stalagmight project file"),
            "{message}"
        );
    }

    /// A version 2 file the load would refuse (here: a piece without an
    /// anchor) is refused here too, and never written.
    #[test]
    fn test_get_survey_geometry_for_file_rejects_an_invalid_v2_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("invalid.stalx");
        let (project, _) = one_line("Invalid", vec![shot(10.0, 0.0, ShotKind::Normal)]);
        let bytes = stalx_v2::encode(&project, &Extras::default(), Mode::Normal).unwrap();
        let text = String::from_utf8(bytes).unwrap();
        let start = text.find("\"placements\":[").unwrap();
        let end = start + text[start..].find("],\n").unwrap() + 3;
        let broken = format!("{}{}", &text[..start], &text[end..]);
        std::fs::write(&path, &broken).unwrap();

        let message = get_survey_geometry_for_file_impl(&path)
            .unwrap_err()
            .message;

        assert!(message.starts_with("Failed to open project"), "{message}");
        assert!(message.contains("inconsistent"), "{message}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
    }

    #[test]
    fn test_get_survey_geometry_for_file_rejects_wrong_extension() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("not-a-project.txt");
        std::fs::write(&path, "nope").unwrap();

        let result = get_survey_geometry_for_file_impl(&path);
        assert!(result.is_err());
        let message = result.unwrap_err().message;
        assert!(message.contains("Failed to open project"));
        assert!(message.contains("Not a Stalagmight project file"));
    }

    #[test]
    fn test_get_survey_geometry_for_file_requires_zero_datum() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("no-datum.stalx");
        let (project, _) = one_line("No Datum", vec![shot(10.0, 0.0, ShotKind::Normal)]);
        write_v2(&path, &project);

        let result = get_survey_geometry_for_file_impl(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no GPS reference"));
    }

    /// A version 2 file with a datum: one shot per shot, from the file.
    #[test]
    fn test_get_survey_geometry_for_file_reads_a_v2_file() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("v2.stalx");
        let (mut project, line) = one_line(
            "V2",
            vec![
                shot(10.0, 0.0, ShotKind::Normal),
                shot(4.0, 180.0, ShotKind::Normal),
            ],
        );
        tie_to_gps(
            &mut project,
            line,
            GpsCoordinates::new(1.0, 2.0, 0.0).unwrap(),
            None,
        );
        write_v2(&path, &project);
        let before = std::fs::read(&path).unwrap();

        let geometry = get_survey_geometry_for_file_impl(&path).unwrap();

        assert_eq!(geometry.origin, [2.0, 1.0]);
        assert_eq!(geometry.shots.len(), 2);
        assert_eq!(geometry, get_survey_geometry_for_project(&project).unwrap());
        assert_eq!(std::fs::read(&path).unwrap(), before, "only read");
    }

    /// A legacy SQLite file is converted in memory (the original is only
    /// read) and previewed like any other.
    #[test]
    fn test_get_survey_geometry_for_file_reads_a_legacy_file() {
        use stalagmight_legacy::models as old;
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("legacy.stalx");
        let mut legacy = stalagmight_legacy::LegacyProject::new("Legacy".to_string(), None);
        let line = legacy
            .add_line(old::LineData {
                shots: vec![old::ShotData {
                    length: 10.0,
                    azimuth_in: 90.0,
                    azimuth_out: 90.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;
        let gps = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        legacy
            .set_zero_datum(
                old::StationRef::new(line, 0),
                old::GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
            )
            .unwrap();
        let bytes =
            stalagmight_legacy::write_v18(&legacy, &stalagmight_legacy::LegacyExtras::default())
                .unwrap();
        std::fs::write(&path, bytes).unwrap();
        let before = std::fs::read(&path).unwrap();

        let geometry = get_survey_geometry_for_file_impl(&path).unwrap();

        assert_eq!(geometry.origin, [0.0, 0.0]);
        assert_eq!(geometry.shots.len(), 1);
        let (lng, lat) = local_to_gps(10.0, 0.0, &gps).unwrap();
        assert!((geometry.shots[0].to[0] - lng).abs() < 1e-6);
        assert!((geometry.shots[0].to[1] - lat).abs() < 1e-6);
        assert_eq!(std::fs::read(&path).unwrap(), before, "only read");
    }
}
