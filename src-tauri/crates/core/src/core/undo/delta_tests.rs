#![expect(
    clippy::unwrap_used,
    reason = "test module: a failed lookup is a failed test"
)]

use serde_json::{json, Value};

use super::{same, ProjectDelta};

/// A tiny deterministic generator (xorshift), so a failing seed replays.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            0
        } else {
            (self.next() % n as u64) as usize
        }
    }
    fn float(&mut self) -> f64 {
        // Mix in values whose bits matter: signed zero and ULP neighbours.
        match self.below(8) {
            0 => 0.0,
            1 => -0.0,
            2 => 0.1 + 0.2,
            _ => (self.next() % 100_000) as f64 / 7.0,
        }
    }
}

const STRIDE: u64 = crate::models::FEATURE_ID_STRIDE as u64;

fn shot(rng: &mut Rng, section: u64, seq: u64, uuid: u64) -> Value {
    json!({
        "id": section * STRIDE + seq,
        "uuid": format!("u-{uuid}"),
        "azimuth_in": rng.float(),
        "azimuth_out": rng.float(),
        "length": rng.float(),
        "depth_in": rng.float(),
        "depth_out": rng.float(),
        "pitch_in": null,
        "comment": if rng.below(4) == 0 { Value::from("note") } else { Value::Null },
        "kind": "Normal",
    })
}

struct World {
    rng: Rng,
    next_uuid: u64,
    next_section: u64,
}

impl World {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            next_uuid: 1,
            next_section: 1,
        }
    }

    fn section(&mut self, shots: usize) -> Value {
        let id = self.next_section;
        self.next_section += 1;
        let mut list = Vec::new();
        for seq in 0..shots as u64 {
            let uuid = self.next_uuid;
            self.next_uuid += 1;
            list.push(shot(&mut self.rng, id, seq, uuid));
        }
        json!({
            "id": id,
            "name": format!("S{id}"),
            "shots": list,
            "next_shot_id": id * STRIDE + shots as u64,
            "reverse_parity": false,
            "gauge_shot_uuid": null,
            "gauge_neighbour_uuid": null,
        })
    }

    fn project(&mut self, sections: usize) -> Value {
        let list: Vec<Value> = (0..sections)
            .map(|_| {
                let n = 1 + self.rng.below(12);
                self.section(n)
            })
            .collect();
        let merges: Vec<Value> = (0..80).map(|i| json!({"a": i, "b": i + 1})).collect();
        let offsets: Vec<Value> = (0..70)
            .map(|i| json!([{"section_id": i, "anchor": "SectionStart"}, [i, 0.5, -0.0]]))
            .collect();
        json!({
            "name": "p",
            "sections": list,
            "station_merges": merges,
            "station_offsets": offsets,
            "section_offsets": {"1": [0.0, 1.0], "2": [3.0, -0.0]},
            "zero_datums": [],
            "next_section_id": self.next_section,
            "counter_parity_shots": (0..90).map(|i| format!("c{i}")).collect::<Vec<_>>(),
        })
    }

    fn sections(p: &mut Value) -> &mut Vec<Value> {
        p["sections"].as_array_mut().unwrap()
    }

    /// One random edit of the kind a real command makes.
    fn mutate(&mut self, p: &mut Value) {
        let n_sections = Self::sections(p).len();
        match self.rng.below(12) {
            // Edit one shot field (sometimes only the sign of a zero).
            0 | 1 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                let shots = Self::sections(p)[s]["shots"].as_array_mut().unwrap();
                if !shots.is_empty() {
                    let i = self.rng.below(shots.len());
                    let v = self.rng.float();
                    shots[i]["length"] = json!(v);
                    if self.rng.below(3) == 0 {
                        shots[i]["depth_in"] = json!(-0.0);
                    }
                }
            }
            // Delete a run of shots.
            2 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                let shots = Self::sections(p)[s]["shots"].as_array_mut().unwrap();
                if !shots.is_empty() {
                    let i = self.rng.below(shots.len());
                    let k = 1 + self.rng.below(shots.len() - i);
                    shots.drain(i..i + k);
                }
            }
            // Insert a new shot.
            3 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                let sec = &mut Self::sections(p)[s];
                let id = sec["id"].as_u64().unwrap();
                let next = sec["next_shot_id"].as_u64().unwrap();
                let uuid = self.next_uuid;
                self.next_uuid += 1;
                let new = shot(&mut self.rng, id, next - id * STRIDE, uuid);
                sec["next_shot_id"] = json!(next + 1);
                let shots = sec["shots"].as_array_mut().unwrap();
                let at = self.rng.below(shots.len() + 1);
                shots.insert(at, new);
            }
            // Split a section: move its tail into a new section.
            4 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                let tail: Vec<Value> = {
                    let shots = Self::sections(p)[s]["shots"].as_array_mut().unwrap();
                    if shots.len() < 2 {
                        return;
                    }
                    let at = 1 + self.rng.below(shots.len() - 1);
                    shots.drain(at..).collect()
                };
                let mut fresh = self.section(0);
                fresh["shots"] = Value::Array(tail);
                Self::sections(p).push(fresh);
                p["next_section_id"] = json!(self.next_section);
            }
            // Combine: append one section's shots to another, delete it.
            5 if n_sections > 1 => {
                let a = self.rng.below(n_sections);
                let mut b = self.rng.below(n_sections);
                if a == b {
                    b = (b + 1) % n_sections;
                }
                let moved = Self::sections(p)[b]["shots"].as_array().unwrap().clone();
                Self::sections(p)[a]["shots"]
                    .as_array_mut()
                    .unwrap()
                    .extend(moved);
                Self::sections(p).remove(b);
            }
            // Reverse a section (order and a field per shot).
            6 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                let sec = &mut Self::sections(p)[s];
                let shots = sec["shots"].as_array_mut().unwrap();
                shots.reverse();
                for shot in shots.iter_mut() {
                    let az = shot["azimuth_in"].as_f64().unwrap();
                    shot["azimuth_in"] = json!((az + 180.0) % 360.0);
                }
                let parity = sec["reverse_parity"].as_bool().unwrap();
                sec["reverse_parity"] = json!(!parity);
            }
            // Delete a section.
            7 if n_sections > 0 => {
                let s = self.rng.below(n_sections);
                Self::sections(p).remove(s);
            }
            // Add a section.
            8 => {
                let n = 1 + self.rng.below(5);
                let fresh = self.section(n);
                Self::sections(p).push(fresh);
                p["next_section_id"] = json!(self.next_section);
            }
            // Edit the ordered and the unordered lists.
            9 => {
                let merges = p["station_merges"].as_array_mut().unwrap();
                if !merges.is_empty() {
                    let i = self.rng.below(merges.len());
                    merges.remove(i);
                }
                merges.push(json!({"a": 1000 + self.rng.below(100), "b": 0}));
            }
            10 => {
                let offsets = p["station_offsets"].as_array_mut().unwrap();
                if !offsets.is_empty() {
                    let i = self.rng.below(offsets.len());
                    offsets[i][1] = json!([self.rng.float(), 0.0, 0.0]);
                }
                let parity = p["counter_parity_shots"].as_array_mut().unwrap();
                parity.retain(|_| self.rng.below(10) != 0);
            }
            // A gauge pair and an offset key.
            _ => {
                if n_sections > 0 {
                    let s = self.rng.below(n_sections);
                    let sec = &mut Self::sections(p)[s];
                    if let Some(first) = sec["shots"].get(0).and_then(|s| s.get("uuid")).cloned() {
                        sec["gauge_shot_uuid"] = first;
                    }
                }
                let key = format!("{}", self.rng.below(5));
                if self.rng.below(2) == 0 {
                    p["section_offsets"][key] = json!([self.rng.float(), 1.0]);
                } else if let Some(map) = p["section_offsets"].as_object_mut() {
                    map.remove(&key);
                }
            }
        }
    }
}

fn round_trip(source: &Value, target: &Value) -> ProjectDelta {
    let delta = ProjectDelta::between(source, target).unwrap();
    let rebuilt = delta.apply(source).unwrap();
    assert!(
        same(&rebuilt, target),
        "delta did not rebuild the target\nsource: {source}\ntarget: {target}\nrebuilt: {rebuilt}\ndelta: {}",
        serde_json::to_string(&delta).unwrap()
    );
    // A delta survives its own serialization.
    let reread: ProjectDelta =
        serde_json::from_str(&serde_json::to_string(&delta).unwrap()).unwrap();
    assert_eq!(reread, delta);
    delta
}

#[test]
fn identical_states_give_an_empty_delta() {
    let mut world = World::new(7);
    let p = world.project(5);
    let delta = round_trip(&p, &p);
    assert!(delta.is_empty());
}

#[test]
fn random_edit_chains_rebuild_both_directions_exactly() {
    for seed in 1..=300u64 {
        let mut world = World::new(seed);
        let mut state = world.project(1 + (seed as usize % 7));
        for _ in 0..12 {
            let before = state.clone();
            world.mutate(&mut state);
            round_trip(&state, &before);
            round_trip(&before, &state);
        }
    }
}

#[test]
fn a_signed_zero_flip_is_an_edit() {
    let mut world = World::new(3);
    let before = world.project(2);
    let mut after = before.clone();
    after["sections"][0]["shots"][0]["length"] = json!(-0.0);
    after["sections"][0]["shots"][0]["depth_out"] = json!(0.0);
    let delta = round_trip(&before, &after);
    assert!(!delta.is_empty());
}

fn encoded_len(delta: &ProjectDelta) -> usize {
    serde_json::to_string(delta).unwrap().len()
}

#[test]
fn moving_and_reversing_shots_costs_runs_not_shots() {
    let mut world = World::new(11);
    let before = world.project(3);
    let shot_bytes = serde_json::to_string(&before["sections"][0]["shots"][0])
        .unwrap()
        .len();
    let mut big = before;
    // One section with 400 shots.
    let long = world.section(400);
    World::sections(&mut big).push(long);
    big["next_section_id"] = json!(world.next_section);
    // Split it in the middle.
    let mut split = big.clone();
    let tail: Vec<Value> = World::sections(&mut split)[3]["shots"]
        .as_array_mut()
        .unwrap()
        .drain(200..)
        .collect();
    let mut fresh = world.section(0);
    fresh["shots"] = Value::Array(tail);
    World::sections(&mut split).push(fresh);
    split["next_section_id"] = json!(world.next_section);
    for (from, to) in [(&big, &split), (&split, &big)] {
        let delta = round_trip(from, to);
        assert!(
            encoded_len(&delta) < 4 * shot_bytes,
            "a split should cost runs, not shots: {}",
            serde_json::to_string(&delta).unwrap()
        );
    }
    // Reverse it: order flips, values stay.
    let mut reversed = big.clone();
    World::sections(&mut reversed)[3]["shots"]
        .as_array_mut()
        .unwrap()
        .reverse();
    for (from, to) in [(&big, &reversed), (&reversed, &big)] {
        let delta = round_trip(from, to);
        assert!(
            encoded_len(&delta) < 4 * shot_bytes,
            "a reverse should cost one run: {}",
            serde_json::to_string(&delta).unwrap()
        );
    }
}

#[test]
fn deleting_one_shot_records_that_shot_only() {
    let mut world = World::new(5);
    let mut before = world.project(2);
    let long = world.section(1000);
    World::sections(&mut before).push(long);
    before["next_section_id"] = json!(world.next_section);
    let mut after = before.clone();
    let removed = World::sections(&mut after)[2]["shots"]
        .as_array_mut()
        .unwrap()
        .remove(500);
    let undo = round_trip(&after, &before);
    let redo = round_trip(&before, &after);
    let shot_bytes = serde_json::to_string(&removed).unwrap().len();
    assert!(
        encoded_len(&undo) < shot_bytes + 200,
        "{}",
        encoded_len(&undo)
    );
    assert!(encoded_len(&redo) < 160, "{}", encoded_len(&redo));
}

#[test]
fn an_unordered_list_survives_a_prune_of_another_element() {
    let mut world = World::new(9);
    let before = world.project(1);
    let mut after = before.clone();
    after["counter_parity_shots"]
        .as_array_mut()
        .unwrap()
        .push(json!("added"));
    let delta = ProjectDelta::between(&after, &before).unwrap();
    // A load pruned an unrelated element out of the live list.
    let mut pruned = after.clone();
    pruned["counter_parity_shots"]
        .as_array_mut()
        .unwrap()
        .remove(3);
    let rebuilt = delta.apply(&pruned).unwrap();
    let list = rebuilt["counter_parity_shots"].as_array().unwrap();
    assert!(!list.contains(&json!("added")));
    assert_eq!(list.len(), 89);
}

#[test]
fn a_positional_edit_refuses_a_source_of_another_length() {
    let mut world = World::new(13);
    let before = world.project(1);
    let mut after = before.clone();
    after["station_merges"].as_array_mut().unwrap().remove(10);
    let delta = ProjectDelta::between(&after, &before).unwrap();
    let mut shorter = after.clone();
    shorter["station_merges"].as_array_mut().unwrap().remove(0);
    assert!(delta.apply(&shorter).is_err());
}

#[test]
fn a_literal_shot_whose_id_a_reload_reused_takes_a_fresh_one() {
    let mut world = World::new(17);
    let mut before = world.project(0);
    let sec = world.section(5);
    World::sections(&mut before).push(sec);
    before["next_section_id"] = json!(world.next_section);
    let mut after = before.clone();
    // Delete the middle shot.
    World::sections(&mut after)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    let undo = ProjectDelta::between(&after, &before).unwrap();
    // A reload renumbers the four survivors 0..4 and the allocator to 4.
    let mut reloaded = after.clone();
    let base = World::sections(&mut reloaded)[0]["id"].as_u64().unwrap() * STRIDE;
    for (i, shot) in World::sections(&mut reloaded)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        shot["id"] = json!(base + i as u64);
    }
    World::sections(&mut reloaded)[0]["next_shot_id"] = json!(base + 4);
    let rebuilt = undo.apply(&reloaded).unwrap();
    let shots = rebuilt["sections"][0]["shots"].as_array().unwrap();
    let ids: std::collections::HashSet<u64> =
        shots.iter().map(|s| s["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 5, "ids must stay unique: {shots:?}");
    let next = rebuilt["sections"][0]["next_shot_id"].as_u64().unwrap();
    assert!(ids.iter().all(|id| *id < next));
    assert_eq!(shots[2]["uuid"], before["sections"][0]["shots"][2]["uuid"]);
}

#[test]
fn a_section_counter_rewound_by_a_reload_is_raised_past_a_restored_section() {
    let mut world = World::new(19);
    let before = world.project(3);
    let mut after = before.clone();
    // Delete the highest section; the counter stays.
    World::sections(&mut after).pop();
    let undo = ProjectDelta::between(&after, &before).unwrap();
    // A reload recomputes the counter as max + 1, the deleted section's id.
    let mut reloaded = after.clone();
    let max = World::sections(&mut reloaded)
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .max()
        .unwrap();
    reloaded["next_section_id"] = json!(max + 1);
    let rebuilt = undo.apply(&reloaded).unwrap();
    let restored_max = rebuilt["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .max()
        .unwrap();
    assert!(rebuilt["next_section_id"].as_u64().unwrap() > restored_max);
}

// === Hostile deltas: a delta is read back from the file ===

fn hostile(text: &str) -> ProjectDelta {
    serde_json::from_str(text).unwrap()
}

/// `delta` breaks a rule, and the one `why` names.
fn refused_for(delta: &ProjectDelta, why: &str) {
    match delta.check_shape() {
        Ok(()) => panic!("a delta that breaks a rule passed: {delta:?}"),
        Err(reason) => assert!(reason.contains(why), "refused for another rule: {reason}"),
    }
}

fn section_count(p: &Value) -> usize {
    p["sections"].as_array().unwrap().len()
}

/// A delta must not build more than its source holds plus what it carries,
/// or a small row grows the project it is applied to without bound.
#[test]
fn a_delta_that_keeps_a_section_twice_is_refused() {
    let mut world = World::new(23);
    let p = world.project(2);
    let n = section_count(&p);
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"k":[0,{n}]}},{{"k":[0,{n}]}}]]}}"#
    ));
    refused_for(&delta, "reads a source section twice");
    assert!(delta.apply(&p).is_err(), "applied: the sections doubled");
}

#[test]
fn a_delta_that_copies_a_shot_twice_is_refused() {
    let mut world = World::new(29);
    let p = world.project(2);
    let n = section_count(&p);
    let id = p["sections"][0]["id"].as_u64().unwrap();
    let copy = r#"{"c":{"sec":0,"at":0,"n":1}}"#;
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"e":{{"at":0,"id":{id},"s":[{copy},{copy}]}}}},{{"k":[1,{}]}}]]}}"#,
        n - 1
    ));
    refused_for(&delta, "copies a source shot twice");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn a_delta_that_copies_out_of_a_kept_section_is_refused() {
    let mut world = World::new(31);
    let p = world.project(2);
    let n = section_count(&p);
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"k":[0,{n}]}},{{"n":{{"f":{{"id":900}},"s":[{{"c":{{"sec":0,"at":0,"n":1}}}}]}}}}]]}}"#
    ));
    refused_for(&delta, "whose shots stay where they are");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn a_delta_that_keeps_an_array_element_twice_is_refused() {
    let mut world = World::new(37);
    let p = world.project(1);
    let len = p["station_merges"].as_array().unwrap().len();
    let delta = hostile(&format!(
        r#"{{"f":{{"station_merges":{{"a":[{len},[{{"k":[0,{len}]}},{{"k":[0,{len}]}}]]}}}}}}"#
    ));
    refused_for(&delta, "reads an element twice");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn a_new_section_with_an_id_past_the_bound_is_refused_not_overflowed() {
    let mut world = World::new(41);
    let p = world.project(1);
    let n = section_count(&p);
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"k":[0,{n}]}},{{"n":{{"f":{{"id":18446744073709551615,"next_shot_id":0}},"s":[]}}}}]]}}"#
    ));
    refused_for(&delta, "past the bound");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn the_holder_walk_refuses_a_delta_that_breaks_the_rules() {
    use std::rc::Rc;
    let view: Vec<super::history::HolderSection> = vec![(1, Rc::new(vec![Some(Rc::from("u-1"))]))];
    let doubled = hostile(r#"{"s":[1,[{"k":[0,1]},{"k":[0,1]}]]}"#);
    assert!(doubled.step_holders(&view).is_none());
    let kept = hostile(r#"{"s":[1,[{"k":[0,1]}]]}"#);
    assert_eq!(kept.step_holders(&view).map(|v| v.len()), Some(1));
}

/// Two shots that share a uuid (a legacy file can hold such a pair): a split
/// that sends one to each side copies each source shot once and writes the
/// second match literally, so the delta keeps the rules and rebuilds exactly.
#[test]
fn a_repeated_uuid_is_copied_once_and_written_literally_after() {
    let mut world = World::new(43);
    let mut before = world.project(0);
    let mut section = world.section(4);
    let twin = section["shots"][0]["uuid"].clone();
    section["shots"][2]["uuid"] = twin;
    World::sections(&mut before).push(section);
    before["next_section_id"] = json!(world.next_section);
    let mut after = before.clone();
    let mut tail = World::sections(&mut after)[0].clone();
    let shots = World::sections(&mut after)[0]["shots"]
        .as_array_mut()
        .unwrap();
    let moved: Vec<Value> = shots.drain(2..).collect();
    tail["id"] = json!(world.next_section);
    tail["shots"] = json!(moved);
    World::sections(&mut after).push(tail);
    after["next_section_id"] = json!(world.next_section + 1);
    for (source, target) in [(&before, &after), (&after, &before)] {
        let delta = round_trip(source, target);
        delta.check_shape().unwrap();
    }
}

/// A copy carries no uuid of its own, so a rename the load's heal makes in
/// the section a shot is copied into is pinned onto the copy as an edit.
#[test]
fn a_renamed_id_is_pinned_onto_the_shot_a_delta_copies() {
    use std::rc::Rc;
    let mut world = World::new(47);
    let mut source = world.project(0);
    let first = world.section(3);
    let second = world.section(1);
    World::sections(&mut source).push(first);
    World::sections(&mut source).push(second);
    source["next_section_id"] = json!(world.next_section + 1);
    // Move the first section's last shot into a new section.
    let mut target = source.clone();
    let moved = World::sections(&mut target)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    let uuid = moved["uuid"].as_str().unwrap().to_string();
    let new_section = world.next_section;
    let mut fresh = World::sections(&mut target)[1].clone();
    fresh["id"] = json!(new_section);
    fresh["shots"] = json!([moved]);
    World::sections(&mut target).push(fresh);
    let mut delta = ProjectDelta::between(&source, &target).unwrap();

    let renamed = format!("{uuid}-renamed");
    let view: Vec<super::history::HolderSection> = source["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| {
            let id = u32::try_from(s["id"].as_u64().unwrap()).unwrap();
            let shots = s["shots"]
                .as_array()
                .unwrap()
                .iter()
                .map(|shot| shot["uuid"].as_str().map(Rc::from))
                .collect();
            (id, Rc::new(shots))
        })
        .collect();
    let target_section = u32::try_from(new_section).unwrap();
    let rename = |section: u32, old: &str| {
        (section == target_section && old == uuid).then(|| renamed.clone())
    };
    assert!(
        delta.rename_copies(&view, &rename),
        "the moved shot is a copy, so the rename has to be pinned onto it"
    );
    let rebuilt = delta.apply(&source).unwrap();
    let mut expected = target.clone();
    World::sections(&mut expected)[2]["shots"][0]["uuid"] = json!(renamed);
    assert!(same(&rebuilt, &expected), "rebuilt: {rebuilt}");
}

/// A section whose text differs but whose value is the same stays where it
/// is, so its shots are not copied anywhere else: a new section holding a
/// shot under one of its uuids carries that shot literally.
#[test]
fn a_section_kept_by_value_lends_no_shot_to_another() {
    let source = r#"{"name":"p","next_section_id":3,"sections":[{"id":1,"name":"A","next_shot_id":100002,"shots":[{"id":100000,"uuid":"u-1","length":2.0},{"id":100001,"uuid":"u-2","length":3.0}]}]}"#;
    // Section 1 again, with its keys in another order; and a new section 2
    // whose shot shares a uuid with one of section 1's.
    let target = r#"{"name":"p","next_section_id":3,"sections":[{"name":"A","id":1,"next_shot_id":100002,"shots":[{"uuid":"u-1","id":100000,"length":2.0},{"id":100001,"uuid":"u-2","length":3.0}]},{"id":2,"name":"B","next_shot_id":200001,"shots":[{"id":200000,"uuid":"u-2","length":3.0}]}]}"#;
    let delta = ProjectDelta::between_texts(source, target).unwrap();
    delta.check_shape().unwrap();
    let rebuilt = delta.apply(&serde_json::from_str(source).unwrap()).unwrap();
    assert!(
        same(&rebuilt, &serde_json::from_str(target).unwrap()),
        "rebuilt: {rebuilt}"
    );
}

/// Two target sections under one id (no live project holds such a pair, but
/// a delta must still encode it): the second is written as a new section, so
/// no source section is read twice.
#[test]
fn a_repeated_section_id_reads_its_source_once() {
    let mut world = World::new(53);
    let source = world.project(2);
    let mut target = source.clone();
    let mut twin = World::sections(&mut target)[0].clone();
    twin["name"] = json!("twin");
    World::sections(&mut target)[0]["name"] = json!("renamed");
    World::sections(&mut target).push(twin);
    let delta = ProjectDelta::between(&source, &target).unwrap();
    delta.check_shape().unwrap();
    // The twin's shots repeat the first section's ids, so the apply settles
    // them to fresh ones; the sections themselves are all there, in order.
    let rebuilt = delta.apply(&source).unwrap();
    let names = |p: &Value| -> Vec<String> {
        p["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&rebuilt), names(&target));
}
