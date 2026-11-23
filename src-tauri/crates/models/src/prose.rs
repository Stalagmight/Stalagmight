//! Prose that names stations, shots and lines (RS-PROSE): a backend text as
//! the words between the names and each name as a part the frontend draws
//! as an inline prose pill (`ProsePill`, `src/components/common/Prose.tsx`),
//! with the line's colour dot, the muted context, the home-line dot, the
//! loop mark and the superscript Station ID.
//!
//! Every text that carries parts keeps its plain string beside them, and the
//! two say the same words: [`plain`] of the parts is the string. Plain-text
//! contexts (aria labels, tooltips, logs, telemetry, files, the clipboard)
//! read the string; only the visible sentence reads the parts. A part's
//! `text` is exactly the words plain text has in its place ("Side / 12
//! (Main)", "Branch: 12 (Main) \u{2192} 13", "\"Side\""), which a screen
//! reader is given in place of the pill, so it reads the sentence it read
//! before pills.
//!
//! Builders word a text with `format!` as before and put a **marked** name
//! where a name goes ([`mark`]): the part as JSON between U+0001 and U+0002,
//! which no name holds (`sanitize_display_text` drops control characters).
//! [`parse`] splits a marked string into parts at the boundary that sends
//! it; a marked string never leaves the backend.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{LineId, StationId, StationRef};

/// Opens a marked part in a text a builder words ([`mark`]).
pub const MARK_OPEN: char = '\u{1}';
/// Closes a marked part.
pub const MARK_CLOSE: char = '\u{2}';

/// One piece of a text that names stations, shots and lines.
// `export_to` without `export` (as on `StationRef`): no export test writes
// the file, but a member-crate type exported to `src-tauri/bindings/` (the
// conversion notice's view) lists these as ts-rs dependencies, and its
// import path needs the same base.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "../../bindings/ProsePart.ts")]
pub enum ProsePart {
    /// Words between the names.
    Text { text: String },
    /// A station: its pill (`Side: 12`, the home-line dot, the loop mark,
    /// `#42`).
    Station(ProseStation),
    /// A shot: `Side: 12 \u{2192} 13`, each end with its marks.
    Shot(ProseShot),
    /// A line: its dot and its name.
    Line(ProseLine),
}

/// A station named in prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/ProseStation.ts")]
pub struct ProseStation {
    /// The words plain text has here: `12`, `Side / 12 (Main)`, `Main / 12
    /// (loop end on Side)`.
    pub text: String,
    /// The line whose colour the pill takes: the slot's line, or an
    /// identity's home line. None: a neutral pill.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_id: Option<LineId>,
    /// The muted context before the name (a line's name), when the text
    /// names the line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    /// The bare name the pill shows.
    pub name: String,
    /// A foreign slot's home line: the dot after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub home_line: Option<LineId>,
    /// A loop end: the loop mark after the name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_end: Option<bool>,
    /// The Station ID the station shows (the node id unless a loop closure
    /// shows another), as the superscript.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub station_id: Option<StationId>,
    /// The slot, when the text names one of the open project's slots now
    /// (a session handle: never persisted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub slot: Option<StationRef>,
}

/// One end of a shot named in prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/ProseShotEnd.ts")]
pub struct ProseShotEnd {
    /// The bare name.
    pub name: String,
    /// A foreign end's home line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub home_line: Option<LineId>,
    /// A loop end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub loop_end: Option<bool>,
}

/// A shot named in prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/ProseShot.ts")]
pub struct ProseShot {
    /// The words plain text has here: `Side: 12 (Main) \u{2192} 13`.
    pub text: String,
    /// The shot's line (its colour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_id: Option<LineId>,
    /// The muted context (the line's name), when the text names the line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<String>,
    pub from: ProseShotEnd,
    pub to: ProseShotEnd,
}

/// A line named in prose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "../../bindings/ProseLine.ts")]
pub struct ProseLine {
    /// The words plain text has here: `Main`, or with its quotes `"Main"`.
    pub text: String,
    /// The line (its colour), when it is one of the project's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line_id: Option<LineId>,
    /// The name the pill shows (no quotes).
    pub name: String,
}

/// `Some(true)` for a loop end, None otherwise (the wire leaves it out).
pub fn loop_flag(loop_end: bool) -> Option<bool> {
    loop_end.then_some(true)
}

impl ProsePart {
    /// The words plain text has in this part's place.
    pub fn text(&self) -> &str {
        match self {
            Self::Text { text } => text,
            Self::Station(s) => &s.text,
            Self::Shot(s) => &s.text,
            Self::Line(l) => &l.text,
        }
    }

    /// Whether the part names something (any part but words).
    pub fn is_name(&self) -> bool {
        !matches!(self, Self::Text { .. })
    }

    /// The same part without its session handle, for a text that outlives
    /// the session (a history entry).
    #[must_use]
    pub fn without_slot(self) -> Self {
        match self {
            Self::Station(s) => Self::Station(ProseStation { slot: None, ..s }),
            other => other,
        }
    }
}

/// `part` marked for a builder's `format!` ([`parse`] splits it back out).
pub fn mark(part: &ProsePart) -> String {
    match serde_json::to_string(part) {
        Ok(json) => format!("{MARK_OPEN}{json}{MARK_CLOSE}"),
        // A part is plain data and always serialises; never lose its words.
        Err(_) => part.text().to_owned(),
    }
}

/// The parts of a marked string: its words, and each marked name. Adjacent
/// words are one part. A malformed mark is read as its words without the
/// marker characters.
pub fn parse(marked: &str) -> Vec<ProsePart> {
    let mut parts = Vec::new();
    let mut words = String::new();
    let mut rest = marked;
    while let Some(open) = rest.find(MARK_OPEN) {
        words.push_str(&rest[..open]);
        let after = &rest[open + MARK_OPEN.len_utf8()..];
        let Some(close) = after.find(MARK_CLOSE) else {
            words.push_str(after);
            rest = "";
            break;
        };
        match serde_json::from_str::<ProsePart>(&after[..close]) {
            Ok(ProsePart::Text { text }) => words.push_str(&text),
            Ok(part) => {
                if !words.is_empty() {
                    parts.push(ProsePart::Text {
                        text: std::mem::take(&mut words),
                    });
                }
                parts.push(part);
            }
            Err(_) => words.push_str(&after[..close]),
        }
        rest = &after[close + MARK_CLOSE.len_utf8()..];
    }
    words.push_str(rest);
    if !words.is_empty() {
        parts.push(ProsePart::Text { text: words });
    }
    parts
}

/// The plain string of `parts`: each part's words, in order.
pub fn plain(parts: &[ProsePart]) -> String {
    parts.iter().map(ProsePart::text).collect()
}

/// The plain string of a marked string (every mark by its words).
pub fn unmark(marked: &str) -> String {
    plain(&parse(marked))
}

/// The plain string of a marked string and its parts, the parts empty when
/// it names nothing (a text with no name needs no parts on the wire).
pub fn split(marked: &str) -> (String, Vec<ProsePart>) {
    let parts = parse(marked);
    let text = plain(&parts);
    if parts.iter().any(ProsePart::is_name) {
        (text, parts)
    } else {
        (text, Vec::new())
    }
}

/// `parts` as an optional wire field (`*_parts`): `None` when they name
/// nothing, so the key stays off the wire.
pub fn named(parts: Vec<ProsePart>) -> Option<Vec<ProsePart>> {
    parts.iter().any(ProsePart::is_name).then_some(parts)
}

/// `part` marked when its words are exactly `plain`, else `plain` itself:
/// a builder that words a name its own way keeps those words, and gains the
/// pill only where the part says the same, so its plain string never
/// changes.
pub fn mark_as(plain: &str, part: Option<ProsePart>) -> String {
    match part {
        Some(part) if part.text() == plain => mark(&part),
        _ => plain.to_owned(),
    }
}

/// `parts` without their session handles ([`ProsePart::without_slot`]): a
/// text worded before an edit, whose slots may name other stations after
/// it, or one that outlives the session.
pub fn without_slots(parts: Vec<ProsePart>) -> Vec<ProsePart> {
    parts.into_iter().map(ProsePart::without_slot).collect()
}

/// A line's name as a part: `text` the words plain text has (`Main`, or
/// `"Main"` when the text quotes it), `name` the name.
pub fn line_part(line_id: Option<LineId>, name: &str, quoted: bool) -> ProsePart {
    ProsePart::Line(ProseLine {
        text: if quoted {
            format!("\"{name}\"")
        } else {
            name.to_owned()
        },
        line_id,
        name: name.to_owned(),
    })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::panic, reason = "tests")]
mod tests {
    use super::*;

    fn station(text: &str, name: &str) -> ProsePart {
        ProsePart::Station(ProseStation {
            text: text.to_owned(),
            line_id: Some(3),
            context: Some("Side".to_owned()),
            name: name.to_owned(),
            home_line: Some(1),
            loop_end: None,
            station_id: Some(42),
            slot: Some(StationRef::new(3, 0)),
        })
    }

    #[test]
    fn a_marked_string_splits_into_its_words_and_names() {
        let s = station("Side / 0 (Main)", "0");
        let marked = format!("{} is archived with its comment", mark(&s));
        let (text, parts) = split(&marked);
        assert_eq!(text, "Side / 0 (Main) is archived with its comment");
        assert_eq!(
            parts,
            vec![
                s,
                ProsePart::Text {
                    text: " is archived with its comment".to_owned()
                }
            ]
        );
    }

    #[test]
    fn plain_of_the_parts_is_the_plain_string() {
        let a = station("Side / 0", "0");
        let b = line_part(Some(1), "Main", true);
        let marked = format!("Detach line {} from {}", mark(&b), mark(&a));
        let (text, parts) = split(&marked);
        assert_eq!(text, "Detach line \"Main\" from Side / 0");
        assert_eq!(plain(&parts), text);
        assert_eq!(unmark(&marked), text);
        assert_eq!(parts.iter().filter(|p| p.is_name()).count(), 2);
    }

    #[test]
    fn a_text_without_names_has_no_parts() {
        assert_eq!(
            split("Move 3 stations"),
            ("Move 3 stations".to_owned(), Vec::new())
        );
    }

    #[test]
    fn a_malformed_mark_reads_as_its_words() {
        let (text, parts) = split("a \u{1}not json\u{2} b \u{1}open");
        assert_eq!(text, "a not json b open");
        assert!(parts.is_empty());
    }

    #[test]
    fn a_part_serialises_with_its_kind_and_without_empty_fields() {
        let part = ProsePart::Station(ProseStation {
            text: "12".to_owned(),
            line_id: None,
            context: None,
            name: "12".to_owned(),
            home_line: None,
            loop_end: loop_flag(false),
            station_id: None,
            slot: None,
        });
        assert_eq!(
            serde_json::to_string(&part).unwrap(),
            r#"{"kind":"station","text":"12","name":"12"}"#
        );
        let line = line_part(Some(2), "Main", false);
        assert_eq!(
            serde_json::to_string(&line).unwrap(),
            r#"{"kind":"line","text":"Main","line_id":2,"name":"Main"}"#
        );
    }

    #[test]
    fn mark_as_marks_a_part_only_when_its_words_are_the_plain_text() {
        let part = station("Side / 0", "0");
        assert_eq!(unmark(&mark_as("Side / 0", Some(part.clone()))), "Side / 0");
        assert_eq!(
            split(&mark_as("Side / 0", Some(part.clone()))).1,
            vec![part.clone()]
        );
        // Other words: the plain text, no part.
        assert_eq!(mark_as("Side / 00", Some(part)), "Side / 00");
        assert_eq!(mark_as("a station", None), "a station");
    }

    #[test]
    fn named_leaves_a_text_without_names_off_the_wire() {
        assert_eq!(named(Vec::new()), None);
        let words = vec![ProsePart::Text {
            text: "x".to_owned(),
        }];
        assert_eq!(named(words), None);
        let line = vec![line_part(Some(1), "Main", false)];
        assert_eq!(named(line.clone()), Some(line));
    }

    #[test]
    fn without_slots_drops_every_session_handle() {
        let parts = without_slots(vec![
            station("Side / 0", "0"),
            line_part(Some(1), "Main", false),
        ]);
        assert!(parts.iter().all(|p| match p {
            ProsePart::Station(s) => s.slot.is_none() && s.station_id == Some(42),
            _ => true,
        }));
        assert_eq!(plain(&parts), "Side / 0Main");
    }

    #[test]
    fn without_slot_drops_the_session_handle_only() {
        let ProsePart::Station(s) = station("Side / 0", "0").without_slot() else {
            panic!("a station part");
        };
        assert_eq!(s.slot, None);
        assert_eq!(s.station_id, Some(42));
    }
}
