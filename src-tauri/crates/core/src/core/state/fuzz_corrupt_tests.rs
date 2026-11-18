//! `FORMAT_CORRUPT=1`: damage ONE persisted history row between a save and the
//! reopen, and hold the load to the rule docs/undo-redo.md states for it - a
//! compact history is usable only whole, so it either loads and every step lands
//! exactly on the state the model recorded, or it is dropped whole (both stacks
//! empty, `history_discarded()` true). Never "loads, then a step refuses".
//!
//! A row is `{"v2": <UndoData>}`, gzip-compressed, in the `undo_stack` or
//! `redo_stack` table (core/undo/persistence.rs, persistence/sqlite/undo.rs). The
//! catalog below edits the JSON of a compact (`Delta`) row, rewrites the envelope,
//! or damages the stored bytes, and every mutation has a DETERMINATE right answer:
//!
//! | kind | what it does | the load must |
//! |---|---|---|
//! | `line-count` | the delta's recorded source line count +/- 1 | refuse it (fit, or `check_shape` for -1) - or keep it and step exactly, which it cannot |
//! | `edit-id` | a line Edit run names another line id | refuse it (fit) |
//! | `copy-past-end` | a shot Copy run starts past the end of its source line | refuse it (fit) |
//! | `repeated-keep` | a line Keep run appears twice | refuse it (`check_shape`: a line read twice) |
//! | `keep-past-end` | a line Keep run shifted past the source's end | refuse it (`check_shape`) |
//! | `arr-length` | a positional array edit records another source length | refuse it, since every step through it refuses |
//! | `bag-absent-del` | a bag edit removes a value no state holds | KEEP it and step exactly: removing an absent value is skipped by design |
//! | `healed-member-del` | the delta deletes `validation`, which it did not edit | KEEP it and step exactly: the restore carries the live value back (the `healed` re-read path) |
//! | `v3-envelope` | `{"v3": ...}`, a later build's envelope | refuse it (an unknown version is damage) |
//! | `garbage-bytes` | the stored blob is random bytes | refuse it |
//! | `truncated-gzip` | the stored gzip member is cut in half | refuse it |
//!
//! Corruptions with NO determinate answer are deliberately not generated: a bag
//! value removed from a delta, a Keep shifted to another in-range span, a changed
//! literal. Each is a valid, fitting delta to a DIFFERENT state, which nothing
//! short of a checksum can tell from a real one, so no oracle could say whether
//! the run is right.
//!
//! Every draw comes from the coverage stream (`Fuzzer::aux`), never the run's own.

use std::fmt;
use std::path::Path;

use serde_json::{Map, Value};

use super::Rng;
use crate::persistence::Database;

/// One rewritten row, named for the move log and the failure payload.
#[derive(Clone, Debug)]
pub(super) struct Corruption {
    kind: Kind,
    /// `undo_stack` or `redo_stack`.
    table: &'static str,
    /// The row's position in its table, oldest first, and how many there were.
    row: usize,
    rows: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    LineCount,
    EditId,
    CopyPastEnd,
    RepeatedKeep,
    KeepPastEnd,
    ArrLength,
    BagAbsentDel,
    HealedMemberDel,
    LaterEnvelope,
    GarbageBytes,
    TruncatedGzip,
}

const KINDS: [Kind; 11] = [
    Kind::LineCount,
    Kind::EditId,
    Kind::CopyPastEnd,
    Kind::RepeatedKeep,
    Kind::KeepPastEnd,
    Kind::ArrLength,
    Kind::BagAbsentDel,
    Kind::HealedMemberDel,
    Kind::LaterEnvelope,
    Kind::GarbageBytes,
    Kind::TruncatedGzip,
];

impl Kind {
    const fn name(self) -> &'static str {
        match self {
            Self::LineCount => "line-count",
            Self::EditId => "edit-id",
            Self::CopyPastEnd => "copy-past-end",
            Self::RepeatedKeep => "repeated-keep",
            Self::KeepPastEnd => "keep-past-end",
            Self::ArrLength => "arr-length",
            Self::BagAbsentDel => "bag-absent-del",
            Self::HealedMemberDel => "healed-member-del",
            Self::LaterEnvelope => "v3-envelope",
            Self::GarbageBytes => "garbage-bytes",
            Self::TruncatedGzip => "truncated-gzip",
        }
    }
}

impl fmt::Display for Corruption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} row {} of {} ({})",
            self.table,
            self.row,
            self.rows,
            self.kind.name()
        )
    }
}

impl Corruption {
    /// The row breaks a rule the READ enforces (the envelope, the JSON, the
    /// gzip member, or `check_shape`), so the load cannot have taken it as part
    /// of a history - whatever the walk does or does not see.
    pub(super) fn must_be_refused(&self) -> bool {
        matches!(
            self.kind,
            Kind::RepeatedKeep
                | Kind::KeepPastEnd
                | Kind::LaterEnvelope
                | Kind::GarbageBytes
                | Kind::TruncatedGzip
        )
    }

    /// The engine tolerates this by design, so the load has no business
    /// dropping the history over it.
    pub(super) fn must_be_kept(&self) -> bool {
        matches!(self.kind, Kind::BagAbsentDel | Kind::HealedMemberDel)
    }

    /// The kind's name, for a failure signature.
    pub(super) const fn kind_name(&self) -> &'static str {
        self.kind.name()
    }
}

/// A history row as the Database API hands it back: command, payload, stamp.
type Row = (String, String, Option<String>);

/// Damage one row of the saved file at `path`, chosen and mutated from `aux`.
/// `None` when the file holds no history row to damage.
pub(super) fn corrupt_one_row(path: &Path, aux: &mut Rng) -> Option<Corruption> {
    let db = Database::open(path).expect("open the saved file to corrupt it");
    let strip = |rows: Vec<(i64, String, String, Option<String>)>| -> Vec<Row> {
        rows.into_iter()
            .map(|(_, command, data, stamp)| (command, data, stamp))
            .collect()
    };
    let mut undo = strip(
        db.load_undo_stack_with_timestamps()
            .expect("read the undo rows"),
    );
    let mut redo = strip(
        db.load_redo_stack_with_timestamps()
            .expect("read the redo rows"),
    );

    // Every (kind, table, row) the catalog can apply, in a fixed order.
    let mut candidates: Vec<(Kind, bool, usize)> = Vec::new();
    for (is_redo, rows) in [(false, &undo), (true, &redo)] {
        for (index, (_, data, _)) in rows.iter().enumerate() {
            let parsed: Option<Value> = serde_json::from_str(data).ok();
            for kind in KINDS {
                let applies = match kind {
                    Kind::LaterEnvelope | Kind::GarbageBytes | Kind::TruncatedGzip => true,
                    _ => parsed.as_ref().is_some_and(|value| {
                        let mut probe = value.clone();
                        mutate_json(&mut probe, kind, &mut Rng::new(0))
                    }),
                };
                if applies {
                    candidates.push((kind, is_redo, index));
                }
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    // A kind first, then a row of it, so the rarely-applicable kinds are drawn
    // as often as the always-applicable ones.
    let mut kinds: Vec<Kind> = candidates.iter().map(|(kind, ..)| *kind).collect();
    kinds.dedup();
    let kind = kinds[aux.below(kinds.len())];
    let rows_of: Vec<(bool, usize)> = candidates
        .iter()
        .filter(|(candidate, ..)| *candidate == kind)
        .map(|(_, is_redo, index)| (*is_redo, *index))
        .collect();
    let (is_redo, index) = rows_of[aux.below(rows_of.len())];
    let table = if is_redo { "redo_stack" } else { "undo_stack" };
    let corruption = Corruption {
        kind,
        table,
        row: index,
        rows: if is_redo { redo.len() } else { undo.len() },
    };

    match kind {
        Kind::GarbageBytes | Kind::TruncatedGzip => {
            drop(db);
            rewrite_blob(path, table, index, kind, aux);
        }
        _ => {
            let rows = if is_redo { &mut redo } else { &mut undo };
            let mut value: Value =
                serde_json::from_str(&rows[index].1).unwrap_or(Value::String(String::new()));
            if kind == Kind::LaterEnvelope {
                value = later_envelope(value);
            } else {
                let changed = mutate_json(&mut value, kind, aux);
                assert!(changed, "a candidate row stopped accepting {}", kind.name());
            }
            rows[index].1 = value.to_string();
            db.replace_history_stacks_with_timestamps(undo, redo)
                .expect("write the corrupted rows back");
        }
    }
    Some(corruption)
}

/// `{"v2": x}` as `{"v3": x}`: what a later build's row looks like to this one.
fn later_envelope(value: Value) -> Value {
    let inner = match value {
        Value::Object(mut map) => map.remove("v2").unwrap_or(Value::Null),
        other => other,
    };
    let mut out = Map::new();
    out.insert("v3".to_string(), inner);
    Value::Object(out)
}

/// Damage the stored bytes of one row's payload in place.
fn rewrite_blob(path: &Path, table: &str, row: usize, kind: Kind, aux: &mut Rng) {
    let conn = rusqlite::Connection::open(path).expect("open the saved file's tables");
    let sequence = i64::try_from(row).expect("a row index fits a sequence");
    let blob: Vec<u8> = conn
        .query_row(
            &format!("SELECT undo_data_json FROM {table} WHERE sequence = ?1"),
            [sequence],
            |r| r.get(0),
        )
        .expect("read the row's stored payload");
    let damaged: Vec<u8> = if kind == Kind::TruncatedGzip {
        blob[..blob.len() / 2].to_vec()
    } else {
        // Random bytes that do not start with the gzip magic, so the reader
        // takes them as a plain-text row (its other branch).
        let length = 16 + aux.below(48);
        let mut bytes: Vec<u8> = (0..length)
            .map(|_| u8::try_from(aux.below(256)).unwrap_or(0))
            .collect();
        bytes[0] = b'#';
        bytes
    };
    conn.execute(
        &format!("UPDATE {table} SET undo_data_json = ?1 WHERE sequence = ?2"),
        rusqlite::params![damaged, sequence],
    )
    .expect("write the damaged payload");
}

/// The delta of a compact row, `row["v2"]["Delta"]["d"]`.
fn delta_of(row: &mut Value) -> Option<&mut Map<String, Value>> {
    row.get_mut("v2")?
        .get_mut("Delta")?
        .get_mut("d")?
        .as_object_mut()
}

/// Apply `kind` to a parsed row. Returns whether the row had what the kind
/// needs (and so was changed); a probe passes a throwaway generator.
fn mutate_json(row: &mut Value, kind: Kind, aux: &mut Rng) -> bool {
    let Some(delta) = delta_of(row) else {
        return false;
    };
    match kind {
        Kind::LineCount => {
            let Some(Value::Array(lines)) = delta.get_mut("s") else {
                return false;
            };
            let Some(count) = lines.first().and_then(Value::as_u64) else {
                return false;
            };
            let next = if count == 0 || aux.chance(50) {
                count + 1
            } else {
                count - 1
            };
            lines[0] = Value::from(next);
            true
        }
        Kind::EditId => pick_line_run(delta, "e", aux).is_some_and(|run| {
            let Some(id) = run.get("id").and_then(Value::as_u64) else {
                return false;
            };
            // Ids are unique, so any other id names a different line - or none.
            run.insert("id".to_string(), Value::from(id + 1 + aux.below(7) as u64));
            true
        }),
        Kind::CopyPastEnd => {
            let mut copies = shot_copies(delta);
            if copies.is_empty() {
                return false;
            }
            let at = aux.below(copies.len());
            copies[at].insert("at".to_string(), Value::from(1_000_000_u64));
            copies[at].remove("rev");
            true
        }
        Kind::RepeatedKeep => {
            let Some(runs) = line_runs(delta) else {
                return false;
            };
            let keeps: Vec<usize> = runs
                .iter()
                .enumerate()
                .filter(|(_, run)| keep_of(run).is_some_and(|(_, len)| len > 0))
                .map(|(index, _)| index)
                .collect();
            if keeps.is_empty() {
                return false;
            }
            let at = keeps[aux.below(keeps.len())];
            let copy = runs[at].clone();
            runs.insert(at + 1, copy);
            true
        }
        Kind::KeepPastEnd => {
            let count = delta
                .get("s")
                .and_then(|s| s.get(0))
                .and_then(Value::as_u64);
            let Some(count) = count else {
                return false;
            };
            let Some(runs) = line_runs(delta) else {
                return false;
            };
            let keeps: Vec<usize> = runs
                .iter()
                .enumerate()
                .filter(|(_, run)| keep_of(run).is_some_and(|(_, len)| len > 0))
                .map(|(index, _)| index)
                .collect();
            if keeps.is_empty() {
                return false;
            }
            let at = keeps[aux.below(keeps.len())];
            let (_, len) = keep_of(&runs[at]).unwrap_or((0, 1));
            // One past the end: the last line it reads is the source's count.
            let shifted = (count + 1).saturating_sub(len);
            runs[at] = serde_json::json!({ "k": [shifted, len] });
            true
        }
        Kind::ArrLength => {
            let mut arrays = 0_usize;
            for_each_edit(delta, &mut |edit| arrays += usize::from(is_arr(edit)));
            if arrays == 0 {
                return false;
            }
            let target = aux.below(arrays);
            let grow = aux.chance(50);
            let mut seen = 0;
            for_each_edit(delta, &mut |edit| {
                if is_arr(edit) {
                    if seen == target {
                        if let Some(Value::Array(pair)) = edit.get_mut("a") {
                            let len = pair.first().and_then(Value::as_u64).unwrap_or(0);
                            let next = if grow || len == 0 { len + 1 } else { len - 1 };
                            pair[0] = Value::from(next);
                        }
                    }
                    seen += 1;
                }
            });
            true
        }
        Kind::BagAbsentDel => {
            let mut bags = 0_usize;
            for_each_edit(delta, &mut |edit| bags += usize::from(is_bag(edit)));
            if bags == 0 {
                return false;
            }
            let target = aux.below(bags);
            let marker = format!("__fuzz-absent-{}__", aux.below(10_000));
            let mut seen = 0;
            for_each_edit(delta, &mut |edit| {
                if !is_bag(edit) {
                    return;
                }
                if seen == target {
                    if let Some(Value::Object(bag)) = edit.get_mut("b") {
                        let del = bag
                            .entry("d".to_string())
                            .or_insert_with(|| Value::Array(Vec::new()));
                        if let Value::Array(values) = del {
                            values.push(Value::String(marker.clone()));
                        }
                    }
                }
                seen += 1;
            });
            true
        }
        Kind::HealedMemberDel => {
            let fields = delta
                .entry("f".to_string())
                .or_insert_with(|| Value::Object(Map::new()));
            let Value::Object(fields) = fields else {
                return false;
            };
            if fields.contains_key("validation") {
                return false;
            }
            fields.insert("validation".to_string(), Value::String("d".to_string()));
            true
        }
        Kind::LaterEnvelope | Kind::GarbageBytes | Kind::TruncatedGzip => false,
    }
}

/// The delta's line runs, `d["s"][1]`.
fn line_runs(delta: &mut Map<String, Value>) -> Option<&mut Vec<Value>> {
    delta.get_mut("s")?.get_mut(1)?.as_array_mut()
}

/// `(at, len)` of a line Keep run, `{"k": [at, len]}`.
fn keep_of(run: &Value) -> Option<(u64, u64)> {
    let pair = run.get("k")?.as_array()?;
    Some((pair.first()?.as_u64()?, pair.get(1)?.as_u64()?))
}

/// One line run of shape `{tag: {...}}`, drawn from those there are.
fn pick_line_run<'a>(
    delta: &'a mut Map<String, Value>,
    tag: &str,
    aux: &mut Rng,
) -> Option<&'a mut Map<String, Value>> {
    let runs = line_runs(delta)?;
    let matching: Vec<usize> = runs
        .iter()
        .enumerate()
        .filter(|(_, run)| run.get(tag).is_some_and(Value::is_object))
        .map(|(index, _)| index)
        .collect();
    if matching.is_empty() {
        return None;
    }
    let at = matching[aux.below(matching.len())];
    runs[at].get_mut(tag)?.as_object_mut()
}

/// Every shot Copy run with at least one shot, `{"c": {...}}`, inside the
/// line Edit and New runs.
fn shot_copies(delta: &mut Map<String, Value>) -> Vec<&mut Map<String, Value>> {
    let mut out = Vec::new();
    let Some(runs) = line_runs(delta) else {
        return out;
    };
    for run in runs.iter_mut() {
        let Some(body) = run
            .as_object_mut()
            .and_then(|object| object.values_mut().next())
            .and_then(Value::as_object_mut)
        else {
            continue;
        };
        let Some(Value::Array(shots)) = body.get_mut("s") else {
            continue;
        };
        for shot_run in shots.iter_mut() {
            if let Some(Value::Object(copy)) = shot_run.get_mut("c") {
                if copy.get("n").and_then(Value::as_u64).unwrap_or(0) > 0 {
                    out.push(copy);
                }
            }
        }
    }
    out
}

/// `{"a": [len, [runs]]}`: a positional array edit.
fn is_arr(edit: &Value) -> bool {
    edit.as_object().is_some_and(|object| {
        object.len() == 1
            && object
                .get("a")
                .and_then(Value::as_array)
                .is_some_and(|pair| pair.len() == 2 && pair[0].is_u64() && pair[1].is_array())
    })
}

/// `{"b": {...}}`: a bag edit.
fn is_bag(edit: &Value) -> bool {
    edit.as_object()
        .is_some_and(|object| object.len() == 1 && object.get("b").is_some_and(Value::is_object))
}

/// Visit every `Edit` the delta holds, outermost first: the top-level member
/// edits, each line Edit run's member edits, each shot copy's edits, and
/// everything nested in an object or array edit. Literal values (`{"s": v}`, a
/// New run's members, literal shots) are not edits and are not entered.
fn for_each_edit(delta: &mut Map<String, Value>, visit: &mut dyn FnMut(&mut Value)) {
    if let Some(Value::Object(fields)) = delta.get_mut("f") {
        for edit in fields.values_mut() {
            walk_edit(edit, visit);
        }
    }
    let Some(runs) = line_runs(delta) else {
        return;
    };
    for run in runs.iter_mut() {
        let Some(Value::Object(body)) = run.get_mut("e") else {
            continue;
        };
        if let Some(Value::Object(fields)) = body.get_mut("f") {
            for edit in fields.values_mut() {
                walk_edit(edit, visit);
            }
        }
        if let Some(Value::Array(shots)) = body.get_mut("s") {
            for shot_run in shots.iter_mut() {
                let Some(Value::Array(mods)) = shot_run.get_mut("c").and_then(|c| c.get_mut("m"))
                else {
                    continue;
                };
                for pair in mods.iter_mut() {
                    if let Some(edit) = pair.get_mut(1) {
                        walk_edit(edit, visit);
                    }
                }
            }
        }
    }
}

fn walk_edit(edit: &mut Value, visit: &mut dyn FnMut(&mut Value)) {
    visit(edit);
    if let Some(Value::Object(members)) = edit.get_mut("o") {
        for inner in members.values_mut() {
            walk_edit(inner, visit);
        }
        return;
    }
    if is_arr(edit) {
        if let Some(Value::Array(runs)) = edit.get_mut("a").and_then(|pair| pair.get_mut(1)) {
            for run in runs.iter_mut() {
                if let Some(inner) = run.get_mut("m").and_then(|pair| pair.get_mut(1)) {
                    walk_edit(inner, visit);
                }
            }
        }
    }
}

// ===========================================================================
// What FORMAT_CORRUPT found
// ===========================================================================

/// OPEN FINDING (FORMAT_CORRUPT, 15 of 210 corrupted runs over the knob
/// matrix; shrunk from COMBINED seed 13 x300 to 11 moves): a row whose gzip
/// member is damaged drops only ITS stack. `decompress_history_rows_within`
/// fails the whole table, the load substitutes an empty stack for it and
/// raises the discarded-history banner - and then loads the OTHER table
/// as usual, so the project opens with its redo branch (or its undo history)
/// intact under a banner saying the history could not be read.
///
/// Every other damaged row - bad JSON, an unknown envelope, a delta that
/// breaks `check_shape` or does not fit - drops BOTH stacks
/// (`drop_history_with_a_hole`, `drop_history_that_does_not_fit`), which is
/// what docs/undo-redo.md promises: "this build drops a history it cannot read
/// whole". The stack that survives is self-consistent (each stack is a chain
/// of deltas from the live project outwards), so nothing restores the wrong
/// state; what breaks is the contract, the banner's meaning, and the one rule
/// the corrupt-row oracle holds every other path to.
#[test]
#[ignore = "open finding: a damaged gzip row drops only its own stack - see the doc comment"]
fn a_damaged_gzip_row_drops_the_whole_history() {
    use crate::core::Command;
    let _config = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("history.stalx");
    let state = super::super::AppState::new().unwrap();
    assert!(
        state
            .execute(Command::CreateProject {
                name: "Gzip".to_string(),
                location: None,
            })
            .success
    );
    for name in ["a", "b", "c"] {
        assert!(
            state
                .execute(Command::CreateLine {
                    name: Some(name.to_string()),
                    surveyor: None,
                    date: None,
                    color: "#3388FF".to_string(),
                    direction: None,
                    shots: Vec::new(),
                    cave_section_id: None,
                })
                .success
        );
    }
    // Something on BOTH stacks: three entries, one of them undone.
    assert!(state.undo().success);
    assert!(state.save_project(Some(path.clone())).success);
    assert!(state.execute(Command::CloseProject).success);
    rewrite_blob(
        &path,
        "undo_stack",
        0,
        Kind::TruncatedGzip,
        &mut Rng::new(1),
    );
    assert!(state.execute(Command::OpenProject { path }).success);
    assert!(state.history_discarded(), "the damaged row was noticed");
    assert_eq!(
        (state.get_undo_stack().len(), state.get_redo_stack().len()),
        (0, 0),
        "a history reported discarded kept part of itself"
    );
}
