//! Property-based, seeded fuzz test for cross-reference anchor stability.
//!
//! Station offsets, station comments, zero datums, loop closures and station
//! merges all anchor to a *station*, identified by the UUID of the shot that
//! creates it ([`StationKey::AfterShot`]). This pins what that identity buys:
//! a random interleaving of inserts and deletes never strands an anchor, and a
//! merge that survives the edits stays colocated.
//!
//! It used to live in the collaboration reconcile fuzz harness, beside the diff
//! engine that harness tested. It is not about collaboration at all, so it
//! belongs here, next to the `Project` geometry it constrains.
//!
//! There is no randomized REVERSE property here because the same invariant is
//! pinned targeted-style, one anchor kind at a time. The tests that carry it -
//! and that must not be deleted as redundant, because nothing else covers what
//! they cover:
//!
//! - ZERO DATUM anchor, in `core/state/reverse_tests.rs`:
//!   `reverse_works_on_a_datum_anchored_section_and_keeps_the_datum_in_place`.
//!   Reverses through the full `Command::ReverseSection` and asserts every
//!   station kept its absolute position and that
//!   `resolve_key(zero_datums[0].station_ref)` follows the physical station to
//!   its mirrored index rather than staying on index 0.
//! - STATION MERGE anchor, same file:
//!   `reversing_a_merge_anchored_section_moves_nothing_through_the_full_execute`.
//!   Also through the full `execute`, the only level at which the reconcile
//!   epilogue's `realign_merges` can run. Asserts the merge followed the
//!   physical station, that the untouched side was not dragged, and that
//!   `check_merge_alignment()` is empty - the same colocation this file's
//!   interleaved-edit property demands.
//! - LOOP CLOSURE anchor with no datum, in `reverse.rs` itself:
//!   `reverse_preserves_adjusted_coordinates_of_a_datum_less_component`, checked
//!   on the CLOSURE-ADJUSTED coordinates that Plan/Export mode force. It pinned
//!   a real bug: the gauge anchor was the lex-min `(section_id, station_index)`,
//!   which a reverse moves to the other physical end, sliding the whole adjusted
//!   component.
//!
//! `reverse.rs`'s own `reverse_refreshes_the_sections_shot_uuid_index` and
//! `reverse_keeps_external_refs_on_their_physical_station` are about the
//! `shot_uuid_index` cache and `station_comments` respectively - neither builds
//! a datum, merge or closure anchor, and neither calls
//! `check_structural_integrity` - so they do NOT stand in for the three above.
//!
//! Dependency-free: a SplitMix64 PRNG plus one generated topology. A failing
//! case prints its seed so it can be pinned as a fixed-seed regression test.
//! Iteration count is scaled by `ANCHOR_FUZZ_ITERS` (default 64).

#![expect(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::{Project, SectionData, ShotData, StationMerge, StationRef};

/// SplitMix64 - deterministic and reproducible from a seed.
struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Rng(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `[0, n)`. `n` must be > 0.
    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }

    /// Uniform in `[lo, hi]` (inclusive).
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + self.below(hi - lo + 1)
    }
}

fn iters() -> u64 {
    std::env::var("ANCHOR_FUZZ_ITERS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(64)
}

/// Depth-continuous shot run starting at `depth`, with within-section distinct
/// azimuths. Returns the shots and the final station depth.
fn gen_run(rng: &mut Rng, n: usize, start_depth: f64) -> (Vec<ShotData>, f64) {
    let mut shots = Vec::with_capacity(n);
    let mut depth = start_depth;
    for i in 0..n {
        let azimuth = 5.0 + (i as f64) * 7.0 + (rng.below(3) as f64); // distinct per i
        let length = 3.0 + (i as f64) * 0.37 + (rng.range(0, 4) as f64) * 0.05;
        let dz = 0.3 + (rng.range(0, 5) as f64) * 0.2;
        let depth_in = depth;
        let depth_out = depth + dz;
        depth = depth_out;
        shots.push(ShotData {
            azimuth_in: azimuth,
            azimuth_out: azimuth,
            length,
            depth_in,
            depth_out,
            ..Default::default()
        });
    }
    (shots, depth)
}

fn section(name: &str, shots: Vec<ShotData>) -> SectionData {
    SectionData {
        name: Some(name.to_string()),
        surveyor: None,
        date: None,
        color: "#3366cc".to_string(),
        direction: None,
        quality: 1.0,
        shots,
    }
}

/// Root section plus a child merged into its station 1 - the shape the property
/// needs, since it puts an external ref on the section being edited.
fn gen_merged_project(rng: &mut Rng) -> Project {
    let mut p = Project::new("Fuzz".into(), None);
    let root_n = rng.range(1, 5);
    let (root_shots, _) = gen_run(rng, root_n, 0.0);
    let root_station1_depth = root_shots[0].depth_out;
    p.add_section(section("Main", root_shots));

    // The child's merge station has to sit at the parent depth (continuity), so
    // flatten its shot 0 and re-base the rest from there.
    let child_n = rng.range(2, 4);
    let mut child_shots = gen_run(rng, child_n, root_station1_depth).0;
    child_shots[0].depth_in = root_station1_depth;
    child_shots[0].depth_out = root_station1_depth;
    let mut d = root_station1_depth;
    for s in child_shots.iter_mut().skip(1) {
        let dz = s.depth_out - s.depth_in;
        s.depth_in = d;
        s.depth_out = d + dz;
        d += dz;
    }
    p.add_section(section("Branch", child_shots));

    let root_id = p.sections[0].id;
    let child_id = p.sections[1].id;
    p.station_merges.push(StationMerge {
        station_a: StationRef::new(root_id, 1).into(),
        station_b: StationRef::new(child_id, 1).into(),
        merged_at: chrono::Utc::now(),
    });
    p
}

/// True if any stored cross-reference dangles (its anchoring shot is gone).
/// With stable identity this is the failure an interleaved insert/delete must
/// never produce - a missed positional shift used to leave one silently.
fn has_dangling_ref(p: &Project) -> bool {
    p.check_structural_integrity()
        .iter()
        .any(|v| v.category.starts_with("dangling_"))
}

/// Property: a random interleaving of shot inserts and deletes on a merged
/// project never strands a cross-reference (no dangling anchor after ANY edit)
/// and the surviving merge stays colocated.
fn prop_anchors_survive_interleaved_edits(seed: u64) {
    let mut rng = Rng::new(seed);
    let mut p = gen_merged_project(&mut rng);
    // Canonicalize anchors, exactly as a load does, then confirm it starts sound.
    p.redrive_offsets_after_load();
    assert!(
        !has_dangling_ref(&p),
        "seed={seed}: project starts with a dangling ref"
    );

    let root_id = p.sections[0].id;
    let ops = rng.range(3, 12);
    for _ in 0..ops {
        let Some(shots_len) = p.get_section(root_id).map(|s| s.shots.len()) else {
            break;
        };
        if shots_len == 0 {
            break;
        }
        let pick = rng.below(shots_len);
        let shot_id = p.get_section(root_id).unwrap().shots[pick].id;
        if shots_len > 1 && rng.below(2) == 0 {
            // Delete the picked shot (rewires/removes any ref on its station).
            let _ = p.remove_shot_with_rewire(root_id, shot_id);
        } else {
            // Insert a fresh shot after the picked one.
            let data = gen_run(&mut rng, 1, 0.0).0.into_iter().next().unwrap();
            p.insert_shot_after(root_id, shot_id, data, None, None);
        }
        assert!(
            !has_dangling_ref(&p),
            "seed={seed}: an interleaved edit stranded a cross-reference:\n{:#?}",
            p.check_structural_integrity()
        );
    }
    // Any merge that survived the edits must still be colocated (the offset
    // cascade kept its endpoints coincident - no silent drift).
    assert!(
        p.check_merge_alignment().is_empty(),
        "seed={seed}: a merge drifted after interleaved edits:\n{p:#?}"
    );
}

/// Where the run starts (`ANCHOR_FUZZ_FIRST`, default 0), so a long run can go
/// on from the last one instead of repeating its cases.
fn first() -> u64 {
    std::env::var("ANCHOR_FUZZ_FIRST")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

#[test]
fn fuzz_anchors_survive_interleaved_edits() {
    let first = first();
    for i in first..first.saturating_add(iters()) {
        prop_anchors_survive_interleaved_edits(i ^ 0x0A11_0001);
    }
}
