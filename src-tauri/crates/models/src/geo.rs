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

/// The exact inverse of [`gps_to_local`] on the origin's side of the earth: the
/// point on the WGS84 ellipsoid whose local `(east, north)` relative to `origin`
/// is the one given, as `(latitude, longitude)` in degrees. A far-side point
/// shares its `(east, north)` with a near-side one and comes back as that one;
/// no survey spans that distance.
///
/// [`gps_to_local`] drops the "up" component, so the inverse puts the point back
/// on the ellipsoid: it solves for the up offset that lands
/// `origin + east*E + north*N + up*U` on the surface (taking the root nearest
/// the origin), then converts that ECEF point to geodetic, which is exact in
/// closed form for a point on the surface. `None` only when no such point exists
/// (an offset beyond the horizon, thousands of kilometres out).
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
#[expect(clippy::unwrap_used)]
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

    #[test]
    fn bearing_due_west() {
        let b = initial_bearing(0.0, 1.0, 0.0, 0.0);
        assert!(
            (b - 270.0).abs() < 0.01,
            "Due west should be ~270 degrees, got {b}"
        );
    }

    /// `local_to_gps` must invert `gps_to_local` exactly in both directions, at any
    /// latitude, from cave scale up to regional distances (near side only).
    #[test]
    fn local_to_gps_inverts_gps_to_local() {
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
    }
}
