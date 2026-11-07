//! #576 RV4-R-P2: "survey '<x>' is read from outside the project directory"
//! is said only for a listing that was actually read. A listing the budget
//! then skips (already read, or over the project's limit) was still reported
//! as read from outside - a contradictory pair with its "was skipped"
//! warning, and one extra outside warning per duplicate listing. The Compass
//! reader already warns only after a successful read.

use std::path::Path;

use super::wpj::parse_wpj_within;

const ONE_SHOT_SRV: &str = "#UNITS meters order=DAV\nA1 A2 10 90 0\n";

/// A project folder and a sibling folder holding the surveys, listed through
/// an absolute `.PATH` to the sibling: `A`, `A` again, then `B`.
fn project_reading_from_a_sibling(base: &Path) -> std::path::PathBuf {
    let proj = base.join("proj");
    let other = base.join("other");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::create_dir_all(&other).unwrap();
    std::fs::write(other.join("A.SRV"), ONE_SHOT_SRV).unwrap();
    std::fs::write(other.join("B.SRV"), ONE_SHOT_SRV).unwrap();
    let wpj = format!(
        ";WALLS Project File\n\
         .BOOK Root\n.NAME ROOT\n.STATUS 8\n\
         .PATH {}\n\
         .SURVEY First\n.NAME A\n.STATUS 8\n\
         .SURVEY Again\n.NAME A\n.STATUS 8\n\
         .SURVEY Other\n.NAME B\n.STATUS 8\n\
         .ENDBOOK\n",
        other.display()
    );
    let wpj_path = proj.join("T.wpj");
    std::fs::write(&wpj_path, wpj).unwrap();
    wpj_path
}

#[test]
fn only_a_survey_that_was_read_is_reported_as_read_from_outside() {
    let dir = tempfile::tempdir().unwrap();
    let wpj_path = project_reading_from_a_sibling(dir.path());
    // Room for one survey file only: A is read, A again is a repeat, B is
    // over the budget.
    let budget = ONE_SHOT_SRV.len() as u64 + 1;
    let parsed = parse_wpj_within(&wpj_path, budget).unwrap();
    for w in &parsed.warnings {
        println!("RV4-R-P2 warning: {w}");
    }
    let read: Vec<&str> = parsed
        .surveys
        .iter()
        .filter(|s| s.resolved && !s.vectors.is_empty())
        .map(|s| s.name.as_str())
        .collect();
    println!("RV4-R-P2 read: {read:?}");
    let outside_a = parsed
        .warnings
        .iter()
        .filter(|w| w.starts_with("survey 'A' is read from outside"))
        .count();
    let outside_b = parsed
        .warnings
        .iter()
        .filter(|w| w.starts_with("survey 'B' is read from outside"))
        .count();
    println!("RV4-R-P2 outside warnings: A x{outside_a} (expected 1), B x{outside_b} (expected 0)");
    assert_eq!(read, ["A"]);
    assert_eq!(outside_a, 1, "A was read once: {:?}", parsed.warnings);
    assert_eq!(
        outside_b, 0,
        "B was skipped, so it was not read from anywhere: {:?}",
        parsed.warnings
    );
    assert!(
        parsed
            .warnings
            .iter()
            .any(|w| w.starts_with("survey 'B' was skipped")),
        "{:?}",
        parsed.warnings
    );
}
