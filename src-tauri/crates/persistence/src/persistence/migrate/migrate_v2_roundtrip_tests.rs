//! End to end (plan 11.1): legacy bytes written by the 0.6.0 save (the
//! frozen writer, and the two tracked fixtures an old build wrote) ->
//! `migrate` -> `.stalx` v2 `encode` -> `decode`. The decoded survey is the
//! migrated one (equal model, and the ignored warnings the format carries),
//! a second encode gives the same bytes, and every slot's raw and Closures
//! Computed View position is bit-identical before and after the round trip.

use std::path::PathBuf;

use crate::models::config::ClosureAdjustmentConfig;
use crate::models::survey::{Project, SlotPosition};

use super::super::stalx_v2::{decode, encode, DecodeOptions, Extras, IgnoredWarning, Mode};
use super::test_support::random_case;
use super::{migrate_bytes, Migrated};

/// Seeds of the legacy generator the round trip runs (the property gate's).
const SEEDS: u64 = 48;

/// The per-file content the format holds that a migration produces: the
/// ignored warnings (the rest of `MigratedExtras` is the app's to map).
fn extras_of(migrated: &Migrated) -> Extras {
    Extras {
        ignored_warnings: migrated
            .extras
            .ignored_warnings
            .iter()
            .map(|w| IgnoredWarning {
                shot: w.shot_uuid.clone(),
                code: w.code.clone(),
                at: w.resolved_at,
                comment: w.comment.clone(),
            })
            .collect(),
        ..Extras::default()
    }
}

/// Every slot's position as bits: the raw view, then the Computed View.
fn position_bits(p: &Project) -> Vec<(SlotPosition, [u64; 3])> {
    let raw = p.raw_positions();
    let adjusted = p.adjusted_positions_from(&raw, &ClosureAdjustmentConfig::default());
    raw.slots(p)
        .into_iter()
        .chain(adjusted.slots(p))
        .map(|s| (s, [s.x.to_bits(), s.y.to_bits(), s.z.to_bits()]))
        .collect()
}

fn round_trip(bytes: &[u8], what: &str) {
    let migrated = migrate_bytes(bytes).unwrap();
    assert!(migrated.report.verification.is_clean(), "{what}");
    let extras = extras_of(&migrated);
    let first = encode(&migrated.project, &extras, Mode::Normal).unwrap();
    let decoded = decode(&first, &DecodeOptions::default()).unwrap();
    assert!(decoded.read_only().is_none(), "{what}: opens writable");
    assert_eq!(decoded.project, migrated.project, "{what}: the decoded model");
    let mut ignores = extras.ignored_warnings.clone();
    ignores.sort_by(|a, b| (&a.shot, &a.code).cmp(&(&b.shot, &b.code)));
    assert_eq!(
        decoded.extras.ignored_warnings, ignores,
        "{what}: ignored warnings"
    );
    let second = encode(&decoded.project, &decoded.extras, Mode::Normal).unwrap();
    assert!(second == first, "{what}: a second encode gives other bytes");
    assert_eq!(
        position_bits(&decoded.project),
        position_bits(&migrated.project),
        "{what}: positions"
    );
}

#[test]
fn legacy_files_round_trip_through_v2_with_the_same_model_bytes_and_positions() {
    for seed in 1..=SEEDS {
        round_trip(&random_case(seed).write(), &format!("seed {seed}"));
    }
}

#[test]
fn the_tracked_fixtures_round_trip_through_v2() {
    for name in ["fractal_plant_iter3.stalx", "naharon.stalx"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../../e2e/fixtures")
            .join(name);
        round_trip(&std::fs::read(path).unwrap(), name);
    }
}
