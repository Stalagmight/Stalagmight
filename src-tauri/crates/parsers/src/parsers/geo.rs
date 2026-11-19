//! Geographic helpers shared by the survey importers (Compass, Walls).
//!
//! Each importer keeps its own format-specific logic - the Compass datum table
//! and datum-implied hemisphere, the Walls signed-zone hemisphere and string
//! datum names - and calls into here only for the parts that are genuinely the
//! same: the `igrf` magnetic-declination boundary and the `proj4rs` UTM → WGS84
//! projection. Keeping those in one place means the external-crate API surface
//! is maintained once instead of drifting between the two parsers.

use chrono::{Datelike, NaiveDate};

/// International foot → meters. Shared by the Compass and Walls importers so the
/// conversion has a single source of truth.
pub const FEET_TO_METERS: f64 = 0.3048;

/// The length (meters) a leg with no usable tape length imports as.
///
/// The project model has no zero-length shot (`create_section` rejects one,
/// which fails the whole import), so a coincident-station tie - two names for
/// one point, or a tape reading of 0 - gets this negligible length instead: the
/// stations stay connected and effectively coincident. Shared so the Walls and
/// Ariane importers nudge by the same amount (#812). Only the horizontal offset
/// collapses: the depth each end records is kept, so a tie with a depth step
/// is a vertical shot of one depth difference.
pub const COINCIDENT_TIE_EPSILON_M: f64 = 0.001;

/// Trim `s` and return the owned result only when it is non-empty - the survey
/// parsers' shared "skip blank fields" helper.
pub fn non_empty(s: &str) -> Option<String> {
    let t = s.trim();
    (!t.is_empty()).then(|| t.to_string())
}

/// IGRF magnetic declination (degrees) at `lat`/`lng` on `date`, or a message
/// explaining why it could not be computed. The bundled `igrf` model only spans
/// 1900-01-01 .. 2025-12-31, so dates outside that range return `Err`.
pub fn igrf_declination(lat: f64, lng: f64, date: NaiveDate) -> Result<f64, String> {
    let ordinal = u16::try_from(date.ordinal()).map_err(|_| "invalid day-of-year".to_string())?;
    let d = time::Date::from_ordinal_date(date.year(), ordinal)
        .map_err(|e| format!("invalid date: {e}"))?;
    igrf::declination(lat, lng, 0, d)
        .map(|field| field.d)
        .map_err(|e| format!("{e:?}"))
}

/// Project a UTM easting/northing (meters) in the given absolute `zone` and
/// hemisphere to WGS84 `(longitude, latitude)` in **degrees**.
///
/// `datum_token` is the proj4rs datum/ellipsoid clause the caller selected for
/// its format (e.g. `"datum=NAD83"`, `"ellps=WGS72"`, `"datum=WGS84"`). Returns
/// `None` if proj4rs cannot build the projection or transform the point.
pub fn project_utm(
    easting: f64,
    northing: f64,
    zone: u32,
    south: bool,
    datum_token: &str,
) -> Option<(f64, f64)> {
    let south_clause = if south { " +south" } else { "" };
    let to = proj4rs::Proj::from_proj_string("+proj=longlat +datum=WGS84 +no_defs").ok()?;
    let from = proj4rs::Proj::from_proj_string(&format!(
        "+proj=utm +zone={zone}{south_clause} +{datum_token} +units=m +no_defs"
    ))
    .ok()?;
    let (lon, lat, _) =
        proj4rs::adaptors::transform_xyz(&from, &to, easting, northing, 0.0).ok()?;
    Some((lon.to_degrees(), lat.to_degrees()))
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn project_utm_matches_known_reference() {
        // UTM zone 31N, easting 500000 (central meridian), northing 0 -> the
        // equator at 3°E. Returns (lon, lat).
        let (lon, lat) = project_utm(500_000.0, 0.0, 31, false, "datum=WGS84").unwrap();
        assert!((lon - 3.0).abs() < 1e-4, "lon {lon}");
        assert!(lat.abs() < 1e-4, "lat {lat}");
    }

    #[test]
    fn igrf_declination_rejects_out_of_range_year() {
        let date = NaiveDate::from_ymd_opt(2030, 1, 1).unwrap();
        assert!(igrf_declination(60.0, -150.0, date).is_err());
    }
}
