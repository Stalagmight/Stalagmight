//! Synthetic survey content for the fuzzer: every project a run creates,
//! edits or seeds is built here from section names and shot counts, so no
//! real survey data ever enters a run.

use std::collections::BTreeMap;
use std::path::Path;

use crate::models::{Project, SectionData, ShotData};
use crate::persistence::Database;

/// A survey's content as the invariants compare it: section name -> shot count.
pub(super) type Fp = BTreeMap<String, usize>;

/// The fingerprint of a loaded project.
pub(super) fn fingerprint(project: &Project) -> Fp {
    project
        .sections
        .iter()
        .map(|s| (s.name.clone().unwrap_or_default(), s.shots.len()))
        .collect()
}

/// A shot the fuzz edits and seeds use; `n` varies the geometry.
pub(super) fn shot(n: usize) -> ShotData {
    let az = f64::from(u32::try_from(n % 36).unwrap_or(0)) * 10.0;
    ShotData {
        length: 3.0 + f64::from(u32::try_from(n % 5).unwrap_or(0)),
        azimuth_in: az,
        azimuth_out: (az + 180.0) % 360.0,
        depth_in: 3.0,
        depth_out: 3.0,
        ..Default::default()
    }
}

/// A section with `shots` shots named `name`.
pub(super) fn section(name: &str, shots: usize) -> SectionData {
    SectionData {
        name: Some(name.to_owned()),
        surveyor: None,
        date: None,
        color: "#4ecdc4".to_owned(),
        direction: None,
        quality: 1.0,
        shots: (0..shots).map(shot).collect(),
    }
}

/// A synthetic project holding `sections`.
pub(super) fn project_with(sections: &[(String, usize)]) -> Project {
    let mut project = Project::new("FuzzCave".to_owned(), None);
    for (name, shots) in sections {
        project.add_section(section(name, *shots));
    }
    project
}

/// Parse a section spec such as `A:3,B:2,C:2` (name:shots, comma-separated).
/// `None` when any entry is malformed or the list is empty.
pub(super) fn parse_sections(spec: &str) -> Option<Vec<(String, usize)>> {
    let parsed: Option<Vec<(String, usize)>> = spec
        .split(',')
        .map(|entry| {
            let (name, shots) = entry.trim().split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            Some((name.to_owned(), shots.trim().parse().ok()?))
        })
        .collect();
    parsed.filter(|list| !list.is_empty())
}

/// Write `sections` as a fresh `.stalx` at `path` with the persistence layer
/// (replacing any file there).
pub(super) fn write_stalx(path: &Path, sections: &[(String, usize)]) -> Result<(), String> {
    let _ = std::fs::remove_file(path);
    let mut db = Database::open(path).map_err(|e| format!("create {}: {e:#}", path.display()))?;
    db.save_project(&project_with(sections))
        .map_err(|e| format!("save {}: {e:#}", path.display()))
}

/// Read a `.stalx` from disk without writing to it. `Err` carries why not (a
/// newer-format file, a half-written file).
pub(super) fn fingerprint_file(path: &Path) -> Result<Fp, String> {
    let db = Database::open_readonly(path).map_err(|e| format!("open: {e:#}"))?;
    let project = db.load_project().map_err(|e| format!("load: {e:#}"))?;
    Ok(fingerprint(&project))
}

#[test]
fn a_section_spec_parses_and_rejects_garbage() {
    assert_eq!(
        parse_sections("A:3, B:2,C:0"),
        Some(vec![
            ("A".to_owned(), 3),
            ("B".to_owned(), 2),
            ("C".to_owned(), 0)
        ])
    );
    for bad in ["", "A", "A:x", ":3", "A:3,,B:2"] {
        assert_eq!(parse_sections(bad), None, "{bad:?}");
    }
}

#[test]
fn a_written_seed_reads_back_with_its_sections() {
    let dir = tempfile::tempdir().expect("tmp");
    let path = dir.path().join("seed.stalx");
    let spec = parse_sections("A:3,B:2,C:2,D:4").expect("spec");
    write_stalx(&path, &spec).expect("write");
    let fp = fingerprint_file(&path).expect("read back");
    assert_eq!(fp, spec.into_iter().collect::<Fp>());
}
