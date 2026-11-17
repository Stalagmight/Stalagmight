//! Ignored on-disk diagnostics relocated from `models::project::tests` ahead
//! of the crate split: they open a real `.stalx` through [`Database`], which
//! sits above `models`.
//!
//! [`Database`]: crate::persistence::Database

use crate::models::Project;

#[test]
#[ignore = "diagnostic against a real on-disk file; run with --ignored"]
fn diagnose_naharon_line31_bridge_closure() {
    use crate::persistence::Database;
    let path =
        std::path::Path::new("/home/user/Documents/stalagmight/20260601_naharon-final.stalx");
    let db = Database::open_existing(path).unwrap();
    let mut project = db.load_project().unwrap();
    project.rebuild_line_index();

    let count31 = |p: &Project| {
        let merges = p
            .station_merges
            .iter()
            .filter(|m| m.station_a.line_id == 31 || m.station_b.line_id == 31)
            .count();
        let closures = p
            .loop_closures
            .iter()
            .filter(|c| c.station_a.line_id == 31 || c.station_b.line_id == 31)
            .count();
        (merges, closures)
    };
    eprintln!(
        "line 31 BEFORE: merges/closures = {:?}",
        count31(&project)
    );
    let converted = project.convert_orphan_closures_to_merges();
    eprintln!("converted {converted} orphan closure(s)");
    eprintln!(
        "line 31 AFTER:  merges/closures = {:?}",
        count31(&project)
    );
    let (merges, closures) = count31(&project);
    assert_eq!(
        closures, 0,
        "line 31's sole-bridge closure should be gone"
    );
    assert!(merges >= 1, "line 31 should now be attached by a merge");
}

#[test]
#[ignore = "diagnostic against a real on-disk file; run with --ignored"]
fn diagnose_naharon_merge_alignment() {
    use crate::persistence::Database;
    let path =
        std::path::Path::new("/home/user/Documents/stalagmight/20260601_naharon-final.stalx");
    let db = Database::open_existing(path).unwrap();
    let mut project = db.load_project().unwrap();
    project.rebuild_line_index();

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
    // every line to the datum network and fixes the drift fully.
    assert!(after.is_empty(), "realign must align ALL merges");
}
