//! [#576 RV14-2] A creation time, kept as the text it was stored as.
//!
//! The `.stalx` `sections.created_at` and `project.created_at` columns hold RFC
//! 3339 text. What this app writes is always `to_rfc3339()` (UTC, `+00:00`), but
//! a file another tool wrote can hold the same kind of value in another form:
//! another offset (`+05:00`), a `Z`, other fractional digits. Read as a UTC time
//! and printed again by the next save, it came back as the same moment in another
//! text, in a column no edit had touched. A stamp keeps the text it was read
//! from, byte for byte, and the save writes that back; only a section with no
//! readable stamp gets a new one. [#576 NF-W18-1] The project's own creation time
//! is one too; the moment it names ([`CreationStamp::utc`]) is what identifies
//! the project to the frontend, so that identity is the same however it is
//! written.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// When a section or a project was created ([`crate::Section::created_at`],
/// [`crate::Project::created_at`]): an RFC 3339 time, held as the exact text it
/// was read from or first stamped as, with the moment that text names.
///
/// Serialized as that text, so an undo snapshot carries it unchanged. Read back,
/// it takes any RFC 3339 text - which is what the fields' earlier type,
/// `DateTime<Utc>`, wrote - and refuses anything else, as that type did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct CreationStamp {
    text: String,
    utc: DateTime<Utc>,
}

impl CreationStamp {
    /// `text` as a stamp, unchanged, when it is an RFC 3339 time; `None` when it
    /// is not, and the next save stamps the section anew.
    pub fn parse(text: &str) -> Option<Self> {
        Self::read(text.to_owned()).ok()
    }

    /// `text` as a stamp, unchanged; the parse error when it is not an RFC 3339
    /// time (a project's unreadable creation time refuses the open, as it did).
    pub fn read(text: String) -> Result<Self, chrono::ParseError> {
        let utc = DateTime::parse_from_rfc3339(&text)?.with_timezone(&Utc);
        Ok(Self { text, utc })
    }

    /// A new stamp of `at`, in the form this app writes.
    pub fn at(at: DateTime<Utc>) -> Self {
        Self {
            text: at.to_rfc3339(),
            utc: at,
        }
    }

    /// The text: as it was stored, or as this app stamped it.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The moment the text names, in UTC: the same for every form of it.
    pub fn utc(&self) -> DateTime<Utc> {
        self.utc
    }
}

impl TryFrom<String> for CreationStamp {
    type Error = String;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        match DateTime::parse_from_rfc3339(&text) {
            Ok(at) => Ok(Self {
                utc: at.with_timezone(&Utc),
                text,
            }),
            Err(e) => Err(format!("not an RFC 3339 time ({e}): {text:?}")),
        }
    }
}

impl From<CreationStamp> for String {
    fn from(stamp: CreationStamp) -> Self {
        stamp.text
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    reason = "test module: a failed assumption panics"
)]
mod tests {
    use super::CreationStamp;
    use chrono::{TimeZone, Utc};

    #[test]
    fn a_readable_text_is_kept_byte_for_byte_and_anything_else_is_no_stamp() {
        for text in [
            "2020-01-02T03:04:05+05:00",
            "2020-01-02T03:04:05Z",
            "2020-01-02T03:04:05.120-03:30",
        ] {
            assert_eq!(CreationStamp::parse(text).unwrap().as_str(), text);
        }
        for text in ["yesterday", "", "2020-01-02 03:04:05"] {
            assert_eq!(CreationStamp::parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn a_new_stamp_is_written_as_this_app_writes_times() {
        let at = Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap();
        assert_eq!(CreationStamp::at(at).as_str(), "2020-01-02T03:04:05+00:00");
    }

    /// Serde carries the text unchanged (an undo snapshot), reads what the
    /// field's earlier type `DateTime<Utc>` wrote, and refuses what it refused.
    #[test]
    fn serde_carries_the_text_and_reads_the_earlier_form() {
        let stamp = CreationStamp::parse("2020-01-02T03:04:05+05:00").unwrap();
        let json = serde_json::to_string(&stamp).unwrap();
        assert_eq!(json, "\"2020-01-02T03:04:05+05:00\"");
        assert_eq!(serde_json::from_str::<CreationStamp>(&json).unwrap(), stamp);

        let earlier =
            serde_json::to_string(&Utc.with_ymd_and_hms(2020, 1, 2, 3, 4, 5).unwrap()).unwrap();
        let read: CreationStamp = serde_json::from_str(&earlier).unwrap();
        assert_eq!(format!("\"{}\"", read.as_str()), earlier);

        assert!(serde_json::from_str::<CreationStamp>("\"yesterday\"").is_err());
    }

    /// [#576 NF-W18-1] Every form of one moment names the same UTC time - the
    /// project identity the frontend holds - and `read` refuses what `parse` does.
    #[test]
    fn every_form_of_a_moment_names_the_same_utc_time() {
        let at = Utc.with_ymd_and_hms(2020, 1, 1, 22, 4, 5).unwrap();
        for text in [
            "2020-01-02T03:04:05+05:00",
            "2020-01-01T22:04:05Z",
            "2020-01-01T22:04:05.000+00:00",
        ] {
            let stamp = CreationStamp::read(text.to_string()).unwrap();
            assert_eq!((stamp.as_str(), stamp.utc()), (text, at));
        }
        assert_eq!(CreationStamp::at(at).utc(), at);
        assert!(CreationStamp::read("yesterday".to_string()).is_err());
    }
}
