//! #576 NF-FIX3R-2: the `.SRV` files one `.WPJ` lists are read once each and
//! capped in total, like the `.DAT` files of a Compass `.MAK` (RV2-R-12).
//! Synthetic projects, written to a temp dir by each test.

use std::fmt::Write as _;
use std::path::Path;

use crate::parsers::linked_files::MAX_LINKED_BYTES;

use super::wpj::parse_wpj_within;
use super::{parse_walls, ParsedWalls};

/// A `.WPJ` with one `.SURVEY` per `(title, name)`.
fn wpj(surveys: &[(&str, &str)]) -> String {
    let mut text = String::from(".BOOK Synthetic Project\n.NAME PROJ\n");
    for (title, name) in surveys {
        writeln!(text, ".SURVEY {title}\n.NAME {name}").unwrap();
    }
    text.push_str(".ENDBOOK\n");
    text
}

fn srv(from: &str, to: &str) -> String {
    format!("#UNITS meters order=DAV\n{from} {to} 10 90 0\n")
}

fn resolved(parsed: &ParsedWalls) -> Vec<String> {
    parsed
        .surveys
        .iter()
        .filter(|s| s.resolved)
        .map(|s| format!("{} ({} shots)", s.name, s.vectors.len()))
        .collect()
}

fn write(dir: &Path, name: &str, text: &str) {
    std::fs::write(dir.join(name), text).unwrap();
}

/// A `.WPJ` listing the same `.SRV` twice - here under two spellings, `A`
/// and `A.SRV` - read and imported it twice. It is read once; the further
/// listing is skipped with a warning naming it.
#[test]
fn a_survey_file_listed_twice_is_read_once() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "A.SRV", &srv("A1", "A2"));
    write(dir.path(), "B.SRV", &srv("B1", "B2"));
    let project = dir.path().join("project.wpj");
    write(
        dir.path(),
        "project.wpj",
        &wpj(&[("First", "A"), ("Again", "A.SRV"), ("Other", "B")]),
    );
    let parsed = parse_walls(&project).unwrap();
    let read = resolved(&parsed);
    let repeated: Vec<&String> = parsed
        .warnings
        .iter()
        .filter(|w| w.contains("is listed more than once"))
        .collect();
    println!(
        "surveys read: {} {read:?} (expected 2); repeat warnings: {repeated:?}",
        read.len()
    );
    assert_eq!(read, ["A (1 shots)", "B (1 shots)"]);
    assert_eq!(repeated.len(), 1, "{:?}", parsed.warnings);
    assert!(
        repeated[0].starts_with("survey 'A.SRV'"),
        "names the skipped listing: {repeated:?}"
    );
}

/// The `.SRV` files one `.WPJ` lists are capped in TOTAL, not only one by
/// one. A survey that would take the project past the limit is skipped and
/// named; the ones before it import. (The real limit is 512 MB, the Compass
/// `.MAK` one; the budget here is two files' worth.)
#[test]
fn the_survey_files_share_one_byte_budget() {
    let dir = tempfile::tempdir().unwrap();
    let mut size = 0;
    for (name, from, to) in [
        ("A.SRV", "A1", "A2"),
        ("B.SRV", "B1", "B2"),
        ("C.SRV", "C1", "C2"),
    ] {
        let text = srv(from, to);
        size = text.len() as u64;
        write(dir.path(), name, &text);
    }
    let project = dir.path().join("project.wpj");
    write(
        dir.path(),
        "project.wpj",
        &wpj(&[("First", "A"), ("Second", "B"), ("Third", "C")]),
    );
    assert_eq!(MAX_LINKED_BYTES, 512 * 1024 * 1024);
    let parsed = parse_wpj_within(&project, 2 * size + 1).unwrap();
    let read = resolved(&parsed);
    let refused: Vec<&String> = parsed
        .warnings
        .iter()
        .filter(|w| w.contains("limit for one project"))
        .collect();
    println!(
        "surveys read with a {} byte budget: {} {read:?} (expected 2); refused: {refused:?}",
        2 * size + 1,
        read.len()
    );
    assert_eq!(read, ["A (1 shots)", "B (1 shots)"]);
    assert_eq!(refused.len(), 1, "{:?}", parsed.warnings);
    assert!(
        refused[0].starts_with("survey 'C' was skipped"),
        "{refused:?}"
    );
}
