//! Ignored on-disk diagnostics relocated from `models::project::tests` ahead
//! of the crate split: they open a real `.stalx` through [`Database`], which
//! sits above `models`.
//!
//! The file is the caller's, named by `STALAGMIGHT_DIAG_STALX`; run without it,
//! they fail naming the variable (#576 E13, NF-PRIVATE-PATHS):
//!
//! ```text
//! STALAGMIGHT_DIAG_STALX=/path/to/project.stalx STALAGMIGHT_DIAG_SECTION=<id> \
//!   cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-persistence \
//!   --lib diagnose_ -- --ignored --nocapture
//! ```
//!
//! [`Database`]: crate::persistence::Database

use crate::models::test_support::{require_env, require_env_file};
use crate::models::Project;

/// The project the diagnostics inspect, loaded from `STALAGMIGHT_DIAG_STALX`.
fn diagnosed_project() -> Project {
    use crate::persistence::Database;
    let path = require_env_file("STALAGMIGHT_DIAG_STALX");
    let db = Database::open_existing(&path).unwrap();
    let mut project = db.load_project().unwrap();
    project.rebuild_section_index();
    project
}

/// A section whose only link to the rest of the survey is a loop closure
/// (`STALAGMIGHT_DIAG_SECTION`) is attached by a merge once orphan closures
/// are converted.
#[test]
#[ignore = "diagnostic against a real on-disk file; set STALAGMIGHT_DIAG_STALX and STALAGMIGHT_DIAG_SECTION"]
fn diagnose_a_sole_bridge_closure_becomes_a_merge() {
    let section: u32 = require_env("STALAGMIGHT_DIAG_SECTION").parse().unwrap();
    let mut project = diagnosed_project();

    let count = |p: &Project| {
        let merges = p
            .station_merges
            .iter()
            .filter(|m| m.station_a.section_id == section || m.station_b.section_id == section)
            .count();
        let closures = p
            .loop_closures
            .iter()
            .filter(|c| c.station_a.section_id == section || c.station_b.section_id == section)
            .count();
        (merges, closures)
    };
    eprintln!(
        "section {section} BEFORE: merges/closures = {:?}",
        count(&project)
    );
    let converted = project.convert_orphan_closures_to_merges();
    eprintln!("converted {converted} orphan closure(s)");
    eprintln!(
        "section {section} AFTER:  merges/closures = {:?}",
        count(&project)
    );
    let (merges, closures) = count(&project);
    assert_eq!(
        closures, 0,
        "section {section}'s sole-bridge closure should be gone"
    );
    assert!(
        merges >= 1,
        "section {section} should now be attached by a merge"
    );
}

#[test]
#[ignore = "diagnostic against a real on-disk file; set STALAGMIGHT_DIAG_STALX"]
fn diagnose_merge_alignment() {
    let mut project = diagnosed_project();

    let before = project.check_merge_alignment();
    eprintln!("== misaligned merges BEFORE: {} ==", before.len());
    for v in &before {
        eprintln!("  {}", v.message);
    }

    let fixed = project.realign_merges();
    eprintln!("== realign_merges fixed {fixed} merge(s) ==");
    let after = project.check_merge_alignment();
    eprintln!("== misaligned merges AFTER: {} ==", after.len());
    for v in &after {
        eprintln!("  {}", v.message);
    }
    // The cascade propagates through already-aligned merges, so realign anchors
    // every section to the datum network and fixes the drift fully.
    assert!(after.is_empty(), "realign must align ALL merges");
}
