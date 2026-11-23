//! The indexes RS-FX9 added for passes over many slots give the per-slot
//! answers (RS-FX10 item 3): [`RecordIndex`] the record lookups,
//! [`SlotTexts`] the plain texts, and the wire's `Station::shown` (built
//! from one `ShownOf` per list) the slot's owner. Fully synthetic data.

use crate::{ShownSlot, StationRef};

use super::ops_test_helpers::{assert_valid, branch, free_line, ok, run};
use super::test_helpers::{at, project};
use super::{ClosureStrategy, LineId, Project, RecordIndex, SlotTexts, StationId};

const BATCH: ClosureStrategy = ClosureStrategy::Batch;

/// Name every station of `line` `{prefix}{index}`.
fn name_line(p: &mut Project, line: LineId, prefix: &str) {
    let chain = p.line_stations(line).unwrap().to_vec();
    for (i, s) in chain.into_iter().enumerate() {
        p.station_mut(s).unwrap().name = format!("{prefix}{i}");
    }
}

/// Every slot of `p` with its node, line by line in chain order.
fn slots(p: &Project) -> Vec<(StationRef, StationId)> {
    p.lines()
        .iter()
        .flat_map(|line| {
            (0..)
                .zip(line.stations())
                .map(|(i, s)| (StationRef::new(line.id, i), s))
        })
        .collect()
}

/// A survey with every shape the indexes resolve:
/// - plain junctions: Side branches from Main 2, Tail from Main 3;
/// - nested records: Twig 0 merged into Spur 1 keeping Spur 1, then Spur 1
///   into Main 4 keeping Main 4 (Main 4 holds Spur 1's record naming Spur
///   and Twig 0's under it naming Twig: foreign slots on both lines);
/// - loop ends on other lines: Side 2 archived behind Main 6, Twig 2
///   behind Main 5, and Spur 3 behind Main 4, whose own record names Spur
///   (foreign by the kept end's records);
/// - a loop end at a junction: Main 3 (Main and Tail) behind Side 1;
/// - a same-line closure: Ring 4 archived behind Ring 0 (not foreign);
/// - a closure keeping both names (Tail 2 to Main 1).
fn mixed() -> (Project, [LineId; 6]) {
    let mut p = project();
    let main = free_line(&mut p, [0.0, 0.0], run(90.0, 6, 10.0));
    let main2 = at(&p, main, 2);
    let side = branch(&mut p, main2, run(0.0, 2, 10.0));
    let spur = free_line(&mut p, [40.0, -30.0], run(0.0, 3, 10.0));
    let twig = free_line(&mut p, [80.0, -30.0], run(270.0, 2, 10.0));
    let ring = free_line(&mut p, [0.0, 200.0], run(90.0, 4, 10.0));
    let main3 = at(&p, main, 3);
    let tail = branch(&mut p, main3, run(180.0, 2, 10.0));
    for (line, prefix) in [(main, "M"), (spur, "P"), (twig, "T"), (ring, "R")] {
        name_line(&mut p, line, prefix);
    }
    // The branches' own stations, past the junctions they share with Main.
    for (line, prefix) in [(side, "S"), (tail, "L")] {
        for i in 1..=2 {
            p.station_mut(at(&p, line, i)).unwrap().name = format!("{prefix}{i}");
        }
    }
    assert_valid(&p);

    let (spur1, main4) = (at(&p, spur, 1), at(&p, main, 4));
    ok(p.merge_stations(at(&p, twig, 0), spur1, spur1, &[]));
    ok(p.merge_stations(spur1, main4, main4, &[]));
    let (main1, main5, main6) = (at(&p, main, 1), at(&p, main, 5), at(&p, main, 6));
    let (side1, side2, twig2, spur3) = (
        at(&p, side, 1),
        at(&p, side, 2),
        at(&p, twig, 2),
        at(&p, spur, 3),
    );
    let (ring0, ring4, tail2) = (at(&p, ring, 0), at(&p, ring, 4), at(&p, tail, 2));
    ok(p.create_closure(
        side2,
        Some(side),
        main6,
        Some(main),
        BATCH,
        Some(main6),
        &[],
    ));
    ok(p.create_closure(
        twig2,
        Some(twig),
        main5,
        Some(main),
        BATCH,
        Some(main5),
        &[],
    ));
    ok(p.create_closure(
        spur3,
        Some(spur),
        main4,
        Some(main),
        BATCH,
        Some(main4),
        &[],
    ));
    ok(p.create_closure(
        main3,
        Some(main),
        side1,
        Some(side),
        BATCH,
        Some(side1),
        &[],
    ));
    ok(p.create_closure(
        ring4,
        Some(ring),
        ring0,
        Some(ring),
        BATCH,
        Some(ring0),
        &[],
    ));
    ok(p.create_closure(tail2, Some(tail), main1, Some(main), BATCH, None, &[]));
    assert_valid(&p);
    (p, [main, side, spur, twig, ring, tail])
}

/// The wire's `shown` of a slot, worked out from the slot's owner alone
/// ([`Project::slot_owner`], every lookup per slot).
fn shown_per_slot(p: &Project, slot: StationRef) -> Option<ShownSlot> {
    let owner = p.slot_owner(slot)?;
    let loop_end = owner.loop_closure.is_some();
    (owner.foreign || loop_end).then_some(ShownSlot {
        id: owner.shown,
        home_line: owner.home_line,
        foreign: owner.foreign,
        loop_end,
        loop_closure: owner.loop_closure,
    })
}

/// The fixture has what the indexes are for: nested records, foreign slots
/// that are not loop ends, foreign loop ends, and a loop end on its kept
/// end's own line.
#[test]
fn the_mixed_fixture_has_every_shape() {
    let (p, [main, _, spur, twig, ring, _]) = mixed();
    let main4 = at(&p, main, 4);
    let records = &p.stations[&main4].archived;
    assert_eq!(records.len(), 2, "{records:?}");
    assert!(records.iter().any(|r| r.into.is_some()), "nested");
    assert!(p.record_naming(main4, spur).is_some());
    assert!(p.record_naming(main4, twig).is_some());
    let owners: Vec<_> = slots(&p)
        .into_iter()
        .map(|(slot, _)| (slot, p.slot_owner(slot).unwrap()))
        .collect();
    assert!(owners
        .iter()
        .any(|(_, o)| o.foreign && o.loop_closure.is_none()));
    assert!(owners
        .iter()
        .any(|(_, o)| o.foreign && o.loop_closure.is_some()));
    assert!(owners
        .iter()
        .any(|(slot, o)| slot.line_id == ring && !o.foreign && o.loop_closure.is_some()));
    // Spur 3 shows Main 4 on Spur, a line Main 4's node is on but its record
    // names; Main 3 is a loop end on both lines through it.
    let spur3 = p.slot_owner(StationRef::new(spur, 3)).unwrap();
    assert_eq!((spur3.shown, spur3.foreign), (main4, true));
    assert!(p.topology().slot_on(main4, spur).is_some());
    let main3 = at(&p, main, 3);
    assert!(p.is_closure_archived(main3));
    assert_eq!(p.topology().lines_of(main3).len(), 2);
    assert_eq!(p.closures.len(), 6);
}

/// Every lookup [`RecordIndex`] answers equals the per-node one, for every
/// node and every line, records' Station IDs and a missing one included.
#[test]
fn the_record_index_answers_as_the_records_do() {
    let (p, _) = mixed();
    let index = RecordIndex::of(&p);
    let mut ids: Vec<StationId> = p.stations.keys().copied().collect();
    ids.extend(
        p.stations
            .values()
            .flat_map(|s| s.archived.iter().map(|r| r.id)),
    );
    ids.push(p.counters.next_station_id + 7);
    for &id in &ids {
        assert_eq!(index.native_lines(&p, id), p.native_lines(id), "{id}");
        for line in p.lines().iter().map(|l| l.id).chain([9_999]) {
            let per_node = p.record_naming(id, line);
            assert_eq!(index.record(&p, id, line), per_node, "{id} on {line}");
            assert_eq!(index.names(id, line), per_node.is_some(), "{id} on {line}");
        }
    }
}

/// [`SlotTexts`] words every slot, shot and station as the per-slot forms
/// do: [`Project::slot_text`], [`Project::slot_name_text`],
/// [`Project::shot_text`] and [`Project::station_text`] on every line and
/// none, slots and shots past a line's end and unknown stations included.
#[test]
fn slot_texts_word_every_slot_as_the_per_slot_forms() {
    let (p, lines) = mixed();
    let mut texts = SlotTexts::new(&p);
    for line in p.lines() {
        let count = line.stations().count();
        for i in 0..=count {
            let slot = StationRef::new(line.id, u32::try_from(i).unwrap());
            assert_eq!(texts.slot_text(slot), p.slot_text(slot), "{slot:?}");
            assert_eq!(
                texts.slot_name_text(slot),
                p.slot_name_text(slot),
                "{slot:?}"
            );
            assert_eq!(
                texts.shot_text(line.id, i),
                p.shot_text(line.id, i),
                "{slot:?}"
            );
        }
    }
    let mut ids: Vec<StationId> = p.stations.keys().copied().collect();
    ids.extend(
        p.stations
            .values()
            .flat_map(|s| s.archived.iter().map(|r| r.id)),
    );
    ids.push(p.counters.next_station_id + 7);
    for &id in &ids {
        for line in lines.iter().copied().map(Some).chain([None, Some(9_999)]) {
            assert_eq!(
                texts.station_text(id, line),
                p.station_text(id, line),
                "{id} on {line:?}"
            );
        }
    }
    // A fresh index words them the same (the home-line cache warms nothing
    // that changes an answer).
    let fresh = SlotTexts::new(&p).slot_text(StationRef::new(lines[3], 0));
    assert_eq!(fresh, p.slot_text(StationRef::new(lines[3], 0)));
}

/// Every wire station's `shown` (one `ShownOf` for the list) is the slot's
/// owner as [`Project::slot_owner`] works it out per slot, in the raw and
/// the computed lists, and absent exactly on a native slot showing its own
/// node.
#[test]
fn the_wire_shows_every_slot_as_its_owner_says() {
    let (p, _) = mixed();
    let expected: Vec<_> = slots(&p)
        .into_iter()
        .map(|(slot, node)| (slot, node, shown_per_slot(&p, slot)))
        .collect();
    assert!(expected.iter().any(|(_, _, s)| s.is_none()));
    let raw = p.stations_wire(&p.raw_positions());
    let got: Vec<_> = raw
        .iter()
        .map(|s| (s.station_ref, s.id, s.shown.clone()))
        .collect();
    assert_eq!(got, expected);
    let adjusted = p.adjusted_positions(&crate::config::ClosureAdjustmentConfig::default());
    let computed: Vec<_> = p
        .adjusted_stations_wire(&adjusted)
        .into_iter()
        .map(|s| (s.station_ref, s.id, s.shown))
        .collect();
    assert_eq!(computed, expected);
}
