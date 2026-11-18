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

fn shot(rng: &mut Rng, line: u64, seq: u64, uuid: u64) -> Value {
    json!({
        "id": line * STRIDE + seq,
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
    next_line: u64,
}

impl World {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1),
            next_uuid: 1,
            next_line: 1,
        }
    }

    fn line(&mut self, shots: usize) -> Value {
        let id = self.next_line;
        self.next_line += 1;
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

    fn project(&mut self, lines: usize) -> Value {
        let list: Vec<Value> = (0..lines)
            .map(|_| {
                let n = 1 + self.rng.below(12);
                self.line(n)
            })
            .collect();
        let merges: Vec<Value> = (0..80).map(|i| json!({"a": i, "b": i + 1})).collect();
        let offsets: Vec<Value> = (0..70)
            .map(|i| json!([{"line_id": i, "anchor": "LineStart"}, [i, 0.5, -0.0]]))
            .collect();
        json!({
            "name": "p",
            "lines": list,
            "station_merges": merges,
            "station_offsets": offsets,
            "line_offsets": {"1": [0.0, 1.0], "2": [3.0, -0.0]},
            "zero_datums": [],
            "next_line_id": self.next_line,
            "counter_parity_shots": (0..90).map(|i| format!("c{i}")).collect::<Vec<_>>(),
        })
    }

    fn lines(p: &mut Value) -> &mut Vec<Value> {
        p["lines"].as_array_mut().unwrap()
    }

    /// One random edit of the kind a real command makes.
    fn mutate(&mut self, p: &mut Value) {
        let n_lines = Self::lines(p).len();
        match self.rng.below(12) {
            // Edit one shot field (sometimes only the sign of a zero).
            0 | 1 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                let shots = Self::lines(p)[s]["shots"].as_array_mut().unwrap();
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
            2 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                let shots = Self::lines(p)[s]["shots"].as_array_mut().unwrap();
                if !shots.is_empty() {
                    let i = self.rng.below(shots.len());
                    let k = 1 + self.rng.below(shots.len() - i);
                    shots.drain(i..i + k);
                }
            }
            // Insert a new shot.
            3 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                let sec = &mut Self::lines(p)[s];
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
            // Split a line: move its tail into a new line.
            4 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                let tail: Vec<Value> = {
                    let shots = Self::lines(p)[s]["shots"].as_array_mut().unwrap();
                    if shots.len() < 2 {
                        return;
                    }
                    let at = 1 + self.rng.below(shots.len() - 1);
                    shots.drain(at..).collect()
                };
                let mut fresh = self.line(0);
                fresh["shots"] = Value::Array(tail);
                Self::lines(p).push(fresh);
                p["next_line_id"] = json!(self.next_line);
            }
            // Combine: append one line's shots to another, delete it.
            5 if n_lines > 1 => {
                let a = self.rng.below(n_lines);
                let mut b = self.rng.below(n_lines);
                if a == b {
                    b = (b + 1) % n_lines;
                }
                let moved = Self::lines(p)[b]["shots"].as_array().unwrap().clone();
                Self::lines(p)[a]["shots"]
                    .as_array_mut()
                    .unwrap()
                    .extend(moved);
                Self::lines(p).remove(b);
            }
            // Reverse a line (order and a field per shot).
            6 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                let sec = &mut Self::lines(p)[s];
                let shots = sec["shots"].as_array_mut().unwrap();
                shots.reverse();
                for shot in shots.iter_mut() {
                    let az = shot["azimuth_in"].as_f64().unwrap();
                    shot["azimuth_in"] = json!((az + 180.0) % 360.0);
                }
                let parity = sec["reverse_parity"].as_bool().unwrap();
                sec["reverse_parity"] = json!(!parity);
            }
            // Delete a line.
            7 if n_lines > 0 => {
                let s = self.rng.below(n_lines);
                Self::lines(p).remove(s);
            }
            // Add a line.
            8 => {
                let n = 1 + self.rng.below(5);
                let fresh = self.line(n);
                Self::lines(p).push(fresh);
                p["next_line_id"] = json!(self.next_line);
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
                if n_lines > 0 {
                    let s = self.rng.below(n_lines);
                    let sec = &mut Self::lines(p)[s];
                    if let Some(first) = sec["shots"].get(0).and_then(|s| s.get("uuid")).cloned() {
                        sec["gauge_shot_uuid"] = first;
                    }
                }
                let key = format!("{}", self.rng.below(5));
                if self.rng.below(2) == 0 {
                    p["line_offsets"][key] = json!([self.rng.float(), 1.0]);
                } else if let Some(map) = p["line_offsets"].as_object_mut() {
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
    after["lines"][0]["shots"][0]["length"] = json!(-0.0);
    after["lines"][0]["shots"][0]["depth_out"] = json!(0.0);
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
    let shot_bytes = serde_json::to_string(&before["lines"][0]["shots"][0])
        .unwrap()
        .len();
    let mut big = before;
    // One line with 400 shots.
    let long = world.line(400);
    World::lines(&mut big).push(long);
    big["next_line_id"] = json!(world.next_line);
    // Split it in the middle.
    let mut split = big.clone();
    let tail: Vec<Value> = World::lines(&mut split)[3]["shots"]
        .as_array_mut()
        .unwrap()
        .drain(200..)
        .collect();
    let mut fresh = world.line(0);
    fresh["shots"] = Value::Array(tail);
    World::lines(&mut split).push(fresh);
    split["next_line_id"] = json!(world.next_line);
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
    World::lines(&mut reversed)[3]["shots"]
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
    let long = world.line(1000);
    World::lines(&mut before).push(long);
    before["next_line_id"] = json!(world.next_line);
    let mut after = before.clone();
    let removed = World::lines(&mut after)[2]["shots"]
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
    let sec = world.line(5);
    World::lines(&mut before).push(sec);
    before["next_line_id"] = json!(world.next_line);
    let mut after = before.clone();
    // Delete the middle shot.
    World::lines(&mut after)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .remove(2);
    let undo = ProjectDelta::between(&after, &before).unwrap();
    // A reload renumbers the four survivors 0..4 and the allocator to 4.
    let mut reloaded = after.clone();
    let base = World::lines(&mut reloaded)[0]["id"].as_u64().unwrap() * STRIDE;
    for (i, shot) in World::lines(&mut reloaded)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .enumerate()
    {
        shot["id"] = json!(base + i as u64);
    }
    World::lines(&mut reloaded)[0]["next_shot_id"] = json!(base + 4);
    let rebuilt = undo.apply(&reloaded).unwrap();
    let shots = rebuilt["lines"][0]["shots"].as_array().unwrap();
    let ids: std::collections::HashSet<u64> =
        shots.iter().map(|s| s["id"].as_u64().unwrap()).collect();
    assert_eq!(ids.len(), 5, "ids must stay unique: {shots:?}");
    let next = rebuilt["lines"][0]["next_shot_id"].as_u64().unwrap();
    assert!(ids.iter().all(|id| *id < next));
    assert_eq!(shots[2]["uuid"], before["lines"][0]["shots"][2]["uuid"]);
}

#[test]
fn a_line_counter_rewound_by_a_reload_is_raised_past_a_restored_line() {
    let mut world = World::new(19);
    let before = world.project(3);
    let mut after = before.clone();
    // Delete the highest line; the counter stays.
    World::lines(&mut after).pop();
    let undo = ProjectDelta::between(&after, &before).unwrap();
    // A reload recomputes the counter as max + 1, the deleted line's id.
    let mut reloaded = after.clone();
    let max = World::lines(&mut reloaded)
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .max()
        .unwrap();
    reloaded["next_line_id"] = json!(max + 1);
    let rebuilt = undo.apply(&reloaded).unwrap();
    let restored_max = rebuilt["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["id"].as_u64().unwrap())
        .max()
        .unwrap();
    assert!(rebuilt["next_line_id"].as_u64().unwrap() > restored_max);
}

// === Hostile deltas: a delta is read back from the file ===

fn hostile(text: &str) -> ProjectDelta {
    serde_json::from_str(text).unwrap()
}

/// `delta` breaks a rule, and the one `why` names.
fn refused_for(delta: &ProjectDelta, why: &str) {
    let checked = delta.check_shape();
    assert!(
        checked.is_err(),
        "a delta that breaks a rule passed: {delta:?}"
    );
    if let Err(reason) = checked {
        assert!(reason.contains(why), "refused for another rule: {reason}");
    }
}

fn line_count(p: &Value) -> usize {
    p["lines"].as_array().unwrap().len()
}

/// A delta must not build more than its source holds plus what it carries,
/// or a small row grows the project it is applied to without bound.
#[test]
fn a_delta_that_keeps_a_line_twice_is_refused() {
    let mut world = World::new(23);
    let p = world.project(2);
    let n = line_count(&p);
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"k":[0,{n}]}},{{"k":[0,{n}]}}]]}}"#
    ));
    refused_for(&delta, "reads a source line twice");
    assert!(delta.apply(&p).is_err(), "applied: the lines doubled");
}

#[test]
fn a_delta_that_copies_a_shot_twice_is_refused() {
    let mut world = World::new(29);
    let p = world.project(2);
    let n = line_count(&p);
    let id = p["lines"][0]["id"].as_u64().unwrap();
    let copy = r#"{"c":{"sec":0,"at":0,"n":1}}"#;
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"e":{{"at":0,"id":{id},"s":[{copy},{copy}]}}}},{{"k":[1,{}]}}]]}}"#,
        n - 1
    ));
    refused_for(&delta, "copies a source shot twice");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn a_delta_that_copies_out_of_a_kept_line_is_refused() {
    let mut world = World::new(31);
    let p = world.project(2);
    let n = line_count(&p);
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
fn a_new_line_with_an_id_past_the_bound_is_refused_not_overflowed() {
    let mut world = World::new(41);
    let p = world.project(1);
    let n = line_count(&p);
    let delta = hostile(&format!(
        r#"{{"s":[{n},[{{"k":[0,{n}]}},{{"n":{{"f":{{"id":18446744073709551615,"next_shot_id":0}},"s":[]}}}}]]}}"#
    ));
    refused_for(&delta, "past the bound");
    assert!(delta.apply(&p).is_err());
}

#[test]
fn the_holder_walk_refuses_a_delta_that_breaks_the_rules() {
    use std::rc::Rc;
    let view: Vec<super::history::HolderLine> = vec![(1, Rc::new(vec![Some(Rc::from("u-1"))]))];
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
    let mut line = world.line(4);
    let twin = line["shots"][0]["uuid"].clone();
    line["shots"][2]["uuid"] = twin;
    World::lines(&mut before).push(line);
    before["next_line_id"] = json!(world.next_line);
    let mut after = before.clone();
    let mut tail = World::lines(&mut after)[0].clone();
    let shots = World::lines(&mut after)[0]["shots"].as_array_mut().unwrap();
    let moved: Vec<Value> = shots.drain(2..).collect();
    tail["id"] = json!(world.next_line);
    tail["shots"] = json!(moved);
    World::lines(&mut after).push(tail);
    after["next_line_id"] = json!(world.next_line + 1);
    for (source, target) in [(&before, &after), (&after, &before)] {
        let delta = round_trip(source, target);
        delta.check_shape().unwrap();
    }
}

/// A copy carries no uuid of its own, so a rename the load's heal makes in
/// the line a shot is copied into is pinned onto the copy as an edit.
#[test]
fn a_renamed_id_is_pinned_onto_the_shot_a_delta_copies() {
    use std::rc::Rc;
    let mut world = World::new(47);
    let mut source = world.project(0);
    let first = world.line(3);
    let second = world.line(1);
    World::lines(&mut source).push(first);
    World::lines(&mut source).push(second);
    source["next_line_id"] = json!(world.next_line + 1);
    // Move the first line's last shot into a new line.
    let mut target = source.clone();
    let moved = World::lines(&mut target)[0]["shots"]
        .as_array_mut()
        .unwrap()
        .pop()
        .unwrap();
    let uuid = moved["uuid"].as_str().unwrap().to_string();
    let new_line = world.next_line;
    let mut fresh = World::lines(&mut target)[1].clone();
    fresh["id"] = json!(new_line);
    fresh["shots"] = json!([moved]);
    World::lines(&mut target).push(fresh);
    let mut delta = ProjectDelta::between(&source, &target).unwrap();

    let renamed = format!("{uuid}-renamed");
    let view: Vec<super::history::HolderLine> = source["lines"]
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
    let target_line = u32::try_from(new_line).unwrap();
    let rename =
        |line: u32, old: &str| (line == target_line && old == uuid).then(|| renamed.clone());
    assert!(
        delta.rename_copies(&view, &rename),
        "the moved shot is a copy, so the rename has to be pinned onto it"
    );
    let rebuilt = delta.apply(&source).unwrap();
    let mut expected = target.clone();
    World::lines(&mut expected)[2]["shots"][0]["uuid"] = json!(renamed);
    assert!(same(&rebuilt, &expected), "rebuilt: {rebuilt}");
}

/// A line whose text differs but whose value is the same stays where it
/// is, so its shots are not copied anywhere else: a new line holding a
/// shot under one of its uuids carries that shot literally.
#[test]
fn a_line_kept_by_value_lends_no_shot_to_another() {
    let source = r#"{"name":"p","next_line_id":3,"lines":[{"id":1,"name":"A","next_shot_id":100002,"shots":[{"id":100000,"uuid":"u-1","length":2.0},{"id":100001,"uuid":"u-2","length":3.0}]}]}"#;
    // Line 1 again, with its keys in another order; and a new line 2
    // whose shot shares a uuid with one of line 1's.
    let target = r#"{"name":"p","next_line_id":3,"lines":[{"name":"A","id":1,"next_shot_id":100002,"shots":[{"uuid":"u-1","id":100000,"length":2.0},{"id":100001,"uuid":"u-2","length":3.0}]},{"id":2,"name":"B","next_shot_id":200001,"shots":[{"id":200000,"uuid":"u-2","length":3.0}]}]}"#;
    let delta = ProjectDelta::between_texts(source, target).unwrap();
    delta.check_shape().unwrap();
    let rebuilt = delta.apply(&serde_json::from_str(source).unwrap()).unwrap();
    assert!(
        same(&rebuilt, &serde_json::from_str(target).unwrap()),
        "rebuilt: {rebuilt}"
    );
}

/// Two target lines under one id (no live project holds such a pair, but
/// a delta must still encode it): the second is written as a new line, so
/// no source line is read twice.
#[test]
fn a_repeated_line_id_reads_its_source_once() {
    let mut world = World::new(53);
    let source = world.project(2);
    let mut target = source.clone();
    let mut twin = World::lines(&mut target)[0].clone();
    twin["name"] = json!("twin");
    World::lines(&mut target)[0]["name"] = json!("renamed");
    World::lines(&mut target).push(twin);
    let delta = ProjectDelta::between(&source, &target).unwrap();
    delta.check_shape().unwrap();
    // The twin's shots repeat the first line's ids, so the apply settles
    // them to fresh ones; the lines themselves are all there, in order.
    let rebuilt = delta.apply(&source).unwrap();
    let names = |p: &Value| -> Vec<String> {
        p["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(names(&rebuilt), names(&target));
}

/// A line of shots with the given uuids, its ids in its id window.
fn uuid_line(id: u64, uuids: &[&str]) -> Value {
    let shots: Vec<Value> = uuids
        .iter()
        .enumerate()
        .map(|(i, uuid)| json!({"id": id * STRIDE + i as u64, "uuid": uuid, "length": 1.5 + i as f64}))
        .collect();
    json!({"id": id, "name": format!("L{id}"), "next_shot_id": id * STRIDE + uuids.len() as u64, "shots": shots})
}

/// The station key of the station after `after` (the line start for `None`).
fn station_key(line: u64, after: Option<&str>) -> Value {
    match after {
        None => json!({"line_id": line, "anchor": "LineStart"}),
        Some(uuid) => json!({"line_id": line, "anchor": {"AfterShot": uuid}}),
    }
}

/// One `[key, value(i)]` pair per station of `line`, as `station_names` and
/// `station_ids` hold one for every station.
fn per_station(line: u64, uuids: &[&str], value: impl Fn(usize) -> Value) -> Vec<Value> {
    (0..=uuids.len())
        .map(|i| {
            let after = i.checked_sub(1).map(|k| uuids[k]);
            json!([station_key(line, after), value(i)])
        })
        .collect()
}

/// Every station of every line named and numbered, as #804's dense
/// `station_names` / `station_ids` are.
fn identified(lines: &[(u64, Vec<&str>)], comments: Vec<Value>) -> Value {
    let mut names = Vec::new();
    let mut ids = Vec::new();
    for (line, uuids) in lines {
        names.extend(per_station(*line, uuids, |i| json!(format!("{line}.{i}"))));
        ids.extend(per_station(*line, uuids, |i| json!(line * 100 + i as u64)));
    }
    json!({
        "name": "p",
        "next_line_id": 3,
        "lines": lines.iter().map(|(id, uuids)| uuid_line(*id, uuids)).collect::<Vec<_>>(),
        "station_names": names,
        "station_ids": ids,
        "station_comments": comments,
    })
}

/// A Reverse and a Delete Line re-key or drop a pair per station in every
/// station-keyed member. Each pair whose key names a station of its state is
/// written as a position and rebuilt from that state's shots, exactly, in
/// both directions; a key that names no station stays literal.
#[test]
fn station_keyed_pairs_travel_as_positions_and_rebuild_exactly() {
    let forward = vec!["u-a", "u-b", "u-c", "u-d"];
    let mut reversed = forward.clone();
    reversed.reverse();
    let dangling = json!([station_key(1, Some("u-gone")), "a note on no station"]);
    let source = identified(
        &[(1, forward.clone()), (2, vec!["u-e", "u-f"])],
        vec![dangling],
    );
    // Line 1 reversed (every station keeps its name and id, under a new
    // key), line 2 deleted with its pairs, the dangling comment dropped.
    let mut target = identified(&[(1, reversed.clone())], Vec::new());
    for member in ["station_names", "station_ids"] {
        let pairs = target[member].as_array_mut().unwrap();
        let values: Vec<Value> = pairs.iter().map(|p| p[1].clone()).rev().collect();
        for (pair, value) in pairs.iter_mut().zip(values) {
            pair[1] = value;
        }
    }
    for (from, to) in [(&source, &target), (&target, &source)] {
        let delta = ProjectDelta::between(from, to).unwrap();
        delta.check_shape().unwrap();
        let rebuilt = delta.apply(from).unwrap();
        for member in ["station_names", "station_ids", "station_comments"] {
            let sorted = |p: &Value| {
                let mut pairs = p[member].as_array().cloned().unwrap_or_default();
                pairs.sort_by_key(ToString::to_string);
                pairs
            };
            assert!(
                same(&Value::Array(sorted(&rebuilt)), &Value::Array(sorted(to))),
                "{member}: {} vs {}",
                rebuilt[member],
                to[member]
            );
        }
        let text = serde_json::to_string(&delta).unwrap();
        let back: ProjectDelta = serde_json::from_str(&text).unwrap();
        assert_eq!(back, delta);
        let positional = |member: &str| match delta.fields.get(member) {
            Some(super::Edit::Bag {
                del,
                add,
                station_del,
                station_add,
            }) => (del.len() + add.len(), station_del.len() + station_add.len()),
            _ => (0, 0),
        };
        // Line 1: 5 stations re-keyed (5 out, 5 in); line 2: 3 stations.
        assert_eq!(positional("station_names"), (0, 13), "{text}");
        assert_eq!(positional("station_ids"), (0, 13), "{text}");
        // The dangling key names no station of the state it is in.
        assert_eq!(positional("station_comments"), (1, 0), "{text}");
        assert_eq!(
            text.matches("AfterShot").count(),
            1,
            "only the dangling key spells out a uuid: {text}"
        );
    }
}

/// A positional pair is rebuilt from the state the step meets: an addition
/// whose station the target does not hold refuses the step rather than
/// writing a key for a station that is not there.
#[test]
fn a_positional_pair_past_its_line_refuses_the_step() {
    let source = identified(&[(1, vec!["u-a"])], Vec::new());
    let mut delta = ProjectDelta::default();
    delta.fields.insert(
        "station_names".to_string(),
        super::Edit::Bag {
            del: Vec::new(),
            add: Vec::new(),
            station_del: vec![(1, 7, json!("x"))],
            station_add: vec![(0, 1, 9, json!("y"))],
        },
    );
    let error = delta.apply(&source).unwrap_err();
    assert!(error.contains("station 9 of line 1"), "{error}");
    // A removal of a station the source does not hold is skipped, as a
    // literal removal of an absent element is.
    if let Some(super::Edit::Bag { station_add, .. }) = delta.fields.get_mut("station_names") {
        station_add.clear();
    }
    let rebuilt = delta.apply(&source).unwrap();
    assert!(same(&rebuilt["station_names"], &source["station_names"]));
}
