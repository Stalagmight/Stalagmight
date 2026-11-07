//! Survey geometry command for GPS-transformed survey data.
//!
//! Provides `get_survey_geometry_for_file`, which returns shot data with
//! GPS-transformed coordinates for lazy loading geometry from arbitrary
//! project files (used by GlobalView for on-demand loading).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::core::CommandError;
use crate::models::{GpsCoordinates, Project, SectionId, ShotKind, StationRef};
use crate::persistence::Database;

/// A self-contained shot with GPS coordinates and metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct GeometryShot {
    /// GPS coordinates of the from station [lng, lat] (MapLibre order)
    pub from: [f64; 2],
    /// GPS coordinates of the to station [lng, lat] (MapLibre order)
    pub to: [f64; 2],
    /// Permanent, jump, or reference (#27). The map styles jumps and
    /// references differently, so the full kind crosses the IPC boundary, not
    /// just a virtual-vs-not bool.
    pub kind: ShotKind,
    /// Section ID this shot belongs to (for coloring/filtering)
    pub section_id: SectionId,
}

/// Survey geometry response containing GPS-transformed shot data.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
pub struct SurveyGeometry {
    /// Array of shots with GPS coordinates
    pub shots: Vec<GeometryShot>,
    /// GPS origin used for the transformation [lng, lat]
    pub origin: [f64; 2],
}

/// Why the survey geometry could not be computed. Internal since #576 C4: the
/// command rejects with a [`CommandError`] (`FAILED`) carrying this message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurveyGeometryError {
    pub message: String,
}

/// Convert local meters (east, north) to GPS coordinates relative to origin, returned in
/// MapLibre `(lng, lat)` order, ready to draw.
///
/// The exact WGS84 inverse of the projection that placed the stations
/// ([`crate::models::local_to_gps`] undoes `gps_to_local`). This used a flat
/// 111 320 m-per-degree model, which put GlobalView's drawing of a project metres away from
/// the workspace map's (#576 D36); the map runs the same inverse in TypeScript.
///
/// GlobalView draws lines from the origin, so the longitude goes through the explicit
/// [`crate::models::unwrap_longitude`] step: continuous with the origin's across the
/// antimeridian, possibly outside [-180, 180], exactly as the workspace map's
/// `metersToMapCoords` draws it.
///
/// An origin beside a pole is drawn like any other (#576 W30-1): the refusal past about
/// +-89.4 degrees guarded the old flat model's divide by cos(lat), which the exact solve does not
/// have, and the workspace map always drew such an origin.
///
/// # Errors
/// Returns an error if the point is beyond the horizon (no point on the earth has that offset
/// from the origin, about an earth radius out): GlobalView then shows the project as failed to
/// load, with this message, rather than draw a station somewhere it is not.
pub fn local_to_gps(
    east: f64,
    north: f64,
    origin: &GpsCoordinates,
) -> Result<(f64, f64), SurveyGeometryError> {
    let (lat, lng) =
        crate::models::local_to_gps(east, north, origin).ok_or_else(|| SurveyGeometryError {
            message: format!(
                "A station {:.0} km from the zero datum is beyond the horizon and cannot be drawn",
                east.hypot(north) / 1000.0
            ),
        })?;
    let lng = crate::models::unwrap_longitude(lng, origin.longitude);
    Ok((lng, lat)) // MapLibre order
}

/// Get survey geometry for a specific file path without loading into AppState.
///
/// This is used by GlobalView for lazy loading geometry from arbitrary projects.
/// Opens the database read-only, computes geometry, and closes it.
///
/// # Returns
/// - `Ok(SurveyGeometry)` with shots and origin
/// - `Err(CommandError)` (`FAILED`) if file doesn't exist or has no GPS reference
#[tauri::command]
pub async fn get_survey_geometry_for_file(path: PathBuf) -> Result<SurveyGeometry, CommandError> {
    // Use spawn_blocking for UI responsiveness
    tauri::async_runtime::spawn_blocking(move || get_survey_geometry_for_file_impl(&path))
        .await
        .map_err(|e| CommandError::internal(format!("Internal error: {e}")))?
        .map_err(|e| CommandError::failed(e.message))
}

/// Internal implementation for file-based geometry loading.
fn get_survey_geometry_for_file_impl(path: &Path) -> Result<SurveyGeometry, SurveyGeometryError> {
    // Open database in read-only mode - never create or modify files
    let db = Database::open_readonly(path).map_err(|e| SurveyGeometryError {
        message: format!("Failed to open project: {e}"),
    })?;

    // Load project from database
    let mut project = db.load_project().map_err(|e| SurveyGeometryError {
        message: format!("Failed to load project: {e}"),
    })?;

    // Assign IDs to any legacy closures with id=0 (in-memory only, read-only DB)
    project.assign_closure_ids();
    // Section offsets are derived: re-solve them so the GlobalView preview renders
    // a drifted file the same way the workspace will after it redrives on open
    // (otherwise the two views can disagree). In-memory only - the DB is read-only.
    project.redrive_offsets_after_load();

    // Compute geometry from project
    get_survey_geometry_for_project(&project)
}

/// Compute survey geometry from a project struct.
/// Shared implementation for the file-based command and its tests.
fn get_survey_geometry_for_project(
    project: &Project,
) -> Result<SurveyGeometry, SurveyGeometryError> {
    // Find zero datum for GPS origin
    let zero_datum = project
        .zero_datums
        .first()
        .ok_or_else(|| SurveyGeometryError {
            message: "Project has no GPS reference. Set a zero datum with GPS coordinates first."
                .to_string(),
        })?;

    let origin_gps = &zero_datum.gps;
    let origin = [origin_gps.longitude, origin_gps.latitude];

    // Compute station coordinates
    let stations = project.compute_station_coordinates();

    // Build a map for quick station lookup
    let station_map: std::collections::HashMap<StationRef, &crate::models::Station> =
        stations.iter().map(|s| (s.station_ref, s)).collect();

    // Find the zero-datum station's local coordinates (x0, y0)
    // All other coordinates are relative to this point. Resolve the stored
    // anchor to its current positional ref for the lookup into station_map.
    let datum_ref = project
        .resolve_key(&zero_datum.station_ref)
        .ok_or_else(|| SurveyGeometryError {
            message: format!(
                "Zero datum station {} anchor no longer resolves",
                zero_datum.station_ref
            ),
        })?;
    let datum_station = station_map
        .get(&datum_ref)
        .ok_or_else(|| SurveyGeometryError {
            message: format!(
                "Zero datum station {} not found in computed stations",
                zero_datum.station_ref
            ),
        })?;
    let x0 = datum_station.coordinates.x;
    let y0 = datum_station.coordinates.y;

    let mut shots = Vec::new();

    // Iterate through sections and shots
    for section in &project.sections {
        for (shot_idx, shot) in section.shots.iter().enumerate() {
            let from_ref = StationRef::new(section.id, shot_idx as u32);
            let to_ref = StationRef::new(section.id, (shot_idx + 1) as u32);

            let from_station = station_map
                .get(&from_ref)
                .ok_or_else(|| SurveyGeometryError {
                    message: format!("Station {from_ref} not found in computed stations"),
                })?;
            let to_station = station_map
                .get(&to_ref)
                .ok_or_else(|| SurveyGeometryError {
                    message: format!("Station {to_ref} not found in computed stations"),
                })?;

            // Compute coordinates relative to zero-datum station (x0, y0)
            let from_east = from_station.coordinates.x - x0;
            let from_north = from_station.coordinates.y - y0;
            let to_east = to_station.coordinates.x - x0;
            let to_north = to_station.coordinates.y - y0;

            // Convert local coordinates to GPS
            let from_gps = local_to_gps(from_east, from_north, origin_gps)?;
            let to_gps = local_to_gps(to_east, to_north, origin_gps)?;

            shots.push(GeometryShot {
                from: [from_gps.0, from_gps.1],
                to: [to_gps.0, to_gps.1],
                kind: shot.kind,
                section_id: section.id,
            });
        }
    }

    Ok(SurveyGeometry { shots, origin })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::models::gps_to_local;
    use crate::models::{Project, SectionData, ShotData, StationRef, ZeroDatum};
    use crate::persistence::Database;
    use tempfile::TempDir;

    /// Degrees per metre north and east at `lat0` on WGS84: 1 / the meridional radius of
    /// curvature, and 1 / (the prime-vertical radius * cos lat). The local scale the exact
    /// inverse must reproduce over a kilometre (to ~1e-8 deg; the asserts allow 1e-6).
    fn wgs84_deg_per_m(lat0: f64) -> (f64, f64) {
        const A: f64 = 6_378_137.0;
        const E2: f64 = 0.006_694_379_990_14;
        let s = lat0.to_radians().sin();
        let w = 1.0 - E2 * s * s;
        let meridional = A * (1.0 - E2) / (w * w.sqrt());
        let prime_vertical = A / w.sqrt();
        (
            (1.0 / meridional).to_degrees(),
            (1.0 / (prime_vertical * lat0.to_radians().cos())).to_degrees(),
        )
    }

    #[test]
    fn test_local_to_gps_at_equator() {
        // On the ellipsoid a degree of latitude is SHORTER at the equator (110 574 m) than a
        // degree of longitude (111 319 m); the flat model used 111 320 m for both.
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        let (lat_per_m, lng_per_m) = wgs84_deg_per_m(0.0);

        // 1000m east at equator
        let (lng, lat) = local_to_gps(1000.0, 0.0, &origin).unwrap();
        let expected_lng_delta = 1000.0 * lng_per_m;
        assert!(
            (lng - expected_lng_delta).abs() < 1e-6,
            "lng: {lng} expected: {expected_lng_delta}"
        );
        assert!(lat.abs() < 1e-9, "lat should be ~0");

        // 1000m north at equator
        let (lng, lat) = local_to_gps(0.0, 1000.0, &origin).unwrap();
        let expected_lat_delta = 1000.0 * lat_per_m;
        assert!(lng.abs() < 1e-9, "lng should be ~0");
        assert!(
            (lat - expected_lat_delta).abs() < 1e-6,
            "lat: {lat} expected: {expected_lat_delta}"
        );
    }

    #[test]
    fn test_local_to_gps_at_45_degrees() {
        // At 45°N, longitude degrees are shorter by about cos(45°) ≈ 0.7071
        let origin = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
        let (lat_per_m, lng_per_m) = wgs84_deg_per_m(45.0);

        // 1000m east at 45°N
        let (lng, lat) = local_to_gps(1000.0, 0.0, &origin).unwrap();
        let expected_lng = -90.0 + 1000.0 * lng_per_m;
        assert!(
            (lng - expected_lng).abs() < 1e-6,
            "lng: {lng} expected: {expected_lng}"
        );
        // Due east on the origin's tangent plane is a normal section, not the parallel: it bends
        // toward the equator by d^2 tan(lat) / (2 N M) - 7.04e-7 deg (8 cm) over a kilometre.
        let w = 1.0 - 0.006_694_379_990_14 * 0.5; // 1 - e^2 sin^2(45)
        let n_m = 6_378_137.0_f64.powi(2) * (1.0 - 0.006_694_379_990_14) / (w * w);
        let expected_lat = 45.0 - (1.0e6 / (2.0 * n_m)).to_degrees();
        assert!(
            (lat - expected_lat).abs() < 1e-10,
            "lat: {lat} expected: {expected_lat}"
        );

        // 1000m north at 45°N
        let (lng, lat) = local_to_gps(0.0, 1000.0, &origin).unwrap();
        let expected_lat = 45.0 + 1000.0 * lat_per_m;
        assert!((lng - (-90.0)).abs() < 1e-9, "lng should be ~-90.0");
        assert!(
            (lat - expected_lat).abs() < 1e-6,
            "lat: {lat} expected: {expected_lat}"
        );
    }

    #[test]
    fn test_local_to_gps_roundtrip() {
        // local_to_gps is the exact inverse of gps_to_local (#576 D36), not an approximation
        let origin = GpsCoordinates::new(30.0, -80.0, 0.0).unwrap();
        let target = GpsCoordinates::new(30.01, -79.99, 0.0).unwrap();

        // Convert target GPS to local meters
        let (east, north) = gps_to_local(&target, &origin);

        // Convert back to GPS
        let (lng, lat) = local_to_gps(east, north, &origin).unwrap();

        assert!(
            (lat - target.latitude).abs() < 1e-9,
            "lat roundtrip error: {} vs {}",
            lat,
            target.latitude
        );
        assert!(
            (lng - target.longitude).abs() < 1e-9,
            "lng roundtrip error: {} vs {}",
            lng,
            target.longitude
        );
    }

    #[test]
    fn test_local_to_gps_diagonal() {
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        let (lat_per_m, lng_per_m) = wgs84_deg_per_m(0.0);

        // 1000m NE (45 degrees)
        let east = 1000.0 * (45.0_f64.to_radians().sin());
        let north = 1000.0 * (45.0_f64.to_radians().cos());
        let (lng, lat) = local_to_gps(east, north, &origin).unwrap();

        let expected_lng = east * lng_per_m;
        let expected_lat = north * lat_per_m;

        assert!((lng - expected_lng).abs() < 1e-6);
        assert!((lat - expected_lat).abs() < 1e-6);
    }

    #[test]
    fn test_local_to_gps_negative_coords() {
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        let (lat_per_m, lng_per_m) = wgs84_deg_per_m(0.0);

        // 1000m west (negative east)
        let (lng, lat) = local_to_gps(-1000.0, 0.0, &origin).unwrap();
        let expected_lng = -1000.0 * lng_per_m;
        assert!((lng - expected_lng).abs() < 1e-6);
        assert!(lat.abs() < 1e-9);

        // 1000m south (negative north)
        let (lng, lat) = local_to_gps(0.0, -1000.0, &origin).unwrap();
        let expected_lat = -1000.0 * lat_per_m;
        assert!(lng.abs() < 1e-9);
        assert!((lat - expected_lat).abs() < 1e-6);
    }

    /// `[origin lat, origin lon, target lat, target lon, east m, north m]`: east/north are what
    /// `gps_to_local` (WGS84) returns for the target, i.e. where the backend PLACES a zero datum
    /// typed with that fix. The same table is asserted against the map's conversion in
    /// src/utils/__tests__/coordinates.test.ts (#576 D36).
    const BACKEND_VECTORS: [[f64; 6]; 10] = [
        [30.0, 0.0, 30.009, 0.0, 0.0, 997.672_660_953_443_4],
        [
            30.0,
            0.0,
            30.0,
            0.009,
            868.376_518_687_013_1,
            0.034_101_066_179_573_53,
        ],
        [0.0, 0.0, 0.009, 0.0, 0.0, 995.168_478_384_073_8],
        [0.0, 0.0, 0.0, 0.009, 1_001.875_413_019_414_5, 0.0],
        [
            45.0,
            -90.0,
            45.05,
            -90.05,
            -3_938.910_955_978_453,
            5_557.827_879_505_819,
        ],
        [
            -30.0,
            151.0,
            -30.18,
            151.18,
            17_336.073_485_110_51,
            -19_967.295_913_146_296,
        ],
        [
            60.0,
            25.0,
            60.1,
            24.9,
            -5_563.148_767_087_514,
            11_145.512_358_657_026,
        ],
        [
            20.5,
            -87.3,
            20.45,
            -87.2,
            10_434.644_415_801_886,
            -5_532.324_088_892_108,
        ],
        [
            70.0,
            0.0,
            70.02,
            0.3,
            11_444.931_185_911_73,
            2_259.398_833_161_237_2,
        ],
        [
            -54.8,
            -68.3,
            -54.9,
            -68.1,
            12_830.611_701_495_578,
            -11_150.369_721_606_98,
        ],
    ];

    /// #576 D36: GlobalView drew a project through a flat 111 320 m/deg model while the backend
    /// places datums on the WGS84 ellipsoid (and the map inverted through a sphere), so the same
    /// station sat metres apart in the two views. `local_to_gps` must invert `gps_to_local`.
    #[test]
    fn local_to_gps_lands_a_station_on_the_fix_the_backend_placed_it_from() {
        let mut failures = Vec::new();
        for [olat, olon, lat, lon, east, north] in BACKEND_VECTORS {
            let origin = GpsCoordinates::new(olat, olon, 0.0).unwrap();
            let target = GpsCoordinates::new(lat, lon, 0.0).unwrap();
            // The table itself is the backend's forward projection.
            let (e, n) = gps_to_local(&target, &origin);
            assert!((e - east).abs() < 1e-6 && (n - north).abs() < 1e-6);

            let (lng2, lat2) = local_to_gps(east, north, &origin).unwrap();
            let off_north = (lat2 - lat) * 111_132.0;
            let off_east = (lng2 - lon) * 111_320.0 * lat.to_radians().cos();
            if (lat2 - lat).abs() > 1e-9 || (lng2 - lon).abs() > 1e-9 {
                failures.push(format!(
                    "origin ({olat}, {olon}): station at the backend position of ({lat}, {lon}) \
                     drawn at ({lat2:.7}, {lng2:.7}), {:.3} m off",
                    off_north.hypot(off_east)
                ));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    /// #576 W30-1: an origin beside a pole is drawn, where the workspace map draws it. The
    /// refusal past +-89.4 degrees guarded the old flat model's divide by cos(lat); the exact
    /// solve (GEO-D1) has no such divide, so GlobalView now places the station exactly where
    /// `models::local_to_gps` (and the frontend's `enuToGps`) does.
    #[test]
    fn globalview_draws_an_origin_beside_the_pole() {
        for (lat0, lon0) in [(89.9, 12.5), (-89.9, -45.0), (89.5, 0.0), (90.0, 0.0)] {
            let origin = GpsCoordinates::new(lat0, lon0, 0.0).unwrap();
            for (east, north) in [(1_000.0, 0.0), (0.0, 1_000.0), (-700.0, -700.0)] {
                let drawn = local_to_gps(east, north, &origin);
                let (fix_lat, fix_lng) = crate::models::local_to_gps(east, north, &origin).unwrap();
                let expected = (crate::models::unwrap_longitude(fix_lng, lon0), fix_lat);
                assert_eq!(
                    drawn.as_ref().map_err(|e| e.message.clone()),
                    Ok(&expected),
                    "origin ({lat0}, {lon0}), offset ({east}, {north}) m"
                );
            }
        }
    }

    #[test]
    fn test_local_to_gps_accepts_high_but_valid_latitude() {
        let high_lat = GpsCoordinates::new(89.0, 0.0, 0.0).unwrap();
        let result = local_to_gps(1000.0, 0.0, &high_lat);
        assert!(result.is_ok(), "89° latitude should be acceptable");
    }

    /// GEO-D1: `models::local_to_gps` returns a fix in [-180, 180]; GlobalView draws lines, so
    /// it unwraps the longitude explicitly. 1000 m east of 179.999 is drawn at 180.0084, next to
    /// the origin, not at -179.9916 on the far side of the globe; the mirror case too.
    #[test]
    fn globalview_draws_a_survey_across_the_antimeridian_in_one_piece() {
        for (lon0, east) in [(179.999, 1_000.0), (-179.999, -1_000.0)] {
            let origin = GpsCoordinates::new(-16.8, lon0, 0.0).unwrap();
            let (lng, lat) = local_to_gps(east, 0.0, &origin).unwrap();
            assert!(
                (lng - lon0).abs() < 0.01,
                "origin lng {lon0}, {east} m east: drawn at lng {lng}, across the globe"
            );
            let (fix_lat, fix_lng) = crate::models::local_to_gps(east, 0.0, &origin).unwrap();
            assert_eq!(lat, fix_lat);
            assert!(((lng - fix_lng).abs() - 360.0).abs() < 1e-9);
        }
    }

    /// GEO-D1: beyond the horizon there is no point to draw; GlobalView reports the project as
    /// failed with a message instead of drawing the station somewhere it is not.
    #[test]
    fn globalview_refuses_a_station_beyond_the_horizon() {
        let origin = GpsCoordinates::new(20.5, -87.4, 0.0).unwrap();
        let err = local_to_gps(20_000_000.0, 0.0, &origin).unwrap_err();
        assert!(
            err.message.contains("beyond the horizon"),
            "{}",
            err.message
        );
        assert!(err.message.contains("20000 km"), "{}", err.message);
    }

    #[test]
    fn test_get_survey_geometry_requires_zero_datum() {
        let mut project = Project::new("No Datum".to_string(), None);
        project.add_section(SectionData {
            shots: vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        let result = get_survey_geometry_for_project(&project);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no GPS reference"));
    }

    #[test]
    fn test_get_survey_geometry_errors_for_missing_datum_station() {
        let mut project = Project::new("Missing Station".to_string(), None);
        let section_id = project
            .add_section(SectionData {
                shots: vec![ShotData {
                    azimuth_in: 0.0,
                    azimuth_out: 0.0,
                    length: 5.0,
                    depth_in: 0.0,
                    depth_out: 0.0,
                    ..Default::default()
                }],
                ..Default::default()
            })
            .id;

        project.zero_datums.push(ZeroDatum {
            station_ref: StationRef::new(section_id, 3).into(),
            gps: GpsCoordinates::new(0.0, 0.0, 0.0).unwrap(),
        });

        let result = get_survey_geometry_for_project(&project);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("Zero datum station"));
    }

    #[test]
    fn test_get_survey_geometry_includes_reference_shots_and_origin() {
        let mut project = Project::new("Geometry".to_string(), None);
        let shot = ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length: 10.0,
            depth_in: 0.0,
            depth_out: 0.0,
            kind: ShotKind::Jump,
            ..Default::default()
        };
        let section_id = project
            .add_section(SectionData {
                shots: vec![shot.clone()],
                ..Default::default()
            })
            .id;

        let gps = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        project
            .set_zero_datum(StationRef::new(section_id, 0), gps)
            .unwrap();

        let geometry = get_survey_geometry_for_project(&project).unwrap();
        assert_eq!(geometry.origin, [0.0, 0.0]);
        assert_eq!(geometry.shots.len(), 1);

        let geom_shot = &geometry.shots[0];
        assert_eq!(geom_shot.kind, ShotKind::Jump);
        assert_eq!(geom_shot.section_id, section_id);
        assert!((geom_shot.from[0]).abs() < 1e-9);
        assert!((geom_shot.from[1]).abs() < 1e-9);

        let (east, north) =
            Project::shot_displacement(&shot, crate::models::AzimuthReference::default());
        let (lng, lat) = local_to_gps(east, north, &gps).unwrap();
        assert!((geom_shot.to[0] - lng).abs() < 1e-6);
        assert!((geom_shot.to[1] - lat).abs() < 1e-6);
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
    fn test_get_survey_geometry_for_file_rejects_corrupted_database() {
        let temp_dir = TempDir::new().unwrap();
        let path = temp_dir.path().join("corrupted.stalx");
        std::fs::write(&path, b"not a sqlite database").unwrap();

        let result = get_survey_geometry_for_file_impl(&path);
        assert!(result.is_err());
        let message = result.unwrap_err().message;
        assert!(message.contains("Failed to open project"));
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

        let mut project = Project::new("No Datum".to_string(), None);
        project.add_section(SectionData {
            shots: vec![ShotData {
                azimuth_in: 0.0,
                azimuth_out: 0.0,
                length: 10.0,
                depth_in: 0.0,
                depth_out: 0.0,
                ..Default::default()
            }],
            ..Default::default()
        });

        let mut db = Database::open(&path).unwrap();
        db.save_project(&project).unwrap();
        drop(db);

        let result = get_survey_geometry_for_file_impl(&path);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("no GPS reference"));
    }
}
