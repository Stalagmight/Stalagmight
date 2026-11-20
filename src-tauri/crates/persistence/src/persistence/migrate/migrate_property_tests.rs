//! Property gate for the migration (plan 11.1, E7 7.1): generated legacy
//! projects, built through the frozen 0.6.0 API and written by the 0.6.0
//! save in both file shapes (and degraded the ways older builds left files),
//! are opened by the frozen reader, migrated and checked by
//! [`super::test_support::check`]: the station map is total, merged members
//! are one station (or tied), every slot is where the old build showed it,
//! measurements are bit-identical, counts and closures add up, `validate()`
//! passes and migrating twice gives the same result.
//!
//! Generator coverage (`test_support::random_case`): empty and single-shot
//! lines, merges onto earlier lines (hand moves on both members), floating
//! pieces moved by hand, datum pieces off their GPS and a frame off the
//! origin, multi-datum pieces (GPS fixes), drifted merges, merge cycles,
//! same-line, self and repeated merges, aligned snaps and torn ones, legacy
//! aligned non-`Move` closures, closures across one shot, orphan closures
//! (promoted by the load), reversed, split and combined lines, names,
//! comments and Sections on merged members, pre-v6 files without shot uuids,
//! uuids repeated across lines, pre-v8 ignores, a SpeleoDB link.
//!
//! Sweeps: `STALAGMIGHT_RUST_MIGRATE_SEEDS=<n>` seeds starting at
//! `STALAGMIGHT_RUST_MIGRATE_FIRST_SEED` (default 1). A failing seed prints
//! the line that replays it alone.

use std::panic::{catch_unwind, AssertUnwindSafe};

use stalagmight_legacy::FileShape;

use super::test_support::{check, random_case};
use crate::models::config::ClosureAdjustmentConfig;

use super::{migrate, MigrationReport};

/// Seeds the gate runs when no sweep is asked for.
const GATE_SEEDS: u64 = 48;

fn env_u64(name: &str) -> Option<u64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

/// What one seed exercised, for the gate's coverage assertion.
#[derive(Default)]
struct Coverage {
    reports: Vec<MigrationReport>,
    release_055: usize,
}

impl Coverage {
    fn total(&self, field: impl Fn(&MigrationReport) -> usize) -> usize {
        self.reports.iter().map(field).sum()
    }
}

fn run_seed(seed: u64) -> Result<(MigrationReport, bool), String> {
    let case = random_case(seed);
    let legacy = case.open();
    let migrated = migrate(&legacy, &ClosureAdjustmentConfig::default()).map_err(|e| format!("migrate: {e}"))?;
    check(&legacy, &migrated).map_err(|e| format!("{e}\n{}", migrated.report.summary()))?;
    Ok((migrated.report, case.shape == FileShape::Release055))
}

#[test]
fn migrated_legacy_projects_keep_every_station_in_place() {
    let first = env_u64("STALAGMIGHT_RUST_MIGRATE_FIRST_SEED").unwrap_or(1);
    let swept = env_u64("STALAGMIGHT_RUST_MIGRATE_SEEDS");
    let seeds = swept.unwrap_or(GATE_SEEDS);
    let mut coverage = Coverage::default();
    let mut failures = Vec::new();
    for seed in first..first + seeds {
        match catch_unwind(AssertUnwindSafe(|| run_seed(seed))) {
            Ok(Ok((report, old_shape))) => {
                coverage.release_055 += usize::from(old_shape);
                coverage.reports.push(report);
            }
            Ok(Err(why)) => failures.push((seed, why)),
            Err(_) => failures.push((seed, "panicked".to_string())),
        }
    }
    for (seed, why) in &failures {
        eprintln!(
            "migration property failed for seed {seed}: {why}\n  replay: \
             STALAGMIGHT_RUST_MIGRATE_FIRST_SEED={seed} STALAGMIGHT_RUST_MIGRATE_SEEDS=1 \
             cargo test -p stalagmight-persistence --lib \
             migrated_legacy_projects_keep_every_station_in_place"
        );
    }
    assert!(failures.is_empty(), "{} seed(s) failed", failures.len());

    // The gate's seeds must reach every conversion rule they are meant to.
    let c = &coverage;
    let checks: [(&str, usize); 21] = [
        ("merges collapsed", c.total(|r| r.merges_collapsed)),
        ("cycle merges demoted", c.total(|r| r.merges_demoted_cycle)),
        (
            "same-line merges demoted",
            c.total(|r| r.merges_demoted_same_line),
        ),
        ("self merges dropped", c.total(|r| r.merges_dropped_self)),
        (
            "repeated merges dropped",
            c.total(|r| r.merges_dropped_redundant),
        ),
        ("aliases", c.total(|r| r.aliases_created)),
        ("comments joined", c.total(|r| r.comments_joined)),
        ("stations renamed", c.total(|r| r.stations_renamed)),
        (
            "aligned closures to Tie",
            c.total(|r| r.aligned_closures_to_tie),
        ),
        ("corrections", c.total(|r| r.corrections_stored)),
        ("placements", c.total(|r| r.placements_default_gauge)),
        ("hand moves", c.total(|r| r.hand_moves_carried)),
        (
            "orphan closures promoted",
            c.total(|r| r.legacy_merges_created),
        ),
        (
            "derived shot uuids",
            c.total(|r| r.legacy_derived_shot_uuids),
        ),
        (
            "renamed shot uuids",
            c.total(|r| r.legacy_renamed_shot_uuids),
        ),
        ("ignores re-keyed", c.total(|r| r.ignores_rekeyed)),
        ("closures kept", c.total(|r| r.closures_kept)),
        ("0.5.5 file shape", c.release_055),
        (
            "datum-less pieces",
            c.total(|r| r.placements_default_gauge + r.placements_held_gauge),
        ),
        ("datums", c.total(|r| r.datums_kept)),
        ("ignores kept", c.total(|r| r.ignores_kept)),
    ];
    if swept.is_some() {
        // A sweep reports what it reached (counts only) instead of gating it.
        for (name, n) in &checks {
            eprintln!("coverage: {name} = {n}");
        }
        let snaps = c
            .reports
            .iter()
            .filter_map(|r| {
                r.verification
                    .expected
                    .get(&super::ExpectedClass::SnapAlignmentChanged)
            })
            .count();
        eprintln!("coverage: seeds with a snap alignment change = {snaps}");
        // The Closures Computed View's expected classes (plan D10, D11):
        // seeds, slots and the largest displacement, per class.
        let mut classes: std::collections::BTreeMap<&str, (usize, usize, f64)> =
            std::collections::BTreeMap::new();
        let mut compared = 0;
        for r in &c.reports {
            if let super::AdjustedCheck::Compared(a) = &r.verification.adjusted {
                compared += 1;
                for (class, stats) in &a.expected {
                    let e = classes.entry(class.as_str()).or_default();
                    e.0 += 1;
                    e.1 += stats.slots;
                    e.2 = e.2.max(stats.max_displacement_m);
                }
            }
        }
        eprintln!("coverage: Computed View compared = {compared}");
        for (class, (seeds, slots, max_m)) in &classes {
            eprintln!(
                "coverage: Computed View {class} = {seeds} seeds, {slots} slots, max {max_m:e} m"
            );
        }
        return;
    }
    let missing: Vec<&str> = checks
        .iter()
        .filter(|(_, n)| *n == 0)
        .map(|(name, _)| *name)
        .collect();
    assert!(
        missing.is_empty(),
        "the gate seeds never exercised: {missing:?}"
    );
}
