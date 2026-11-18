//! Which north a line's stored azimuths are measured against (#438).
//!
//! The contract this file exists to hold: **a stored azimuth is the azimuth that
//! was measured**. No importer rotates a reading into some internal convention,
//! and nothing writes a declination back into a shot. Instead each line
//! declares how its readings relate to true north, and geometry adds that
//! constant when it computes - once per shot, at compute time.
//!
//! That is Ariane's model (its files carry measured azimuths plus a
//! `useMagneticAzimuth` flag and it derives the declination itself), and it is
//! the one that honours the project's no-mutation-of-original-data policy.
//!
//! Two directions, and both are needed:
//!
//! * [`AzimuthReference::to_true`] - measured -> world bearing. Every geometry
//!   walk (coordinates, closure solve, move/split) reads through it.
//! * [`AzimuthReference::to_measured`] - world bearing -> measured. Every path
//!   that solves a displacement and writes an azimuth BACK onto a shot has to
//!   use it, or the line creeps by one correction per adjustment.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::normalize_azimuth;

/// What a line's stored azimuths are measured against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "lowercase")]
#[ts(export, export_to = "../../bindings/", rename_all = "lowercase")]
pub enum NorthReference {
    /// Compass readings, uncorrected. The default, because it is what every
    /// instrument produces and what a file that says nothing implies.
    #[default]
    Magnetic,
    /// Already true north; [`AzimuthReference::correction`] is 0.
    True,
    /// A projection grid north. The correction folds in the meridian
    /// convergence as well as the declination.
    Grid,
}

impl NorthReference {
    /// Stable token for the `lines.azimuth_north` column. Do not rename:
    /// these strings are in saved files.
    pub fn as_str(self) -> &'static str {
        match self {
            NorthReference::Magnetic => "magnetic",
            NorthReference::True => "true",
            NorthReference::Grid => "grid",
        }
    }

    /// Inverse of [`Self::as_str`]. An unrecognised token reads as `Magnetic`,
    /// which pairs with a 0 correction to mean "nothing recorded" - the
    /// pre-#438 behaviour, and the only fallback that cannot rotate a cave.
    pub fn from_token(token: &str) -> Self {
        match token {
            "true" => NorthReference::True,
            "grid" => NorthReference::Grid,
            _ => NorthReference::Magnetic,
        }
    }
}

/// Where [`AzimuthReference::correction`] came from, so the UI can explain the
/// number and a later policy change knows what it is allowed to recompute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export, export_to = "../../bindings/", rename_all = "snake_case")]
pub enum CorrectionSource {
    /// No correction is on record. Geometry is uncorrected and the UI should
    /// say so rather than implying a verified zero.
    #[default]
    Unknown,
    /// The source file declared it (Compass `DECLINATION`, Walls `DECL=`).
    FileDeclared,
    /// Derived from a geomagnetic model for `epoch_year` at the project's
    /// georeference. Reproducible: the epoch is recorded with the value.
    IgrfAuto { epoch_year: i32 },
    /// Entered by hand. Never recomputed by a policy change.
    Manual,
}

impl CorrectionSource {
    /// Stable token for the `lines.azimuth_source` column, paired with
    /// [`Self::epoch_year`] for the one variant that carries data.
    pub fn as_str(self) -> &'static str {
        match self {
            CorrectionSource::Unknown => "unknown",
            CorrectionSource::FileDeclared => "file",
            CorrectionSource::IgrfAuto { .. } => "igrf",
            CorrectionSource::Manual => "manual",
        }
    }

    /// The epoch of an [`IgrfAuto`](Self::IgrfAuto), for the nullable column.
    pub fn epoch_year(self) -> Option<i32> {
        match self {
            CorrectionSource::IgrfAuto { epoch_year } => Some(epoch_year),
            _ => None,
        }
    }

    /// Inverse of [`Self::as_str`]. An unrecognised token, or an `igrf` row with
    /// no epoch, reads as `Unknown`: the number is still applied (it is in the
    /// correction column), but nothing claims to know where it came from.
    pub fn from_token(token: &str, epoch_year: Option<i32>) -> Self {
        match (token, epoch_year) {
            ("file", _) => CorrectionSource::FileDeclared,
            ("manual", _) => CorrectionSource::Manual,
            ("igrf", Some(epoch_year)) => CorrectionSource::IgrfAuto { epoch_year },
            _ => CorrectionSource::Unknown,
        }
    }
}

/// How one line's stored azimuths relate to true north.
///
/// `Copy` on purpose: geometry hoists it out of the shot loop and passes it by
/// value into the per-shot accessor, so the whole cost of the correction is one
/// addition per shot.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct AzimuthReference {
    /// What the stored numbers are measured against. Descriptive: the UI reads
    /// it, geometry does not. [`Self::correction`] is what rotates a bearing.
    pub north: NorthReference,
    /// Degrees to ADD to a stored azimuth to get a true-north bearing. For
    /// [`NorthReference::Magnetic`] this is the declination (east positive); for
    /// [`NorthReference::Grid`] it also folds in the meridian convergence; for
    /// [`NorthReference::True`] it is 0.
    pub correction: f64,
    /// Where `correction` came from.
    pub source: CorrectionSource,
}

impl Default for AzimuthReference {
    /// "Nothing recorded": magnetic, no correction, no provenance. Byte for
    /// byte the behaviour every project had before #438, which is what makes
    /// the new columns safe to default on an existing file.
    fn default() -> Self {
        Self {
            north: NorthReference::Magnetic,
            correction: 0.0,
            source: CorrectionSource::Unknown,
        }
    }
}

impl AzimuthReference {
    /// Readings are already true north.
    pub fn true_north(source: CorrectionSource) -> Self {
        Self {
            north: NorthReference::True,
            correction: 0.0,
            source,
        }
    }

    /// Magnetic readings with a declination to add (east positive).
    pub fn magnetic(correction: f64, source: CorrectionSource) -> Self {
        Self {
            north: NorthReference::Magnetic,
            correction: Self::sanitise(correction),
            source,
        }
    }

    /// Grid readings; `correction` is declination minus grid convergence, i.e.
    /// whatever has to be ADDED to reach true north.
    pub fn grid(correction: f64, source: CorrectionSource) -> Self {
        Self {
            north: NorthReference::Grid,
            correction: Self::sanitise(correction),
            source,
        }
    }

    /// Rebuild from persisted columns, enforcing the two invariants the type
    /// guarantees in memory: the correction is finite, and a `True` reference
    /// carries no correction. A hand-edited or corrupt row therefore cannot
    /// rotate a cave.
    pub fn from_parts(north: NorthReference, correction: f64, source: CorrectionSource) -> Self {
        match north {
            NorthReference::True => Self::true_north(source),
            NorthReference::Magnetic => Self::magnetic(correction, source),
            NorthReference::Grid => Self::grid(correction, source),
        }
    }

    /// Re-impose the type's invariants on a value that arrived from OUTSIDE -
    /// deserialized off the IPC wire or read out of a `.stalx` column.
    ///
    /// `serde` builds the struct field by field and honours no constructor, so
    /// a payload can assert `north: true` with a correction of 5, or a NaN
    /// correction, and every geometry path downstream would apply it. This is
    /// the one call every ingress makes; [`Self::from_parts`] is the same rule
    /// spelled out for a caller that holds the three columns separately.
    pub fn sanitised(self) -> Self {
        Self::from_parts(self.north, self.correction, self.source)
    }

    fn sanitise(correction: f64) -> f64 {
        if correction.is_finite() {
            correction
        } else {
            0.0
        }
    }

    /// The rotation geometry actually applies, in degrees.
    pub fn applied_correction(self) -> f64 {
        Self::sanitise(self.correction)
    }

    /// True when this reference leaves every bearing untouched - the case that
    /// must stay bit-for-bit identical to the pre-#438 geometry.
    pub fn is_identity(self) -> bool {
        self.applied_correction() == 0.0
    }

    /// Measured azimuth -> true-north bearing.
    ///
    /// Degrees in `[0, 360)` **when a correction is applied**. The identity fast
    /// path returns its input UNTOUCHED, including an out-of-range or non-finite
    /// one: an uncorrected project has to walk the exact numbers it walked
    /// before this type existed, and `normalize_azimuth` is not the identity on
    /// a reading outside the range (the DMP parser stores headings verbatim, and
    /// `validation.rs` only warns). So a caller that needs a canonical value
    /// from an arbitrary reading must normalise it itself - this is a rotation,
    /// not a sanitiser.
    pub fn to_true(self, measured_degrees: f64) -> f64 {
        if self.is_identity() {
            // Not just a fast path: `normalize_azimuth` is not the identity on
            // an out-of-range reading, and an uncorrected project must walk the
            // exact same numbers it walked before this type existed.
            return measured_degrees;
        }
        normalize_azimuth(measured_degrees + self.applied_correction())
    }

    /// True-north bearing -> measured azimuth. The inverse of [`Self::to_true`],
    /// with the same range caveat: every writer of a solved bearing needs it.
    pub fn to_measured(self, true_degrees: f64) -> f64 {
        if self.is_identity() {
            return true_degrees;
        }
        normalize_azimuth(true_degrees - self.applied_correction())
    }
}

#[cfg(test)]
#[expect(
    clippy::float_cmp,
    reason = "test module: the identity fast path is exact by construction"
)]
mod tests {
    use super::*;

    #[test]
    fn nothing_recorded_is_the_pre_438_behaviour() {
        let reference = AzimuthReference::default();
        assert!(reference.is_identity());
        // Out-of-range readings pass through UNNORMALISED, exactly as a bare
        // `average_azimuth()` did before the accessor existed.
        assert_eq!(reference.to_true(400.0), 400.0);
        assert_eq!(reference.to_measured(-30.0), -30.0);
    }

    /// The sign is the whole defect: an EAST declination means magnetic north
    /// sits west of true north, so a magnetic reading is SMALLER than the true
    /// bearing and the declination is ADDED.
    #[test]
    fn east_declination_is_added_west_is_subtracted() {
        let east = AzimuthReference::magnetic(2.1, CorrectionSource::FileDeclared);
        assert!((east.to_true(10.0) - 12.1).abs() < 1e-12);
        let west = AzimuthReference::magnetic(-2.1, CorrectionSource::FileDeclared);
        assert!((west.to_true(10.0) - 7.9).abs() < 1e-12);
    }

    #[test]
    fn to_measured_inverts_to_true_across_the_wrap() {
        let reference = AzimuthReference::magnetic(-7.5, CorrectionSource::Manual);
        for measured in [0.0, 1.0, 179.9, 180.0, 350.0, 359.99] {
            let round_trip = reference.to_measured(reference.to_true(measured));
            assert!(
                (round_trip - measured).abs() < 1e-9,
                "measured {measured} round-tripped to {round_trip}"
            );
        }
    }

    #[test]
    fn correction_wraps_rather_than_escaping_the_range() {
        let reference = AzimuthReference::magnetic(5.0, CorrectionSource::Manual);
        assert!((reference.to_true(357.0) - 2.0).abs() < 1e-12);
        let west = AzimuthReference::magnetic(-5.0, CorrectionSource::Manual);
        assert!((west.to_true(2.0) - 357.0).abs() < 1e-12);
    }

    /// A corrupt row must not be able to rotate a cave: the two ways a bad
    /// number can arrive both collapse to "no correction".
    #[test]
    fn non_finite_and_true_north_never_rotate() {
        let poisoned = AzimuthReference::magnetic(f64::NAN, CorrectionSource::Unknown);
        assert!(poisoned.is_identity());
        assert_eq!(poisoned.to_true(42.0), 42.0);

        let contradictory =
            AzimuthReference::from_parts(NorthReference::True, 5.0, CorrectionSource::FileDeclared);
        assert!(contradictory.is_identity());
        assert_eq!(contradictory.correction, 0.0);
    }

    #[test]
    fn tokens_round_trip_through_the_persisted_form() {
        for north in [
            NorthReference::Magnetic,
            NorthReference::True,
            NorthReference::Grid,
        ] {
            assert_eq!(NorthReference::from_token(north.as_str()), north);
        }
        for source in [
            CorrectionSource::Unknown,
            CorrectionSource::FileDeclared,
            CorrectionSource::Manual,
            CorrectionSource::IgrfAuto { epoch_year: 2026 },
        ] {
            assert_eq!(
                CorrectionSource::from_token(source.as_str(), source.epoch_year()),
                source
            );
        }
        // An `igrf` row that lost its epoch cannot claim provenance.
        assert_eq!(
            CorrectionSource::from_token("igrf", None),
            CorrectionSource::Unknown
        );
        assert_eq!(
            NorthReference::from_token("wat"),
            NorthReference::Magnetic,
            "an unknown token must read as the no-correction default"
        );
    }
}
