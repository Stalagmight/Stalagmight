//! Who explored and who surveyed a shot, as Ariane records them.
//!
//! Ariane's legacy `Explorer` element is ONE field holding TWO roles, in two
//! encodings: a plain compound string (`"nh / ds"`, the form every file in the
//! #411 survey used) and an entity-escaped element pair
//! (`"<Explorer>nh</Explorer><Surveyor>ds</Surveyor>"`). Ariane 26.3 and later
//! also write separate `XMLExplorer` / `XMLSurveyor` elements; those win when
//! either is non-empty. Within a role, names are split on `,` and `;`.

use crate::models::{split_names, Person};

/// The separator the plain compound form uses between the two roles, and the
/// one [`people_line`] joins them with.
pub(super) const ROLE_SEPARATOR: &str = " / ";

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
/// * Plain form: `a / b` gives explorer `a` and surveyor `b`; a single value
///   gives explorers, as Ariane's field name and openspeleo-lib read it.
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
        let mut explorers = split_names(lead);
        for person in split_names(explorer.map_or("", |(_, text)| text)) {
            crate::models::add_person(&mut explorers, person);
        }
        return (
            explorers,
            split_names(surveyor.map_or("", |(_, text)| text)),
        );
    }
    let halves: Vec<&str> = raw.split(ROLE_SEPARATOR).collect();
    match halves.as_slice() {
        [explorers, surveyors] => (split_names(explorers), split_names(surveyors)),
        _ => (split_names(raw), Vec::new()),
    }
}

/// The shot's people: the Ariane 26.3+ `XMLExplorer`/`XMLSurveyor` pair when
/// either names somebody, else the legacy `Explorer` field.
pub(super) fn shot_people(
    legacy: Option<&str>,
    xml_explorer: Option<&str>,
    xml_surveyor: Option<&str>,
) -> (Vec<Person>, Vec<Person>) {
    let explorers = split_names(xml_explorer.unwrap_or_default());
    let surveyors = split_names(xml_surveyor.unwrap_or_default());
    if !explorers.is_empty() || !surveyors.is_empty() {
        return (explorers, surveyors);
    }
    legacy.map(parse_explorer_field).unwrap_or_default()
}

/// One display line for a shot's people: each role's names joined with `", "`,
/// the two roles joined with `" / "`, an empty role left out - the line the
/// import preview shows (`TmlSection::surveyor`, `TmlShot::explorer`).
pub fn people_line(explorers: &[Person], surveyors: &[Person]) -> Option<String> {
    let halves: Vec<String> = [explorers, surveyors]
        .iter()
        .filter_map(|people| crate::models::join_names(people))
        .collect();
    (!halves.is_empty()).then(|| halves.join(ROLE_SEPARATOR))
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
            ("Tester", &["Tester"], &[]),
            ("A, B; C", &["A", "B", "C"], &[]),
            ("unknown / ds", &[], &["ds"]),
            ("nh / None", &["nh"], &[]),
            (
                "<Explorer>nh</Explorer><Surveyor>ds</Surveyor>",
                &["nh"],
                &["ds"],
            ),
            (
                "<Explorer>unknown</Explorer><Surveyor>ds, kb</Surveyor>",
                &[],
                &["ds", "kb"],
            ),
            ("<Surveyor>ds</Surveyor>", &[], &["ds"]),
            ("lead<Explorer>nh</Explorer>", &["lead", "nh"], &[]),
            ("<Explorer>nh", &["nh"], &[]),
            ("", &[], &[]),
            ("a / b / c", &["a / b / c"], &[]),
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

    #[test]
    fn the_display_line_matches_the_old_rejoin() {
        let p = |n: &str| Person::named(n).unwrap();
        assert_eq!(
            people_line(&[p("nh")], &[p("ds")]).as_deref(),
            Some("nh / ds")
        );
        assert_eq!(people_line(&[], &[p("ds")]).as_deref(), Some("ds"));
        assert_eq!(people_line(&[p("a"), p("b")], &[]).as_deref(), Some("a, b"));
        assert_eq!(people_line(&[], &[]), None);
    }
}
