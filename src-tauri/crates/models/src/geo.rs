//! GPS-to-local coordinate conversion using ENU (East-North-Up) projection.
//!
//! Converts WGS84 GPS coordinates to local meters relative to a reference point.
//! Uses the WGS84 ellipsoid for accurate geodetic to ECEF conversion.

use super::GpsCoordinates;

/// WGS84 ellipsoid semi-major axis (equatorial radius) in meters
const WGS84_A: f64 = 6_378_137.0;

/// WGS84 ellipsoid eccentricity squared
/// e^2 = (a^2 - b^2) / a^2 where b = 6_356_752.314245 (polar radius)
const WGS84_E2: f64 = 0.006_694_379_990_14;

/// Convert geodetic coordinates (lat, lon, alt) to ECEF (Earth-Centered, Earth-Fixed).
///
/// # Arguments
/// * `lat` - Latitude in degrees
/// * `lon` - Longitude in degrees
/// * `alt` - Altitude above ellipsoid in meters (always use 0 for cave survey)
///
/// # Returns
/// (x, y, z) in meters in ECEF coordinate system
pub fn geodetic_to_ecef(lat: f64, lon: f64, alt: f64) -> (f64, f64, f64) {
    let lat_rad = lat.to_radians();
    let lon_rad = lon.to_radians();

    let sin_lat = lat_rad.sin();
    let cos_lat = lat_rad.cos();
    let sin_lon = lon_rad.sin();
    let cos_lon = lon_rad.cos();

    // N = radius of curvature in the prime vertical
    let n = WGS84_A / (1.0 - WGS84_E2 * sin_lat * sin_lat).sqrt();

    let x = (n + alt) * cos_lat * cos_lon;
    let y = (n + alt) * cos_lat * sin_lon;
    let z = (n * (1.0 - WGS84_E2) + alt) * sin_lat;

    (x, y, z)
}

/// Convert ECEF difference to ENU (East-North-Up) coordinates.
///
/// # Arguments
/// * `dx`, `dy`, `dz` - Difference in ECEF coordinates (target - origin)
/// * `lat0` - Origin latitude in degrees
/// * `lon0` - Origin longitude in degrees
///
/// # Returns
/// (east, north, up) in meters relative to origin
fn ecef_to_enu(dx: f64, dy: f64, dz: f64, lat0: f64, lon0: f64) -> (f64, f64, f64) {
    let lat_rad = lat0.to_radians();
    let lon_rad = lon0.to_radians();

    let sin_lat = lat_rad.sin();
    let cos_lat = lat_rad.cos();
    let sin_lon = lon_rad.sin();
    let cos_lon = lon_rad.cos();

    // Rotation matrix from ECEF to ENU
    let east = -sin_lon * dx + cos_lon * dy;
    let north = -sin_lat * cos_lon * dx - sin_lat * sin_lon * dy + cos_lat * dz;
    let up = cos_lat * cos_lon * dx + cos_lat * sin_lon * dy + sin_lat * dz;

    (east, north, up)
}

/// Convert GPS coordinates to local ENU coordinates relative to an origin.
///
/// Returns (east, north) offset in meters. The "Up" component is ignored since
/// cave surveys use depth measurements instead of GPS altitude.
///
/// # Arguments
/// * `gps` - Target GPS coordinates
/// * `origin` - Origin GPS coordinates (reference point)
///
/// # Returns
/// (east, north) offset in meters where:
/// - East is positive towards the east (local x-axis)
/// - North is positive towards the north (local y-axis)
///
/// # Important
/// Always uses altitude=0 for both points. The `GpsCoordinates.depth_reference`
/// field is NOT ellipsoid height - it represents meters below surface and must
/// not be used as altitude in the conversion. `Project.surface_elevation`
/// (#431, the entrance elevation above sea level) is likewise never fed into
/// this conversion: geodesy here is purely horizontal (east/north), and
/// station depth is tracked separately as a gauge reading against a single
/// project-wide surface, not as ellipsoid height.
pub fn gps_to_local(gps: &GpsCoordinates, origin: &GpsCoordinates) -> (f64, f64) {
    // Always use alt=0 - depth_reference is not ellipsoid height, and
    // surface_elevation (#431) is project metadata, not per-station altitude
    let (x0, y0, z0) = geodetic_to_ecef(origin.latitude, origin.longitude, 0.0);
    let (x1, y1, z1) = geodetic_to_ecef(gps.latitude, gps.longitude, 0.0);

    let dx = x1 - x0;
    let dy = y1 - y0;
    let dz = z1 - z0;

    let (east, north, _up) = ecef_to_enu(dx, dy, dz, origin.latitude, origin.longitude);

    (east, north)
}

/// The inverse of [`gps_to_local`]: the point ON the WGS84 ellipsoid whose local
/// `(east, north)` from `origin` are the given metres, as `(latitude, longitude)` in degrees,
/// longitude in [-180, 180]. The one local-to-GPS algorithm (#576 GEO-D1): the frontend's
/// `enuToGps` (src/utils/coordinates.ts) runs it step for step, so the workspace map, the data
/// table's GPS column and GlobalView place a station identically (#576 D36), and the stalx
/// reader reports it.
///
/// [`gps_to_local`] drops the "up" component, so the answer is where the origin's vertical
/// through `(east, north)` on its tangent plane meets the surface: it solves for the `t` that
/// puts `origin + east*E + north*N + t*U` on the ellipsoid (a quadratic; the root nearest the
/// origin), then converts that ECEF point to geodetic, which is exact in closed form for a point
/// on the surface. Exact to rounding: the round trip through [`gps_to_local`] is within
/// nanometres out to 50 km and beyond. A far-side point shares its `(east, north)` with a
/// near-side one and comes back as that one; no survey spans that distance.
///
/// `None` only when no such point exists: the vertical misses the ellipsoid (an offset beyond
/// the horizon, roughly an earth radius out), or an input is not finite. Each caller says what
/// it does then: GlobalView refuses to draw the project (`survey_geometry.rs`), the TML importer
/// anchors at the file's own fix, the stalx reader leaves the station without coordinates.
///
/// The longitude is not continuous with the origin's across the antimeridian. A map that draws
/// lines from the origin passes it through [`unwrap_longitude`] first; anything that stores it
/// as a [`GpsCoordinates`] must not.
pub fn local_to_gps(east: f64, north: f64, origin: &GpsCoordinates) -> Option<(f64, f64)> {
    let b2 = WGS84_A * WGS84_A * (1.0 - WGS84_E2);
    let a2 = WGS84_A * WGS84_A;

    let lat0 = origin.latitude.to_radians();
    let lon0 = origin.longitude.to_radians();
    let (sin_lat, cos_lat) = lat0.sin_cos();
    let (sin_lon, cos_lon) = lon0.sin_cos();
    // ENU unit vectors at the origin, in ECEF.
    let e = (-sin_lon, cos_lon, 0.0);
    let n = (-sin_lat * cos_lon, -sin_lat * sin_lon, cos_lat);
    let u = (cos_lat * cos_lon, cos_lat * sin_lon, sin_lat);

    let (x0, y0, z0) = geodetic_to_ecef(origin.latitude, origin.longitude, 0.0);
    let q = (
        x0 + east * e.0 + north * n.0,
        y0 + east * e.1 + north * n.1,
        z0 + east * e.2 + north * n.2,
    );
    // |(q + t*u)| on the ellipsoid: A t^2 + B t + C = 0.
    let qa = (u.0 * u.0 + u.1 * u.1) / a2 + u.2 * u.2 / b2;
    let qb = 2.0 * ((q.0 * u.0 + q.1 * u.1) / a2 + q.2 * u.2 / b2);
    let qc = (q.0 * q.0 + q.1 * q.1) / a2 + q.2 * q.2 / b2 - 1.0;
    let disc = qb * qb - 4.0 * qa * qc;
    if !disc.is_finite() || disc < 0.0 {
        return None;
    }
    let sqrt_disc = disc.sqrt();
    let (t1, t2) = (
        (-qb + sqrt_disc) / (2.0 * qa),
        (-qb - sqrt_disc) / (2.0 * qa),
    );
    let t = if t1.abs() <= t2.abs() { t1 } else { t2 };
    let (x, y, z) = (q.0 + t * u.0, q.1 + t * u.1, q.2 + t * u.2);

    // On the surface (height 0) geodetic latitude satisfies
    // tan(lat) = z / (p * (1 - e^2)) exactly.
    let p = x.hypot(y);
    let lat = z.atan2(p * (1.0 - WGS84_E2)).to_degrees();
    let lon = y.atan2(x).to_degrees();
    (lat.is_finite() && lon.is_finite()).then_some((lat, lon))
}

/// `lon` (degrees, in [-180, 180] as [`local_to_gps`] returns it) moved by a whole turn when
/// that brings it within half a turn of `reference`. Away from the antimeridian it is returned
/// unchanged, bit for bit.
///
/// The explicit step for a map that draws lines from the origin (GlobalView,
/// `survey_geometry.rs`; the frontend's `metersToMapCoords` does the same): 1000 m east of
/// 179.999 is drawn at 180.0084, continuous with the origin, not at -179.9916, which MapLibre
/// joins to the origin by a line the long way round the globe (#576 D36 review). The result can
/// leave [-180, 180], so it is never stored as a [`GpsCoordinates`].
pub fn unwrap_longitude(lon: f64, reference: f64) -> f64 {
    let delta = lon - reference;
    if delta > 180.0 {
        lon - 360.0
    } else if delta < -180.0 {
        lon + 360.0
    } else {
        lon
    }
}

/// Haversine distance between two GPS points in meters.
///
/// Uses WGS84 mean earth radius for the calculation.
pub fn haversine_distance(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS: f64 = 6_371_008.8; // WGS84 mean radius in meters

    let lat1_rad = lat1.to_radians();
    let lat2_rad = lat2.to_radians();
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();

    let a =
        (dlat / 2.0).sin().powi(2) + lat1_rad.cos() * lat2_rad.cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().asin();

    EARTH_RADIUS * c
}

/// Initial bearing from point 1 to point 2 in degrees [0, 360).
///
/// Returns the forward azimuth (compass bearing) from the first point to
/// the second point, measured clockwise from north.
pub fn initial_bearing(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let lat1_rad = lat1.to_radians();
    let lat2_rad = lat2.to_radians();
    let dlon_rad = (lon2 - lon1).to_radians();

    let x = dlon_rad.sin() * lat2_rad.cos();
    let y = lat1_rad.cos() * lat2_rad.sin() - lat1_rad.sin() * lat2_rad.cos() * dlon_rad.cos();

    let bearing = x.atan2(y).to_degrees();
    (bearing + 360.0) % 360.0
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod tests {
    use super::*;

    #[test]
    fn same_point_returns_zero() {
        let gps = GpsCoordinates::new(45.0, -90.0, 10.0).unwrap();
        let (east, north) = gps_to_local(&gps, &gps);
        assert!(
            east.abs() < 1e-9,
            "East should be ~0 for same point, got {east}"
        );
        assert!(
            north.abs() < 1e-9,
            "North should be ~0 for same point, got {north}"
        );
    }

    #[test]
    fn small_latitude_delta_at_equator() {
        // At the equator, 1 degree of latitude ~ 110574.3 meters (per WGS84)
        // 0.01 degree ~ 1105.7 meters
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        let target = GpsCoordinates::new(0.01, 0.0, 0.0).unwrap();

        let (east, north) = gps_to_local(&target, &origin);

        // North should be approximately 1105.7 meters (within tolerance)
        assert!(
            (north - 1105.7).abs() < 5.0,
            "North should be ~1105.7m for 0.01 deg lat, got {north}"
        );
        // East should be nearly zero
        assert!(
            east.abs() < 1.0,
            "East should be ~0 for lat-only change, got {east}"
        );
    }

    #[test]
    fn small_longitude_delta_at_equator() {
        // At the equator, 1 degree of longitude ~ 111319.5 meters
        // 0.01 degree ~ 1113.2 meters
        let origin = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        let target = GpsCoordinates::new(0.0, 0.01, 0.0).unwrap();

        let (east, north) = gps_to_local(&target, &origin);

        // East should be approximately 1113.2 meters (within tolerance)
        assert!(
            (east - 1113.2).abs() < 5.0,
            "East should be ~1113.2m for 0.01 deg lon, got {east}"
        );
        // North should be nearly zero
        assert!(
            north.abs() < 1.0,
            "North should be ~0 for lon-only change, got {north}"
        );
    }

    #[test]
    fn depth_reference_is_ignored() {
        // Different depth_reference values should give the same result
        let origin1 = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
        let origin2 = GpsCoordinates::new(45.0, -90.0, 100.0).unwrap();
        let target1 = GpsCoordinates::new(45.01, -90.0, 0.0).unwrap();
        let target2 = GpsCoordinates::new(45.01, -90.0, 50.0).unwrap();

        let (east1, north1) = gps_to_local(&target1, &origin1);
        let (east2, north2) = gps_to_local(&target2, &origin2);

        assert!(
            (east1 - east2).abs() < 1e-9,
            "East should be same regardless of depth_reference"
        );
        assert!(
            (north1 - north2).abs() < 1e-9,
            "North should be same regardless of depth_reference"
        );
    }

    #[test]
    fn negative_deltas_give_negative_results() {
        // Moving south should give negative north
        let origin = GpsCoordinates::new(0.01, 0.0, 0.0).unwrap();
        let target = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();

        let (east, north) = gps_to_local(&target, &origin);

        assert!(north < 0.0, "Moving south should give negative north");
        assert!(east.abs() < 1.0, "East should be ~0 for lat-only change");
    }

    #[test]
    fn mid_latitude_conversion() {
        // Test at 45 degrees latitude (typical cave location)
        // At 45 deg lat, 1 degree of longitude ~ 78847 meters
        // 0.01 degree ~ 788.5 meters
        let origin = GpsCoordinates::new(45.0, -90.0, 0.0).unwrap();
        let target = GpsCoordinates::new(45.0, -89.99, 0.0).unwrap();

        let (east, north) = gps_to_local(&target, &origin);

        // East should be approximately 788.5 meters at 45 deg lat
        assert!(
            (east - 788.5).abs() < 5.0,
            "East should be ~788.5m for 0.01 deg lon at 45 deg lat, got {east}"
        );
        assert!(
            north.abs() < 1.0,
            "North should be ~0 for lon-only change, got {north}"
        );
    }

    #[test]
    fn geodetic_to_ecef_basic() {
        // At equator, lon=0: should be on X axis at ~6378137m
        let (x, y, z) = geodetic_to_ecef(0.0, 0.0, 0.0);
        assert!(
            (x - WGS84_A).abs() < 1.0,
            "X at equator/prime meridian should be ~a"
        );
        assert!(y.abs() < 1.0, "Y should be ~0");
        assert!(z.abs() < 1.0, "Z should be ~0");

        // At north pole: should be on Z axis at ~6356752m (polar radius)
        let (x, y, z) = geodetic_to_ecef(90.0, 0.0, 0.0);
        assert!(x.abs() < 1.0, "X at north pole should be ~0");
        assert!(y.abs() < 1.0, "Y at north pole should be ~0");
        // Polar radius b = a * sqrt(1 - e^2) ~ 6356752.314
        let expected_z = WGS84_A * (1.0 - WGS84_E2).sqrt();
        assert!(
            (z - expected_z).abs() < 1.0,
            "Z at north pole should be ~{expected_z}, got {z}"
        );
    }

    // === haversine_distance tests ===

    #[test]
    fn haversine_same_point_is_zero() {
        let d = haversine_distance(45.0, -90.0, 45.0, -90.0);
        assert!(d.abs() < 1e-9, "Same point should be 0m, got {d}");
    }

    #[test]
    fn haversine_equator_one_degree_lon() {
        // At equator, 1 degree of longitude ~ 111,195 meters (with mean radius)
        let d = haversine_distance(0.0, 0.0, 0.0, 1.0);
        assert!(
            (d - 111_195.0).abs() < 100.0,
            "1 deg lon at equator should be ~111,195m, got {d}"
        );
    }

    #[test]
    fn haversine_equator_one_degree_lat() {
        let d = haversine_distance(0.0, 0.0, 1.0, 0.0);
        assert!(
            (d - 111_195.0).abs() < 100.0,
            "1 deg lat at equator should be ~111,195m, got {d}"
        );
    }

    #[test]
    fn haversine_known_distance() {
        // London (51.5074, -0.1278) to Paris (48.8566, 2.3522) ~ 343.5 km
        let d = haversine_distance(51.5074, -0.1278, 48.8566, 2.3522);
        assert!(
            (d - 343_500.0).abs() < 1000.0,
            "London-Paris should be ~343.5km, got {d}m"
        );
    }

    // === initial_bearing tests ===

    #[test]
    fn bearing_due_north() {
        let b = initial_bearing(0.0, 0.0, 1.0, 0.0);
        assert!(
            b.abs() < 0.01 || (b - 360.0).abs() < 0.01,
            "Due north should be ~0 degrees, got {b}"
        );
    }

    #[test]
    fn bearing_due_east() {
        let b = initial_bearing(0.0, 0.0, 0.0, 1.0);
        assert!(
            (b - 90.0).abs() < 0.01,
            "Due east should be ~90 degrees, got {b}"
        );
    }

    #[test]
    fn bearing_due_south() {
        let b = initial_bearing(1.0, 0.0, 0.0, 0.0);
        assert!(
            (b - 180.0).abs() < 0.01,
            "Due south should be ~180 degrees, got {b}"
        );
    }

    /// `local_to_gps` is the inverse of `gps_to_local` to well under a millimetre, from the
    /// equator to near the pole and out to 20 km in every direction (#576 D36).
    #[test]
    fn local_to_gps_inverts_gps_to_local() {
        for (lat0, lon0) in [
            (0.0, 0.0),
            (30.0, 0.0),
            (-30.0, 151.0),
            (70.0, 0.0),
            (-54.8, -68.3),
            (89.0, 10.0),
        ] {
            let origin = GpsCoordinates::new(lat0, lon0, 0.0).unwrap();
            for d in [0.0, 1.0, 1_000.0, 5_000.0, 20_000.0] {
                for k in 0..8 {
                    let a = f64::from(k) * std::f64::consts::FRAC_PI_4;
                    let (east, north) = (d * a.sin(), d * a.cos());
                    let (lat, lon) = local_to_gps(east, north, &origin).unwrap();
                    let back = GpsCoordinates::new(lat, lon, 0.0).unwrap();
                    let (e, n) = gps_to_local(&back, &origin);
                    assert!(
                        (e - east).hypot(n - north) < 1e-5,
                        "origin ({lat0}, {lon0}), ({east}, {north}) came back as ({e}, {n})"
                    );
                }
            }
        }
    }

    /// The issue's scenario: a datum typed as (30.009, 0) from an origin datum at (30, 0) is
    /// placed 997.67 m north, and the inverse draws it back on its own fix - not 3.1 m south of
    /// it, as the sphere the map used did.
    #[test]
    fn a_second_datum_converts_back_to_its_own_fix() {
        let origin = GpsCoordinates::new(30.0, 0.0, 0.0).unwrap();
        let fix = GpsCoordinates::new(30.009, 0.0, 0.0).unwrap();
        let (east, north) = gps_to_local(&fix, &origin);
        assert!((north - 997.673).abs() < 1e-3, "north {north}");
        let (lat, lon) = local_to_gps(east, north, &origin).unwrap();
        assert!((lat - 30.009).abs() < 1e-10, "lat {lat}");
        assert!(lon.abs() < 1e-10, "lon {lon}");
    }

    /// A survey across the antimeridian stays one piece on a map (#576 D36 review RV1-RUST-5):
    /// from an origin at 179.999 E, a station 1000 m east is at -179.9916 as a fix, which a map
    /// draws at 180.0084 - continuous with the origin - after the explicit `unwrap_longitude`
    /// step, not at -179.9916, which MapLibre joins to the origin by a line the long way round
    /// the globe. The mirror case west of -179.999 too. The fix itself stays in [-180, 180]
    /// (GEO-D1), so it can be stored as a `GpsCoordinates`.
    #[test]
    fn unwrap_longitude_keeps_a_survey_across_the_antimeridian_continuous() {
        for (lon0, east) in [(179.999, 1_000.0), (-179.999, -1_000.0)] {
            let origin = GpsCoordinates::new(-16.8, lon0, 0.0).unwrap();
            let (lat, lon) = local_to_gps(east, 0.0, &origin).unwrap();
            assert!(
                (lon + lon0).abs() < 0.01,
                "origin lon {lon0}, {east} m east: the fix {lon} is not across the antimeridian"
            );
            let drawn = unwrap_longitude(lon, lon0);
            assert!(
                (drawn - lon0).abs() < 0.01,
                "origin lon {lon0}, {east} m east: drawn at {drawn}, across the globe"
            );
            let turn = drawn - lon - 360.0_f64.copysign(lon0);
            assert!(turn.abs() < 1e-9, "a whole turn, nothing else: {turn}");
            // Both name the same point: the exact inverse either way.
            for longitude in [lon, drawn] {
                let back = GpsCoordinates {
                    latitude: lat,
                    longitude,
                    depth_reference: 0.0,
                };
                let (e, n) = gps_to_local(&back, &origin);
                assert!((e - east).hypot(n) < 1e-6, "came back as ({e}, {n})");
            }
        }
        // Away from the antimeridian the step changes nothing, bit for bit.
        assert_eq!(
            unwrap_longitude(-87.25, -87.3).to_bits(),
            (-87.25_f64).to_bits()
        );
    }

    /// `local_to_gps` under test, as `(latitude, longitude)` of a surface point.
    fn inverse(east: f64, north: f64, origin: &GpsCoordinates) -> (f64, f64) {
        local_to_gps(east, north, origin).unwrap()
    }

    /// GEO-D1: the round trip `gps_to_local(local_to_gps(east, north))` gives back
    /// `(east, north)` within 1e-6 m for every offset up to 50 km, at the equator, mid
    /// latitudes, beside both poles and across the antimeridian. Deterministic sweep: fixed
    /// distances (0.1 m to 50 km) at eight bearings, plus 2000 pseudo-random offsets per
    /// origin, uniform over the 50 km disc. The #576 one-correction approximation misses by
    /// about 0.1 mm at 50 km and fails this bound.
    #[test]
    fn local_to_gps_round_trips_within_a_micrometre_out_to_50_km() {
        const ORIGINS: [(f64, f64); 11] = [
            (0.0, 0.0),
            (20.5, -87.3),
            (-33.9, 151.2),
            (45.0, -90.0),
            (60.0, 25.0),
            (70.0, 0.0),
            (-54.8, -68.3),
            (89.9, 10.0),
            (-89.99, -45.0),
            (-16.8, 179.999),
            (10.0, -179.9995),
        ];
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let mut uniform = move || {
            // xorshift64*, top 53 bits as a float in [0, 1).
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let bits = state.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11;
            bits as f64 / (1_u64 << 53) as f64
        };
        let mut worst = (0.0_f64, String::new());
        let mut samples = 0_u32;
        for (lat0, lon0) in ORIGINS {
            let origin = GpsCoordinates::new(lat0, lon0, 0.0).unwrap();
            let mut offsets = Vec::new();
            for d in [0.1, 1.0, 1_000.0, 5_000.0, 20_000.0, 50_000.0] {
                for k in 0..8 {
                    let a = f64::from(k) * std::f64::consts::FRAC_PI_4 + 0.1;
                    offsets.push((d * a.sin(), d * a.cos()));
                }
            }
            for _ in 0..2000 {
                let d = 50_000.0 * uniform().sqrt();
                let a = std::f64::consts::TAU * uniform();
                offsets.push((d * a.sin(), d * a.cos()));
            }
            for (east, north) in offsets {
                let (lat, lon) = inverse(east, north, &origin);
                let back = GpsCoordinates {
                    latitude: lat,
                    longitude: lon,
                    depth_reference: 0.0,
                };
                let (e, n) = gps_to_local(&back, &origin);
                let miss = (e - east).hypot(n - north);
                samples += 1;
                if miss.is_nan() || miss > worst.0 {
                    worst = (
                        miss,
                        format!("origin ({lat0}, {lon0}), offset ({east:.3}, {north:.3}) m"),
                    );
                }
            }
        }
        println!(
            "{samples} round trips; worst miss {:.3e} m at {}",
            worst.0, worst.1
        );
        assert!(
            worst.0 < 1e-6,
            "round trip missed by {:.3e} m (bound 1e-6 m) at {}",
            worst.0,
            worst.1
        );
    }

    #[test]
    fn bearing_due_west() {
        let b = initial_bearing(0.0, 1.0, 0.0, 0.0);
        assert!(
            (b - 270.0).abs() < 0.01,
            "Due west should be ~270 degrees, got {b}"
        );
    }

    /// `local_to_gps` must invert `gps_to_local` exactly in both directions, at any latitude,
    /// from cave scale up to regional distances (near side only).
    #[test]
    fn local_to_gps_inverts_gps_to_local_both_ways_at_regional_scale() {
        let origins = [(0.0, 0.0), (20.5, -87.4), (-60.0, 150.0), (80.0, 10.0)];
        let offsets = [
            (0.0, 0.0),
            (10.0, -3.0),
            (-4_000.0, 2_500.0),
            (90_000.0, 120_000.0),
        ];
        for (lat0, lon0) in origins {
            let origin = GpsCoordinates::new(lat0, lon0, 0.0).unwrap();
            for (east, north) in offsets {
                let (lat, lon) = local_to_gps(east, north, &origin).unwrap();
                let back = gps_to_local(&GpsCoordinates::new(lat, lon, 0.0).unwrap(), &origin);
                assert!(
                    (back.0 - east).abs() < 1e-6 && (back.1 - north).abs() < 1e-6,
                    "origin ({lat0}, {lon0}) offset ({east}, {north}) came back as {back:?}"
                );
                let target = GpsCoordinates::new(lat, lon, 0.0).unwrap();
                let (e2, n2) = gps_to_local(&target, &origin);
                let (lat2, lon2) = local_to_gps(e2, n2, &origin).unwrap();
                assert!((lat2 - lat).abs() < 1e-9 && (lon2 - lon).abs() < 1e-9);
            }
        }
    }

    #[test]
    fn local_to_gps_refuses_a_point_beyond_the_horizon() {
        let origin = GpsCoordinates::new(20.5, -87.4, 0.0).unwrap();
        assert_eq!(local_to_gps(20_000_000.0, 0.0, &origin), None);
        // At the equator the horizon due east is exactly one semi-major axis out.
        let equator = GpsCoordinates::new(0.0, 0.0, 0.0).unwrap();
        assert!(local_to_gps(6_378_000.0, 0.0, &equator).is_some());
        assert_eq!(local_to_gps(6_379_000.0, 0.0, &equator), None);
        assert_eq!(local_to_gps(f64::NAN, 0.0, &equator), None);
        assert_eq!(local_to_gps(0.0, f64::INFINITY, &equator), None);
    }

    /// The backend/frontend parity table (#576 GEO-D1), one row per line:
    /// `origin_lat origin_lon east_m north_m lat lon`, the last two what `local_to_gps` returns,
    /// or `none none` where it is `None`. Offsets of about 0.1, 1, 5, 20 and 50 km at the
    /// equator, at a mid latitude, 1.1 km from the north pole, 11 km from the south pole and on
    /// both sides of the antimeridian, then six out to and past the horizon.
    ///
    /// src/utils/__tests__/coordinates.test.ts reads this string from this file and asserts the
    /// frontend's `enuToGps` against it to 1e-10 degrees; the test below pins it to the backend
    /// to 1e-12 (room for a platform libm's last-ulp differences). So the two sides agree to
    /// within 1e-9 degrees. A change to the algorithm fails here first: the failure prints the
    /// new rows in this format.
    const FRONTEND_PARITY: &str = "
0.0 0.0 50.0 86.603 0.0007832110982338431 0.0004491576421060447
0.0 0.0 978.148 -207.912 -0.0018802926674557146 0.008786853024452559
0.0 0.0 522.642 -4972.609 -0.04497076253369422 0.004694974408967928
0.0 0.0 -18270.909 -8134.733 -0.07356806210045728 -0.16413072696480338
0.0 0.0 -33456.53 37157.241 0.33604063396807554 -0.3005516354845275
20.5 -87.3 50.0 86.603 20.50078224603546 -87.29952066955657
20.5 -87.3 978.148 -207.912 20.498121769137384 -87.29062303949956
20.5 -87.3 522.642 -4972.609 20.455084421507678 -87.29499111891376
20.5 -87.3 -18270.909 -8134.733 20.426433910558995 -87.47507203528993
20.5 -87.3 -33456.53 37157.241 20.835323529120537 -87.621439430737
89.99 10.0 50.0 86.603 89.99076450421282 12.778259848392883
89.99 10.0 978.148 -207.912 89.98525599255318 46.43873006614401
89.99 10.0 522.642 -4972.609 89.94527962609288 14.905450402656989
89.99 10.0 -18270.909 -8134.733 89.81664393463652 -53.144145854655775
89.99 10.0 -33456.53 37157.241 89.55972425929303 -127.1292032841038
-89.9 -45.0 50.0 86.603 -89.89922364612443 -44.74548939266109
-89.9 -45.0 978.148 -207.912 -89.90147148553713 -39.90071593383439
-89.9 -45.0 522.642 -4972.609 -89.94432296776837 -40.179037625114745
-89.9 -45.0 -18270.909 -8134.733 -89.83417878966263 -125.56982306428287
-89.9 -45.0 -33456.53 37157.241 -89.47375778278108 -79.69507763889871
-16.8 179.999 50.0 86.603 -16.799217445255245 179.99946904945628
-16.8 179.999 978.148 -207.912 -16.801878510833923 -179.99182387632433
-16.8 179.999 522.642 -4972.609 -16.844932886971158 -179.99609592546085
-16.8 179.999 -18270.909 -8134.733 -16.873434692740087 179.8275337594089
-16.8 179.999 -33456.53 37157.241 -16.463996825095037 179.6856877703171
10.0 -179.9995 50.0 86.603 10.000782973630637 -179.99904395832075
10.0 -179.9995 978.148 -207.912 9.998120156976212 -179.9905785474652
10.0 -179.9995 522.642 -4972.609 9.955042758774415 -179.9947337343973
10.0 -179.9995 -18270.909 -8134.733 9.926412344983376 179.83389159300944
10.0 -179.9995 -33456.53 37157.241 10.335795489850149 179.69502900323235
0.0 0.0 6000000.0 0.0 0.0 70.17173779372447
0.0 0.0 6378000.0 0.0 0.0 89.6244638328084
45.0 -90.0 3000000.0 4000000.0 62.02185125173275 0.3237881472003407
0.0 0.0 6379000.0 0.0 none none
20.5 -87.4 20000000.0 0.0 none none
89.99 10.0 0.0 6400000.0 none none
";

    /// `(origin_lat, origin_lon, east, north)` and the fix of each [`FRONTEND_PARITY`] row.
    fn frontend_parity_rows() -> Vec<([f64; 4], Option<(f64, f64)>)> {
        let rows: Vec<_> = FRONTEND_PARITY
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                let field: Vec<&str> = line.split_whitespace().collect();
                assert_eq!(field.len(), 6, "a parity row has six fields: {line:?}");
                let num = |s: &str| s.parse::<f64>().unwrap();
                let input = [num(field[0]), num(field[1]), num(field[2]), num(field[3])];
                let fix = (field[4] != "none").then(|| (num(field[4]), num(field[5])));
                (input, fix)
            })
            .collect();
        assert!(rows.len() > 30, "the parity table lost its rows");
        rows
    }

    #[test]
    fn local_to_gps_matches_the_frontend_parity_table() {
        let mut mismatches = Vec::new();
        for ([lat0, lon0, east, north], fix) in frontend_parity_rows() {
            let origin = GpsCoordinates::new(lat0, lon0, 0.0).unwrap();
            let got = local_to_gps(east, north, &origin);
            let agrees = match (got, fix) {
                (Some((la, lo)), Some((lat, lon))) => {
                    (la - lat).abs() < 1e-12 && (lo - lon).abs() < 1e-12
                }
                (None, None) => true,
                _ => false,
            };
            if !agrees {
                let now = got.map_or("none none".to_string(), |(la, lo)| format!("{la:?} {lo:?}"));
                mismatches.push(format!("{lat0:?} {lon0:?} {east:?} {north:?} {now}"));
            }
        }
        assert!(
            mismatches.is_empty(),
            "local_to_gps no longer returns these rows of FRONTEND_PARITY; it now gives:\n{}",
            mismatches.join("\n")
        );
    }
}
