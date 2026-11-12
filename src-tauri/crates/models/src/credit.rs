//! Per-shot survey credit (#707): who surveyed a shot and when, when that
//! differs from its section.
//!
//! A section records one surveyor and one date. Combining two sections from
//! different trips would otherwise credit one trip's shots to the other trip's
//! surveyor, so a shot can override either value. Each field has three states:
//!
//! | Field value        | Meaning                    | On disk (`shot_credits`) |
//! |--------------------|----------------------------|--------------------------|
//! | `None`             | same as the section        | `NULL`                   |
//! | `Some(None)`       | explicitly unknown         | `''`                     |
//! | `Some(Some(v))`    | this shot's own value      | the value                |
//!
//! "Explicitly unknown" exists so an unattributed section joining an
//! attributed one does not inherit the other section's surveyor.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::shot::double_option;

/// A shot's overrides of its section's surveyor and date. `Shot::credit` is
/// `None` when both are inherited, which is what every shot loaded from a file
/// written before #707 has; read it through `Shot::credit()`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ShotCredit {
    /// `None`: the section's surveyor. `Some(None)`: unknown. `Some(Some(s))`: `s`.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional, type = "string | null")]
    pub surveyor: Option<Option<String>>,
    /// `None`: the section's date. `Some(None)`: unknown. `Some(Some(d))`: `d`.
    #[serde(
        default,
        deserialize_with = "double_option",
        skip_serializing_if = "Option::is_none"
    )]
    #[ts(optional, type = "string | null")]
    pub date: Option<Option<NaiveDate>>,
}

impl ShotCredit {
    /// Both values from the section. `Shot::credit` is `None` in this state, so
    /// a shot never stores it; this is what the accessor lends out instead.
    pub const INHERITED: ShotCredit = ShotCredit {
        surveyor: None,
        date: None,
    };

    /// True when the shot takes both values from its section.
    pub fn is_inherited(&self) -> bool {
        self.surveyor.is_none() && self.date.is_none()
    }

    /// The form a `Shot` stores: `None` when both values are inherited, so
    /// "no overrides" has exactly one representation.
    pub fn into_stored(self) -> Option<ShotCredit> {
        (!self.is_inherited()).then_some(self)
    }

    /// The surveyor to credit, given the section's surveyor.
    pub fn surveyor_or<'a>(&'a self, section: Option<&'a str>) -> Option<&'a str> {
        match &self.surveyor {
            None => section,
            Some(value) => value.as_deref(),
        }
    }

    /// The survey date to credit, given the section's date.
    pub fn date_or(&self, section: Option<NaiveDate>) -> Option<NaiveDate> {
        match self.date {
            None => section,
            Some(value) => value,
        }
    }

    /// The `shot_credits.surveyor` column value: `NULL`, `''` for unknown, or the name.
    pub fn surveyor_column(&self) -> Option<String> {
        self.surveyor
            .as_ref()
            .map(|value| value.clone().unwrap_or_default())
    }

    /// The `shot_credits.date` column value: `NULL`, `''` for unknown, or `YYYY-MM-DD`.
    pub fn date_column(&self) -> Option<String> {
        self.date.map(|value| {
            value
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default()
        })
    }

    /// Rebuild from the two `shot_credits` columns. An empty string is "unknown". A
    /// date that does not parse is also read as unknown: crediting the shot to
    /// its section's date instead would invent a date the file never stated.
    pub fn from_columns(surveyor: Option<String>, date: Option<String>) -> Self {
        Self {
            surveyor: surveyor.map(|s| Some(s).filter(|s| !s.is_empty())),
            date: date.map(|d| NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()),
        }
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: panics on a failed assumption are the point"
)]
mod tests {
    use super::*;

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn inherited_credit_takes_the_section_values() {
        let credit = ShotCredit::default();
        assert!(credit.is_inherited());
        assert_eq!(credit.surveyor_or(Some("Bob")), Some("Bob"));
        assert_eq!(credit.date_or(Some(day(2026, 3, 2))), Some(day(2026, 3, 2)));
    }

    #[test]
    fn unknown_credit_does_not_fall_back_to_the_section() {
        let credit = ShotCredit {
            surveyor: Some(None),
            date: Some(None),
        };
        assert!(!credit.is_inherited());
        assert_eq!(credit.surveyor_or(Some("Bob")), None);
        assert_eq!(credit.date_or(Some(day(2026, 3, 2))), None);
    }

    #[test]
    fn own_credit_overrides_the_section() {
        let credit = ShotCredit {
            surveyor: Some(Some("Ann".to_string())),
            date: Some(Some(day(2026, 4, 11))),
        };
        assert_eq!(credit.surveyor_or(Some("Bob")), Some("Ann"));
        assert_eq!(credit.date_or(None), Some(day(2026, 4, 11)));
    }

    #[test]
    fn columns_round_trip_all_three_states() {
        for credit in [
            ShotCredit::default(),
            ShotCredit {
                surveyor: Some(None),
                date: Some(None),
            },
            ShotCredit {
                surveyor: Some(Some("Ann".to_string())),
                date: Some(Some(day(2026, 4, 11))),
            },
        ] {
            let back = ShotCredit::from_columns(credit.surveyor_column(), credit.date_column());
            assert_eq!(back, credit);
        }
    }

    #[test]
    fn an_unparseable_date_column_reads_as_unknown_not_inherited() {
        let credit = ShotCredit::from_columns(None, Some("not a date".to_string()));
        assert_eq!(credit.date, Some(None));
    }

    #[test]
    fn json_omits_inherited_fields_and_keeps_null_for_unknown() {
        assert_eq!(serde_json::to_string(&ShotCredit::default()).unwrap(), "{}");
        let unknown = ShotCredit {
            surveyor: Some(None),
            date: None,
        };
        let json = serde_json::to_string(&unknown).unwrap();
        assert_eq!(json, r#"{"surveyor":null}"#);
        assert_eq!(serde_json::from_str::<ShotCredit>(&json).unwrap(), unknown);
        assert_eq!(
            serde_json::from_str::<ShotCredit>("{}").unwrap(),
            ShotCredit::default()
        );
    }
}
