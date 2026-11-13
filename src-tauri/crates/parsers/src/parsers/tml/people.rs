//! Who explored and who surveyed a shot, as Ariane records them.
//!
//! Ariane's legacy `Explorer` element is ONE field holding TWO roles, in two
//! encodings: a plain compound string (`"nh / ds"`, the form every file in the
//! #411 survey used) and an entity-escaped element pair
//! (`"<Explorer>nh</Explorer><Surveyor>ds</Surveyor>"`). Ariane 26.3 and later
//! also write separate `XMLExplorer` / `XMLSurveyor` elements; those win when
//! either is non-empty.
//!
//! The plain form is read by the SAME splitter as a `.stalx` file's legacy
//! `sections.surveyor` column ([`legacy_people`]), so one text names the same
//! people in the same roles on both paths: a single value with no `" / "`
//! is surveyors, which is what every saved file and SpeleoDB's reader have
//! always made of it. Within a role, names are split by [`split_people`]
//! (`,` and `;`, except that `Last, First` is one person).

use crate::models::{legacy_people, split_people, Person};

/// The text of `<tag>...</tag>` inside an already-unescaped field value, with
/// the byte offset of its OPENING tag so the caller can see what came before it.
fn tagged_half<'a>(raw: &'a str, tag: &str) -> Option<(usize, &'a str)> {
    let open = format!("<{tag}>");
    let opens_at = raw.find(&open)?;
    let rest = &raw[opens_at + open.len()..];
    // An unterminated half keeps everything after the open tag.
    Some((
        opens_at,
        rest.split_once(format!("</{tag}>").as_str())
            .map_or(rest, |(text, _)| text),
    ))
}

/// `(explorers, surveyors)` of Ariane's legacy `Explorer` field.
///
/// * Element form: each half goes to its role. Text before the first tag is a
///   name that lost its element; it is kept, as an explorer (the field's own
///   role).
/// * Plain form, read exactly like the legacy column ([`legacy_people`]):
///   `a / b` gives explorer `a` and surveyor `b`, `a /` explorer `a` alone, and
///   any other text - a single name included - surveyors.
///
/// Placeholders (`unknown`, `none`, `?`) and empty names are dropped per name,
/// so an unfilled explorer half never takes the surveyor with it (#427).
pub(super) fn parse_explorer_field(raw: &str) -> (Vec<Person>, Vec<Person>) {
    if raw.contains("<Explorer>") || raw.contains("<Surveyor>") {
        let explorer = tagged_half(raw, "Explorer");
        let surveyor = tagged_half(raw, "Surveyor");
        let first_tag = [explorer, surveyor]
            .iter()
            .flatten()
            .map(|(at, _)| *at)
            .min()
            .unwrap_or(0);
        let lead = &raw[..first_tag];
        let mut explorers = split_people(lead);
        for person in split_people(explorer.map_or("", |(_, text)| text)) {
            crate::models::add_person(&mut explorers, person);
        }
        return (
            explorers,
            split_people(surveyor.map_or("", |(_, text)| text)),
        );
    }
    legacy_people(raw)
}

/// The shot's people: the Ariane 26.3+ `XMLExplorer`/`XMLSurveyor` pair when
/// either names somebody, else the legacy `Explorer` field.
pub(super) fn shot_people(
    legacy: Option<&str>,
    xml_explorer: Option<&str>,
    xml_surveyor: Option<&str>,
) -> (Vec<Person>, Vec<Person>) {
    let explorers = split_people(xml_explorer.unwrap_or_default());
    let surveyors = split_people(xml_surveyor.unwrap_or_default());
    if !explorers.is_empty() || !surveyors.is_empty() {
        return (explorers, surveyors);
    }
    legacy.map(parse_explorer_field).unwrap_or_default()
}

/// One line for a shot's people (`TmlShot::explorer`, and through it
/// `TmlSection::surveyor`, the section's legacy template): the legacy column's
/// text ([`crate::models::legacy_credit_text`]), so [`legacy_people`] reads it
/// back into the same roles - `"explorers / surveyors"`, `"explorers /"`, or
/// the surveyors alone.
pub fn people_line(explorers: &[Person], surveyors: &[Person]) -> Option<String> {
    crate::models::legacy_credit_text(explorers, surveyors)
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn names(people: &[Person]) -> Vec<&str> {
        people.iter().map(|p| p.name.as_str()).collect()
    }

    #[test]
    fn the_legacy_field_in_both_encodings() {
        // (raw, explorers, surveyors)
        let cases: &[(&str, &[&str], &[&str])] = &[
            ("nh / ds", &["nh"], &["ds"]),
            // A single value is surveyors, as in a .stalx legacy column.
            ("Tester", &[], &["Tester"]),
            ("A, B; C", &[], &["A", "B", "C"]),
            // `Last, First` is one person on this path too.
            ("Doe, John", &[], &["Doe, John"]),
            ("Doe, John / Ann Lee", &["Doe, John"], &["Ann Lee"]),
            // Explorers only, as this app writes them in the plain form.
            ("nh /", &["nh"], &[]),
            ("unknown / ds", &[], &["ds"]),
            ("nh / None", &["nh"], &[]),
            (
                "<Explorer>nh</Explorer><Surveyor>ds</Surveyor>",
                &["nh"],
                &["ds"],
            ),
            (
                "<Explorer>unknown</Explorer><Surveyor>ds, kb; ab</Surveyor>",
                &[],
                &["ds", "kb", "ab"],
            ),
            // Exactly two one-word names read as one `Last, First` person, as
            // in the legacy column: one splitter on both paths.
            (
                "<Explorer>unknown</Explorer><Surveyor>ds, kb</Surveyor>",
                &[],
                &["ds, kb"],
            ),
            ("<Surveyor>ds</Surveyor>", &[], &["ds"]),
            ("lead<Explorer>nh</Explorer>", &["lead", "nh"], &[]),
            ("<Explorer>nh", &["nh"], &[]),
            ("", &[], &[]),
            ("a / b / c", &[], &["a / b / c"]),
            (
                "<Explorer>Doe, John</Explorer><Surveyor>Ann; Bo</Surveyor>",
                &["Doe, John"],
                &["Ann", "Bo"],
            ),
        ];
        for (raw, explorers, surveyors) in cases {
            let (e, s) = parse_explorer_field(raw);
            assert_eq!(names(&e), *explorers, "explorers of {raw:?}");
            assert_eq!(names(&s), *surveyors, "surveyors of {raw:?}");
        }
    }

    #[test]
    fn the_xml_pair_wins_when_it_names_anybody() {
        let (e, s) = shot_people(Some("old / older"), Some("New"), Some("Newer; Newest"));
        assert_eq!(
            (names(&e), names(&s)),
            (vec!["New"], vec!["Newer", "Newest"])
        );
        let (e, s) = shot_people(Some("old / older"), Some(""), Some("unknown"));
        assert_eq!((names(&e), names(&s)), (vec!["old"], vec!["older"]));
        assert_eq!(shot_people(None, None, None), (Vec::new(), Vec::new()));
    }

    /// The line is the legacy column's text, so the legacy splitter (which
    /// reads it as the section template) gives every role back.
    #[test]
    fn the_people_line_reads_back_into_the_same_roles() {
        let p = |n: &str| Person::named(n).unwrap();
        assert_eq!(
            people_line(&[p("nh")], &[p("ds")]).as_deref(),
            Some("nh / ds")
        );
        assert_eq!(people_line(&[], &[p("ds")]).as_deref(), Some("ds"));
        assert_eq!(
            people_line(&[p("Ann Lee"), p("Bo Ray")], &[]).as_deref(),
            Some("Ann Lee, Bo Ray /")
        );
        assert_eq!(people_line(&[], &[]), None);
        let (e, s) = legacy_people(&people_line(&[p("Ann Lee"), p("Bo Ray")], &[]).unwrap());
        assert_eq!((names(&e), names(&s)), (vec!["Ann Lee", "Bo Ray"], vec![]));
    }
}
