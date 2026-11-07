//! #576 RV4-R-P1: decision B2-MARKER reads ANY 2000-01-01 as "no date". The
//! date is parsed with `%Y-%m-%d`, which also accepts an unpadded month or
//! day, so the placeholder check compares the parsed date, not the raw text:
//! a file from a tool that does not zero-pad (or one edited by hand) would
//! otherwise still import "2000-1-1" as 1 January 2000, set the IGRF epoch
//! to 2000 and rotate a magnetic georeferenced survey.

use super::test_helpers::{parse_synthetic_tmlu, tml_shot_xml, wrap_xml};
use super::*;

fn date_of_text(raw: &str) -> Option<NaiveDate> {
    let mut fields = HashMap::new();
    fields.insert("date", raw.to_string());
    shot_date(&fields)
}

#[test]
fn every_spelling_of_the_placeholder_date_reads_as_undated() {
    let spellings = ["2000-01-01", "2000-1-1", "2000-01-1", "2000-1-01"];
    let read: Vec<_> = spellings
        .iter()
        .map(|raw| (*raw, date_of_text(raw)))
        .collect();
    for (raw, date) in &read {
        println!("RV4-R-P1: shot_date({raw:?}) = {date:?}");
    }
    for (raw, date) in read {
        assert_eq!(date, None, "{raw:?} is the placeholder 2000-01-01");
    }
}

#[test]
fn other_dates_and_unparsable_text_are_unchanged() {
    assert_eq!(
        date_of_text("2019-2-3"),
        NaiveDate::from_ymd_opt(2019, 2, 3)
    );
    assert_eq!(
        date_of_text("2000-01-02"),
        NaiveDate::from_ymd_opt(2000, 1, 2)
    );
    assert_eq!(
        date_of_text("1999-12-31"),
        NaiveDate::from_ymd_opt(1999, 12, 31)
    );
    assert_eq!(date_of_text("not a date"), None);
}

#[test]
fn an_unpadded_placeholder_section_imports_undated() {
    let xml = wrap_xml(
        &[
            tml_shot_xml(0, -1, "START", "Start"),
            tml_shot_xml(1, 0, "REAL", "Unpadded")
                .replace("<Date>2024-06-15</Date>", "<Date>2000-1-1</Date>"),
        ]
        .join("\n"),
    );
    let tml = parse_synthetic_tmlu(&xml);
    let date = tml
        .sections
        .iter()
        .find(|s| s.name == "Unpadded")
        .map(|s| s.date);
    println!("RV4-R-P1: section 'Unpadded' with <Date>2000-1-1</Date> imports with date {date:?}");
    assert_eq!(date, Some(None));
}
