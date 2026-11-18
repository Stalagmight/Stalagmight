#![expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
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
    let mut big = before.clone();
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
    assert!(encoded_len(&undo) < shot_bytes + 200, "{}", encoded_len(&undo));
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
