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

/// The dates the bundled geomagnetic model covers, as the importers name it to
/// the user. The vendored `igrf` crate carries IGRF-14 (`vendor/igrf`), which
/// is defined from 1900.0 up to, not including, 2030.0 (#811).
pub const IGRF_MODEL_SPAN: &str = "1900-01-01 to 2029-12-31";

/// IGRF magnetic declination (degrees) at `lat`/`lng` on `date`, or a
/// plain-language reason it could not be computed, written to finish a
/// sentence in an import receipt. A date outside [`IGRF_MODEL_SPAN`] returns
/// `Err`: the model can extrapolate, but the result is not a published value.
pub fn igrf_declination(lat: f64, lng: f64, date: NaiveDate) -> Result<f64, String> {
    let ordinal = u16::try_from(date.ordinal()).map_err(|_| "invalid day-of-year".to_string())?;
    let d = time::Date::from_ordinal_date(date.year(), ordinal)
        .map_err(|e| format!("invalid date: {e}"))?;
    // Never the error's `Debug` form: `DateOutOfRange` carries the whole
    // extrapolated field, and that is what reached the receipt before #811.
    igrf::declination(lat, lng, 0, d)
        .map(|field| field.d)
        .map_err(|e| match e {
            // The callers already name the date; the coordinates stay out of
            // the text, since warnings are logged and a cave's position is
            // often private.
            igrf::Error::DateOutOfRange(_) => format!(
                "the date is outside the dates the IGRF-14 geomagnetic model covers \
                 ({IGRF_MODEL_SPAN})"
            ),
            igrf::Error::LatitudeOutOfBounds => "the latitude is out of range".to_string(),
            igrf::Error::LongitudeOutOfBounds => "the longitude is out of range".to_string(),
        })
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
        let reason = igrf_declination(60.0, -150.0, date).unwrap_err();
        // #811: the reason finishes a sentence the user reads. It names the
        // date and the model's span, and carries none of the model's internals.
        assert_eq!(
            reason,
            "the date is outside the dates the IGRF-14 geomagnetic model covers \
             (1900-01-01 to 2029-12-31)"
        );
    }

    /// Both ends of [`IGRF_MODEL_SPAN`] are inside the model, the day after its
    /// end is not: the span the user is told is the span the model serves.
    #[test]
    fn igrf_declination_serves_exactly_the_span_it_names() {
        for (y, m, d) in [(1900, 1, 1), (2029, 12, 31)] {
            let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
            assert!(igrf_declination(10.0, 10.0, date).is_ok(), "{date}");
        }
        let after = NaiveDate::from_ymd_opt(2030, 1, 1).unwrap();
        assert!(igrf_declination(10.0, 10.0, after).is_err());
    }

    /// Coordinates never appear in the reason (it is logged with the import).
    #[test]
    fn an_out_of_range_coordinate_is_not_echoed() {
        let date = NaiveDate::from_ymd_opt(2026, 1, 1).unwrap();
        let lat = igrf_declination(123.456, 0.0, date).unwrap_err();
        let lng = igrf_declination(0.0, 654.321, date).unwrap_err();
        assert!(
            !lat.contains("123") && !lng.contains("654"),
            "{lat} / {lng}"
        );
    }

    /// IGRF-14 declinations (degrees east) from the British Geological Survey's
    /// IGRF-14 calculator
    /// (`geomag.bgs.ac.uk/web_service/GMModels/igrf/14/`, altitude 0, queried
    /// 2026-11-19), at synthetic points. The dates cover the span IGRF-13 did
    /// not reach (2026 to its last days in 2029), the 2020-2025 interval
    /// IGRF-14 revised, a model boundary, and intervals it left unchanged.
    /// BGS reports three decimals, so 0.01 degree is two orders of rounding.
    #[test]
    fn igrf_declination_matches_the_bgs_igrf14_calculator() {
        let cases: [(f64, f64, (i32, u32, u32), f64); 9] = [
            (20.5, -87.3, (2026, 6, 15), -2.419),
            (60.0, -150.0, (2027, 1, 1), 13.848),
            (-33.9, 151.2, (2029, 12, 15), 12.881),
            (45.0, 5.0, (2022, 3, 1), 2.157),
            (10.0, 100.0, (2026, 11, 3), -0.418),
            (35.0, -110.0, (2010, 7, 1), 10.64),
            (51.5, 0.0, (1950, 1, 1), -9.053),
            (-45.0, -70.0, (2024, 12, 31), 7.078),
            (0.0, 0.0, (2025, 1, 1), -4.014),
        ];
        for (lat, lng, (y, m, d), expected) in cases {
            let date = NaiveDate::from_ymd_opt(y, m, d).unwrap();
            let got = igrf_declination(lat, lng, date).unwrap();
            assert!(
                (got - expected).abs() < 0.01,
                "({lat}, {lng}) on {date}: got {got:.4}, BGS IGRF-14 says {expected}"
            );
        }
    }

    /// Tripwire for the next expiry (#811): the bundled model must cover a
    /// survey dated today. It reads the wall clock on purpose and goes red on
    /// 2030-01-01, the first day a user's survey falls outside IGRF-14 - not
    /// earlier, when the next generation (IGRF-15, expected late 2029) may not
    /// exist yet and nothing could turn it green. Update `vendor/igrf/data` as
    /// MODIFICATIONS.md describes.
    #[test]
    fn the_bundled_model_covers_today() {
        let today = chrono::Utc::now().date_naive();
        assert!(
            igrf_declination(0.0, 0.0, today).is_ok(),
            "the bundled IGRF model does not cover today ({today}); update vendor/igrf \
             to the next IGRF generation (see vendor/igrf/MODIFICATIONS.md)"
        );
    }
}
