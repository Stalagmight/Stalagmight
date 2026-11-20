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
