//! The bound every importer puts on a reading (#576 FZ-PARSE-5-D1).
//!
//! A reading can be finite and still too large for the geometry: the station
//! walk adds lengths and depths up, a Walls RECT vector squares its
//! components, and two readings near `f64::MAX` overflow to infinity. So each
//! importer reads a length, depth, elevation, passage dimension or length
//! correction beyond [`MAX_READING_M`] as missing - the same treatment its
//! format gives a non-finite one - and says so with the reading's location
//! (survey line, shot, point) in the warnings its import dialog shows before
//! Import is pressed. The preview and the import read the same parsed data,
//! so they agree. The core refuses an import whose computed station
//! positions are still not finite, as a backstop for a reading no importer
//! bounds.
//!
//! The bound is 10,000 km. No survey reading on Earth gets near it (a UTM
//! northing, the largest coordinate a length field can carry, tops out at
//! exactly 10,000,000 m), and with every reading inside it the sums stay
//! finite: four billion shots of 10,000 km add up to 4e16 m, and squaring
//! that is still far from overflow.
//!
//! Angles (a bearing, an inclination, a declination or an angle correction)
//! get the same treatment against [`MAX_ANGLE_DEG`]: a real one is within a
//! few turns, and inside the bound an fs/bs average or a corrected sum of them
//! cannot overflow either.

/// The largest reading, in metres, an importer accepts as measured.
pub const MAX_READING_M: f64 = 1.0e7;

/// [`MAX_READING_M`] as the warnings name it.
pub const MAX_READING_LABEL: &str = "10,000 km";

/// The largest angle, in degrees, an importer accepts as measured.
pub const MAX_ANGLE_DEG: f64 = 1.0e6;

/// [`MAX_ANGLE_DEG`] as the warnings name it.
pub const MAX_ANGLE_LABEL: &str = "1,000,000 degree";

/// The kind of reading a bound applies to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Quantity {
    /// A length, depth, elevation, passage dimension or length correction,
    /// in metres, bounded by [`MAX_READING_M`].
    Length,
    /// A bearing, inclination, declination or angle correction, in degrees,
    /// bounded by [`MAX_ANGLE_DEG`].
    Angle,
}

impl Quantity {
    /// Whether `value` is a finite reading of this kind beyond its limit. A
    /// non-finite one is not: every importer already reads it as missing,
    /// with its own warning.
    pub(crate) fn beyond_limit(self, value: f64) -> bool {
        let limit = match self {
            Self::Length => MAX_READING_M,
            Self::Angle => MAX_ANGLE_DEG,
        };
        value.is_finite() && value.abs() > limit
    }

    /// The note for one out-of-bound reading of this kind: `location: what
    /// VALUE m is beyond the 10,000 km limit no real survey reaches; outcome.`
    pub(crate) fn note(self, location: &str, what: &str, value: f64, outcome: &str) -> String {
        let (unit, label) = match self {
            Self::Length => ("m", MAX_READING_LABEL),
            Self::Angle => ("degrees", MAX_ANGLE_LABEL),
        };
        format!(
            "{location}: {what} {} {unit} is beyond the {label} limit no real survey \
             reaches; {outcome}.",
            scientific(value)
        )
    }
}

/// At most this many out-of-bound notes per file; the rest are counted in one
/// closing line, so a hostile file cannot flood the preview.
const MAX_NOTES: usize = 20;

/// Whether `metres` is a finite reading beyond [`MAX_READING_M`]. A
/// non-finite one is not: every importer already reads it as missing, with
/// its own warning.
pub(crate) fn beyond_limit(metres: f64) -> bool {
    Quantity::Length.beyond_limit(metres)
}

/// The note for one out-of-bound reading: `location: what VALUE m is beyond
/// the 10,000 km limit no real survey reaches; outcome.`
pub(crate) fn note(location: &str, what: &str, metres: f64, outcome: &str) -> String {
    Quantity::Length.note(location, what, metres, outcome)
}

/// `v` in scientific notation to five significant digits, trailing zeros
/// dropped: `1e308`, `1.7983e307`, `-2e7`.
fn scientific(v: f64) -> String {
    let full = format!("{v:.4e}");
    match full.split_once('e') {
        Some((mantissa, exponent)) if mantissa.contains('.') => {
            let mantissa = mantissa.trim_end_matches('0').trim_end_matches('.');
            format!("{mantissa}e{exponent}")
        }
        _ => full,
    }
}

/// Collects one file's out-of-bound notes, keeping the first [`MAX_NOTES`].
#[derive(Default)]
pub(crate) struct BoundNotes {
    notes: Vec<String>,
    more: usize,
}

impl BoundNotes {
    pub(crate) fn push(&mut self, note: String) {
        if self.notes.len() < MAX_NOTES {
            self.notes.push(note);
        } else {
            self.more += 1;
        }
    }

    /// The notes, then one line counting those left out, appended to `out`.
    pub(crate) fn finish_into(self, out: &mut Vec<String>) {
        out.extend(self.notes);
        if self.more > 0 {
            out.push(format!(
                "{} more readings beyond the limit no real survey reaches were read the same \
                 way.",
                self.more
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bound_is_inclusive_and_ignores_non_finite_values() {
        assert!(!beyond_limit(MAX_READING_M));
        assert!(!beyond_limit(-MAX_READING_M));
        assert!(beyond_limit(MAX_READING_M * 1.000_001));
        assert!(beyond_limit(-f64::MAX));
        assert!(!beyond_limit(f64::INFINITY));
        assert!(!beyond_limit(f64::NAN));
    }

    #[test]
    fn an_angle_has_its_own_bound_and_unit() {
        assert!(!Quantity::Angle.beyond_limit(-MAX_ANGLE_DEG));
        assert!(Quantity::Angle.beyond_limit(MAX_ANGLE_DEG * 1.000_001));
        assert!(!Quantity::Angle.beyond_limit(f64::NAN));
        assert_eq!(
            Quantity::Angle.note(
                "survey:3",
                "inclination backsight",
                -f64::MAX,
                "read as missing"
            ),
            "survey:3: inclination backsight -1.7977e308 degrees is beyond the 1,000,000 degree \
             limit no real survey reaches; read as missing."
        );
    }

    #[test]
    fn values_print_short() {
        assert_eq!(scientific(1e308), "1e308");
        assert_eq!(scientific(5.9e307 * 0.3048), "1.7983e307");
        assert_eq!(scientific(-2e7), "-2e7");
        assert_eq!(scientific(10_000_001.0), "1e7");
    }

    #[test]
    fn notes_past_the_cap_are_counted_in_one_line() {
        let mut notes = BoundNotes::default();
        for i in 0..MAX_NOTES + 3 {
            notes.push(note(
                &format!("line {i}"),
                "distance",
                1e308,
                "read as missing",
            ));
        }
        let mut out = Vec::new();
        notes.finish_into(&mut out);
        assert_eq!(out.len(), MAX_NOTES + 1);
        assert_eq!(
            out[0],
            "line 0: distance 1e308 m is beyond the 10,000 km limit no real survey reaches; \
             read as missing."
        );
        assert_eq!(
            out[MAX_NOTES],
            "3 more readings beyond the limit no real survey reaches were read the same way."
        );
    }
}
