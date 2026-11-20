//! (b) The two tracked old fixtures open, deterministically. Only counts and
//! determinism are asserted; nothing of their contents is printed.
#![expect(clippy::unwrap_used, reason = "test setup and assertions")]

use std::path::PathBuf;

use stalagmight_legacy::{open_path, LegacySurvey, OpenOptions, Positions, Slot};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../e2e/fixtures")
        .join(name)
}

fn bits(positions: &Positions) -> Vec<(Slot, [u64; 3])> {
    positions
        .iter()
        .map(|(slot, p)| (*slot, p.map(f64::to_bits)))
        .collect()
}

fn open_twice(name: &str) -> (LegacySurvey, LegacySurvey) {
    let path = fixture(name);
    let before = std::fs::read(&path).unwrap();
    let a = open_path(&path, &OpenOptions::default()).unwrap();
    let b = open_path(&path, &OpenOptions::default()).unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), before, "{name} untouched");

    assert_eq!(bits(&a.raw_positions), bits(&b.raw_positions), "{name}");
    assert_eq!(
        bits(a.adjusted_positions.as_ref().unwrap()),
        bits(b.adjusted_positions.as_ref().unwrap()),
        "{name}"
    );
    assert_eq!(a.component_anchors, b.component_anchors, "{name}");
    assert_eq!(
        a.load.derived_shot_uuids, b.load.derived_shot_uuids,
        "{name}"
    );
    assert_eq!(
        serde_json::to_string(&a.project).unwrap(),
        serde_json::to_string(&b.project).unwrap(),
        "{name}"
    );

    // One position per station slot, raw and adjusted, all finite.
    let slots: usize = a.project.lines.iter().map(|l| l.shots.len() + 1).sum();
    assert!(slots > 0, "{name}");
    assert_eq!(a.raw_positions.len(), slots, "{name}");
    assert_eq!(
        a.adjusted_positions.as_ref().unwrap().len(),
        slots,
        "{name}"
    );
    assert!(a
        .raw_positions
        .values()
        .chain(a.adjusted_positions.as_ref().unwrap().values())
        .all(|p| p.iter().all(|v| v.is_finite())));
    assert!(!a.component_anchors.is_empty(), "{name}");
    (a, b)
}

#[test]
fn the_schema_4_fixture_opens_deterministically() {
    let (a, _) = open_twice("fractal_plant_iter3.stalx");
    assert_eq!(a.schema_version, 4);
    // Pre-v6: no shot uuids on disk, so every one is derived.
    let shots: usize = a.project.lines.iter().map(|l| l.shots.len()).sum();
    assert_eq!(a.load.derived_shot_uuids.len(), shots);
}

#[test]
fn the_schema_14_fixture_opens_deterministically() {
    let (a, _) = open_twice("naharon.stalx");
    assert_eq!(a.schema_version, 14);
}
