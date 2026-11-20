//! FILE-FORMAT properties at the `AppState` level: what the app's save and
//! open owe each other on a `.stalx` v2 file.
//!
//! The persistence crate already pins the codec on its own
//! (`stalx_v2::format_v2_*_tests`: canonical spellings, float bits, hostile
//! input, the guard and the atomic write in isolation). This suite does not
//! repeat any of that. It drives the app: random surveys built through the
//! command path, with history on both stacks, saved and opened the way the
//! app saves and opens them.
//!
//! 1. **The save is a function of the session.** Save, open in a fresh state,
//!    save again: three generations, the same BYTES (no mask: a v2 save
//!    stamps nothing; `modified` is the survey's own edit time). A re-save
//!    of an unchanged project rewrites the same bytes too.
//! 2. **The open is a function of the file.** Several opens of one file give
//!    the same survey (canonical text), verdict, Computed View (bit for bit),
//!    id allocators and history (lengths and labels).
//! 3. **The guard** (plan 6.5). A file changed on disk since it was opened
//!    (a same-size byte flip, a truncation, another valid project, a delete)
//!    refuses the plain save and the local-only save with
//!    `EXTERNALLY_MODIFIED` and is left exactly as the other program left
//!    it; the Overwrite answer writes the survey and the next plain save
//!    succeeds. A touch, or a rewrite with the same bytes, is not a change.
//! 4. **Unknown content** (plan 6.4). Tier 2 (an unknown top-level member,
//!    unknown keys in `project`, `settings` and `summary`) survives an edit
//!    and a save verbatim, and a local-only save too; tier 3 (an unknown key
//!    in `local`) is dropped and the file opens; tier 1 (an unknown key in a
//!    station record) opens read-only: edits and saves are refused, a display
//!    toggle still saves locally, and the body is not rewritten.
//! 5. **Old file shapes.** A legacy SQLite file surgically degraded the ways
//!    older builds left them (no gauge columns, no undo tables, no station
//!    names or ids table, disagreeing version markers) opens through the
//!    frozen reader, and its first save upgrades it to a v2 file that reopens
//!    as the same survey; an impossible version/schema pairing is refused
//!    cleanly, with the file untouched.
//!
//! # Data
//!
//! Everything is generated in code. No fixture is read, copied or derived from.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use tempfile::TempDir;

use super::robustness_kit::{
    build_project, canonical, canonical_snapshot, differences, first_difference, observe,
    open_fresh, panic_text, random_offset, random_shot, read_targets, write_legacy, Observed, Rng,
    COLORS,
};
use super::{AppState, EXTERNALLY_MODIFIED_CODE, READ_ONLY_FILE_CODE};
use crate::core::{Command, CommandResult};
use crate::models::{
    ClosureStrategy, GpsCoordinates, LineUpdates, ShotUpdates, StationRef,
};
use crate::persistence::stalx_v2::{self, Format};
use crate::test_helpers::isolated_config;

// ===========================================================================
// Building a project to save
// ===========================================================================

/// A random project with history on both stacks (both are persisted, and a
/// run that saved an empty history would prove nothing about them).
pub(super) fn build_state(seed: u32, closures: bool, moves: usize) -> Option<AppState> {
    let state = AppState::new().ok()?;
    let mut rng = Rng::new(seed);
    build_project(&state, &mut rng, closures);
    for _ in 0..moves {
        mutate(&state, &mut rng);
    }
    for _ in 0..2 {
        state.undo();
    }
    Some(state)
}

/// One random command over a broad set of what the file holds: lines,
/// shots, placements (Move Line), station comments and names, junctions,
/// GPS fixes with an accuracy, loop closures, Sections, reverses, line
/// metadata.
pub(super) fn mutate(state: &AppState, rng: &mut Rng) -> CommandResult {
    let Some(targets) = read_targets(state) else {
        return CommandResult::failure("no project".to_string());
    };
    if targets.lines.is_empty() {
        return CommandResult::failure("no line".to_string());
    }
    let line = targets.lines[rng.below(targets.lines.len())].0;
    let station = |rng: &mut Rng| {
        let (l, i) = targets.stations[rng.below(targets.stations.len())];
        StationRef::new(l, i)
    };
    let cmd = match rng.below(11) {
        0 => {
            let count = 1 + rng.below(3);
            Command::CreateLine {
                name: Some(format!("fmt-{}", rng.range(1000, 9999))),
                surveyor: None,
                date: None,
                color: (*rng.pick(COLORS)).to_string(),
                direction: None,
                shots: (0..count).map(|_| random_shot(rng)).collect(),
                cave_section_id: None,
            }
        }
        1 => {
            let shots = &targets.lines[rng.below(targets.lines.len())];
            let after =
                (!shots.1.is_empty() && rng.chance(50)).then(|| shots.1[rng.below(shots.1.len())]);
            Command::CreateShot {
                line_id: shots.0,
                shot: random_shot(rng),
                after_shot_id: after,
                before_shot_id: None,
            }
        }
        2 => {
            let Some((line_id, shot_id)) = targets.pick_shot(rng) else {
                return CommandResult::failure("no shot".to_string());
            };
            Command::UpdateShot {
                line_id,
                shot_id,
                updates: ShotUpdates {
                    length: Some(rng.float(1.0, 15.0)),
                    comment: rng
                        .chance(30)
                        .then(|| Some(format!("note \"{}\"\t\\", rng.range(0, 99)))),
                    ..ShotUpdates::default()
                },
            }
        }
        3 => Command::MoveLine {
            line_id: line,
            offset: random_offset(rng),
        },
        4 => Command::SetStationComment {
            station_ref: station(rng),
            comment: Some(format!("fmt-note-{}", rng.range(0, 9999))),
        },
        5 => Command::SetStationName {
            station_ref: station(rng),
            name: format!("N{}", rng.range(0, 999)),
        },
        6 => {
            let a = station(rng);
            let others: Vec<(u32, u32)> = targets
                .stations
                .iter()
                .copied()
                .filter(|s| s.0 != a.line_id)
                .collect();
            if others.is_empty() {
                return CommandResult::failure("one line".to_string());
            }
            let (l, i) = others[rng.below(others.len())];
            Command::MergeStations {
                station_a: a,
                station_b: StationRef::new(l, i),
            }
        }
        7 => {
            let Ok(gps) = GpsCoordinates::new(
                20.4 + rng.float(-0.001, 0.001),
                -87.3 + rng.float(-0.001, 0.001),
                0.0,
            ) else {
                return CommandResult::failure("gps".to_string());
            };
            Command::SetZeroDatum {
                station_ref: station(rng),
                coordinates: gps,
                accuracy_m: rng.chance(60).then(|| rng.float(0.0, 50.0)),
                entrance: rng.chance(30).then(|| rng.chance(50)),
            }
        }
        8 => Command::CreateLoopClosure {
            station_a: station(rng),
            station_b: station(rng),
            closure_strategy: *rng.pick(&[ClosureStrategy::Batch, ClosureStrategy::Move]),
        },
        9 => Command::CreateCaveSection {
            name: format!("Section {}", rng.range(0, 99)),
            color: (*rng.pick(COLORS)).to_string(),
            line_ids: vec![line],
        },
        _ => {
            if rng.chance(50) {
                Command::ReverseLine {
                    line_id: line,
                    swap_left_right: rng.chance(50),
                }
            } else {
                Command::UpdateLineMetadata {
                    line_id: line,
                    updates: LineUpdates {
                        name: Some(Some(format!("renamed-{}", rng.range(0, 9999)))),
                        ..LineUpdates::default()
                    },
                }
            }
        }
    };
    state.execute(cmd)
}

/// The undo and redo labels, in stack order.
fn history_labels(state: &AppState) -> (Vec<String>, Vec<String>) {
    let labels = |items: Vec<crate::core::UndoHistoryItem>| {
        items
            .into_iter()
            .map(|i| format!("{} | {} | {:?}", i.command_type, i.description, i.preview_data))
            .collect()
    };
    (
        labels(state.get_undo_stack()),
        labels(state.get_redo_stack()),
    )
}

// ===========================================================================
// PROPERTY 1 and 2
// ===========================================================================

/// What one seed's check found.
struct Checked {
    checked: bool,
    problems: Vec<String>,
}

impl Checked {
    const fn skipped() -> Self {
        Self {
            checked: false,
            problems: Vec::new(),
        }
    }
}

/// Save, open fresh, save: three generations of the same bytes; a re-save of
/// the unchanged project rewrites the same bytes; the file holds the survey
/// the session held.
fn check_save_open_save(seed: u32, closures: bool, moves: usize) -> Checked {
    let Some(state) = build_state(seed, closures, moves) else {
        return Checked::skipped();
    };
    let Ok(temp) = TempDir::new() else {
        return Checked::skipped();
    };
    let mut problems = Vec::new();
    let session = canonical_snapshot(&state);
    let mut generations: Vec<Vec<u8>> = Vec::new();
    let mut live = state;
    for generation in 0..3 {
        let path: PathBuf = temp.path().join(format!("gen{generation}.stalx"));
        let saved = live.save_project(Some(path.clone()));
        if !saved.success {
            problems.push(format!(
                "seed {seed}: generation {generation} could not be saved: {:?}",
                saved.error
            ));
            return Checked {
                checked: true,
                problems,
            };
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return Checked::skipped();
        };
        if generation == 0 {
            // A plain re-save of the unchanged project: the same bytes.
            let again = live.save_project(None);
            let now = std::fs::read(&path).unwrap_or_default();
            if !again.success || now != bytes {
                problems.push(format!(
                    "seed {seed}: re-saving the unchanged project changed its file ({:?})",
                    again.error
                ));
            }
            match stalx_v2::decode(&bytes, &stalx_v2::DecodeOptions::default()) {
                Ok(decoded) if canonical(&decoded.project) == session => {}
                Ok(decoded) => problems.push(format!(
                    "seed {seed}: the file does not hold the session's survey: {}",
                    first_difference(&session, &canonical(&decoded.project))
                )),
                Err(e) => problems.push(format!("seed {seed}: the file does not decode: {e}")),
            }
        }
        generations.push(bytes);
        drop(live);
        let Some(reopened) = open_fresh(&path) else {
            problems.push(format!(
                "seed {seed}: the file the writer just wrote would not open (generation \
                 {generation})"
            ));
            return Checked {
                checked: true,
                problems,
            };
        };
        live = reopened;
    }
    for generation in 1..generations.len() {
        if generations[generation - 1] != generations[generation] {
            problems.push(format!(
                "seed {seed}: save/open generation {generation} differs from generation {} at {}",
                generation - 1,
                first_difference(
                    &String::from_utf8_lossy(&generations[generation - 1]),
                    &String::from_utf8_lossy(&generations[generation])
                )
            ));
        }
    }
    Checked {
        checked: true,
        problems,
    }
}

/// Several opens of one file see the same state, history labels included.
fn check_open_is_a_function(seed: u32, closures: bool, moves: usize, loads: usize) -> Checked {
    let Some(state) = build_state(seed, closures, moves) else {
        return Checked::skipped();
    };
    let Ok(temp) = TempDir::new() else {
        return Checked::skipped();
    };
    let path = temp.path().join("once.stalx");
    if !state.save_project(Some(path.clone())).success {
        return Checked::skipped();
    }
    let in_session = (observe(&state), history_labels(&state));
    drop(state);
    let mut problems = Vec::new();
    let mut seen: Option<(Observed, (Vec<String>, Vec<String>))> = None;
    for load in 0..loads {
        let Some(opened) = open_fresh(&path) else {
            problems.push(format!("seed {seed}: open {load} of the file failed"));
            continue;
        };
        let now = (observe(&opened), history_labels(&opened));
        if load == 0 {
            // The file holds what the session held. Session shot ids are
            // never persisted (plan 1.1), so a line's session allocator may
            // stand higher in the session (a shot created and undone): it is
            // the one allocator a reopen does not carry.
            let persisted = |o: &Observed| {
                let mut o = o.clone();
                o.allocators.retain(|k, _| !k.ends_with(".next_shot_id"));
                o
            };
            let found = differences(&persisted(&in_session.0), &persisted(&now.0));
            if !found.is_empty() {
                problems.push(format!(
                    "seed {seed}: the opened file differs from the session it was saved \
                     from:\n  {}",
                    found.join("\n  ")
                ));
            }
            if in_session.1 != now.1 {
                problems.push(format!(
                    "seed {seed}: the opened history differs from the session's:\n  {:?}\n  {:?}",
                    in_session.1, now.1
                ));
            }
        }
        if let Some(first) = &seen {
            let found = differences(&first.0, &now.0);
            if !found.is_empty() || first.1 != now.1 {
                problems.push(format!(
                    "seed {seed}: open {load} of the SAME file gave a different state:\n  {}",
                    found.join("\n  ")
                ));
            }
        } else {
            seen = Some(now);
        }
    }
    Checked {
        checked: true,
        problems,
    }
}

// ===========================================================================
// PROPERTY 3: the guard
// ===========================================================================

/// What another program does to the open file.
#[derive(Clone, Copy, Debug)]
enum External {
    /// One byte changed, the size kept: only the hash tells.
    SameSizeFlip,
    Truncate,
    /// Another valid project written over it.
    Replace,
    Delete,
    /// The modification time changes, the bytes do not.
    Touch,
    /// The same bytes written again (a sync client's no-op).
    RewriteSame,
}

const EXTERNALS: [External; 6] = [
    External::SameSizeFlip,
    External::Truncate,
    External::Replace,
    External::Delete,
    External::Touch,
    External::RewriteSame,
];

impl External {
    const fn changes_content(self) -> bool {
        !matches!(self, Self::Touch | Self::RewriteSame)
    }

    fn apply(self, path: &Path, other: &[u8]) -> std::io::Result<()> {
        match self {
            Self::SameSizeFlip => {
                let mut bytes = std::fs::read(path)?;
                // Inside the project name: still the same size and still JSON.
                let at = bytes
                    .windows(9)
                    .position(|w| w == b"RustFuzz\"")
                    .unwrap_or(bytes.len() / 2);
                bytes[at] = if bytes[at] == b'R' { b'Q' } else { b'R' };
                std::fs::write(path, bytes)
            }
            Self::Truncate => {
                let bytes = std::fs::read(path)?;
                std::fs::write(path, &bytes[..bytes.len() / 2])
            }
            Self::Replace => std::fs::write(path, other),
            Self::Delete => std::fs::remove_file(path),
            Self::Touch => {
                let file = std::fs::OpenOptions::new().append(true).open(path)?;
                file.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(5))
            }
            Self::RewriteSame => {
                let bytes = std::fs::read(path)?;
                std::fs::write(path, bytes)
            }
        }
    }
}

fn refused_as_modified(result: &Result<(), String>) -> bool {
    result
        .as_ref()
        .is_err_and(|e| e.starts_with(EXTERNALLY_MODIFIED_CODE))
}

/// One external change to the open file, and what the app owes it.
fn check_guard(seed: u32, external: External) -> Vec<String> {
    let label = |what: &str| format!("seed {seed} [{external:?}]: {what}");
    let Some(state) = build_state(seed, false, 10) else {
        return Vec::new();
    };
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let path = temp.path().join("guarded.stalx");
    if !state.save_project(Some(path.clone())).success {
        return vec![label("the survey could not be saved")];
    }
    // Another valid project: the same one renamed.
    let other = match std::fs::read(&path)
        .ok()
        .and_then(|b| stalx_v2::decode(&b, &stalx_v2::DecodeOptions::default()).ok())
    {
        Some(mut decoded) => {
            decoded.project.name = "Another project".to_string();
            stalx_v2::encode(&decoded.project, &decoded.extras, stalx_v2::Mode::Normal)
                .unwrap_or_default()
        }
        None => return vec![label("the saved file does not decode")],
    };
    drop(state);
    let Some(state) = open_fresh(&path) else {
        return vec![label("the saved file would not open")];
    };
    // An edit, so a save has something to write.
    mutate(&state, &mut Rng::new(seed ^ 0x5eed));
    let mut problems = Vec::new();
    if let Err(e) = external.apply(&path, &other) {
        return vec![label(&format!("the external change failed: {e}"))];
    }
    let left = std::fs::read(&path).ok();

    let plain = state.save_project(None);
    let plain: Result<(), String> = if plain.success {
        Ok(())
    } else {
        Err(plain.error.unwrap_or_default())
    };
    if external.changes_content() {
        if !refused_as_modified(&plain) {
            problems.push(label(&format!(
                "the plain save over a file another program changed was not refused as \
                 {EXTERNALLY_MODIFIED_CODE}: {plain:?}"
            )));
        }
        if std::fs::read(&path).ok() != left {
            problems.push(label(
                "the refused save changed the file another program left",
            ));
        }
        let toggle = state
            .save_display_setting("showGrid", "true")
            .map_err(|e| e.to_string());
        if !refused_as_modified(&toggle) {
            problems.push(label(&format!(
                "the local-only save over a changed file was not refused: {toggle:?}"
            )));
        }
        if std::fs::read(&path).ok() != left {
            problems.push(label(
                "the refused local-only save changed the file another program left",
            ));
        }
        let overwrite = state.save_project_overwriting(None);
        if !overwrite.success {
            problems.push(label(&format!(
                "the Overwrite answer failed: {:?}",
                overwrite.error
            )));
        }
    } else if let Err(e) = &plain {
        problems.push(label(&format!(
            "a save was refused although the content did not change: {e}"
        )));
    }
    // From here the file is ours again.
    let next = state.save_project(None);
    if !next.success {
        problems.push(label(&format!(
            "the next plain save failed: {:?}",
            next.error
        )));
    }
    match std::fs::read(&path)
        .ok()
        .and_then(|b| stalx_v2::decode(&b, &stalx_v2::DecodeOptions::default()).ok())
    {
        Some(decoded) if canonical(&decoded.project) == canonical_snapshot(&state) => {}
        _ => problems.push(label("the file does not hold the survey after the save")),
    }
    let toggle = state.save_display_setting("showGrid", "false");
    if let Err(e) = toggle {
        problems.push(label(&format!("the next local-only save failed: {e}")));
    }
    problems
}

// ===========================================================================
// PROPERTY 4: unknown content
// ===========================================================================

/// Insert `fragment` before the last `}` of the top-level member line that
/// starts with `"member":{`.
fn inject_into(text: &str, member: &str, fragment: &str) -> Option<String> {
    let prefix = format!("\"{member}\":{{");
    let mut done = false;
    let lines: Vec<String> = text
        .lines()
        .map(|line| {
            if !done && line.starts_with(&prefix) {
                if let Some(at) = line.rfind('}') {
                    done = true;
                    return format!("{}{fragment}{}", &line[..at], &line[at..]);
                }
            }
            line.to_string()
        })
        .collect();
    done.then(|| lines.join("\n") + "\n")
}

/// Insert a whole top-level member line before `"history"` (or `"end"`).
fn inject_member(text: &str, line: &str) -> Option<String> {
    let at = text
        .find("\n\"history\":")
        .or_else(|| text.find("\n\"end\":true"))?;
    Some(format!("{}\n{line}{}", &text[..at], &text[at..]))
}

/// Give the first station record an extras object with an unknown key.
fn inject_into_first_station(text: &str) -> Option<String> {
    let start = text.find("\"stations\":[\n")? + "\"stations\":[\n".len();
    let end = start + text[start..].find('\n')?;
    let record = &text[start..end];
    let (body, tail) = record
        .strip_suffix(',')
        .map_or((record, ""), |b| (b, ","));
    let body = body.strip_suffix(']')?;
    let changed = if let Some(open) = body.find('{') {
        format!(
            "{}{{\"x_future\":1,{}]{tail}",
            &body[..open],
            &body[open + 1..]
        )
    } else {
        format!("{body},{{\"x_future\":1}}]{tail}")
    };
    Some(format!("{}{changed}{}", &text[..start], &text[end..]))
}

const TOP_LEVEL: &str = "\"x_future_member\":{\"kept\":[1,2.5,\"three\"]},";
const IN_PROJECT: &str = ",\"x_future_project\":{\"k\":true}";
const IN_SETTINGS: &str = ",\"x_future_setting\":[0.125]";
const IN_SUMMARY: &str = ",\"x_future_summary\":\"cached\"";

/// Tiers 2 and 3: kept through an edit, a save and a local-only save; tier 3
/// dropped.
fn check_unknown_members(seed: u32) -> Vec<String> {
    let label = |what: &str| format!("seed {seed} [unknown content]: {what}");
    let Some(state) = build_state(seed, false, 8) else {
        return Vec::new();
    };
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let path = temp.path().join("future.stalx");
    if !state.save_project(Some(path.clone())).success {
        return vec![label("the survey could not be saved")];
    }
    drop(state);
    let text = String::from_utf8(std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
    let injected = inject_member(&text, TOP_LEVEL)
        .and_then(|t| inject_into(&t, "project", IN_PROJECT))
        .and_then(|t| inject_into(&t, "settings", IN_SETTINGS))
        .and_then(|t| inject_into(&t, "summary", IN_SUMMARY))
        .and_then(|t| inject_into(&t, "local", ",\"x_future_local\":7"));
    let Some(injected) = injected else {
        return vec![label("the injection points were not found")];
    };
    if std::fs::write(&path, &injected).is_err() {
        return Vec::new();
    }
    let Some(state) = open_fresh(&path) else {
        return vec![label("a file with tier 2 and 3 unknown content would not open")];
    };
    let mut problems = Vec::new();
    if state.read_only_reason().is_some() {
        problems.push(label("tier 2 and 3 unknown content opened read-only"));
    }
    let kept = |bytes: &[u8], what: &str, problems: &mut Vec<String>| {
        let text = String::from_utf8_lossy(bytes);
        for fragment in [
            TOP_LEVEL.trim_end_matches(','),
            IN_PROJECT.trim_start_matches(','),
            IN_SETTINGS.trim_start_matches(','),
            IN_SUMMARY.trim_start_matches(','),
        ] {
            if !text.contains(fragment) {
                problems.push(label(&format!("{what} dropped the unknown {fragment}")));
            }
        }
        if text.contains("x_future_local") {
            problems.push(label(&format!(
                "{what} kept an unknown key of `local` (tier 3 is dropped)"
            )));
        }
    };
    // A local-only save first: it copies every other member verbatim.
    if let Err(e) = state.save_display_setting("showGrid", "true") {
        problems.push(label(&format!("the local-only save failed: {e}")));
    }
    let bytes = std::fs::read(&path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    for fragment in [TOP_LEVEL.trim_end_matches(','), IN_PROJECT, IN_SETTINGS] {
        if !text.contains(fragment.trim_start_matches(',')) {
            problems.push(label(&format!(
                "the local-only save dropped the unknown {fragment}"
            )));
        }
    }
    // Then an edit and a real save.
    let edited = mutate(&state, &mut Rng::new(seed ^ 0xface));
    let saved = state.save_project(None);
    if !saved.success {
        problems.push(label(&format!("the save failed: {:?}", saved.error)));
    }
    let bytes = std::fs::read(&path).unwrap_or_default();
    kept(&bytes, "an edit and a save", &mut problems);
    match open_fresh(&path) {
        Some(reopened) if canonical_snapshot(&reopened) == canonical_snapshot(&state) => {
            if edited.success && reopened.read_only_reason().is_some() {
                problems.push(label("the re-saved file opened read-only"));
            }
        }
        Some(_) => problems.push(label("the re-saved file does not hold the edited survey")),
        None => problems.push(label("the re-saved file would not open")),
    }
    problems
}

/// Tier 1: an unknown key in a survey record opens the file read-only.
fn check_unknown_record(seed: u32) -> Vec<String> {
    let label = |what: &str| format!("seed {seed} [unknown record key]: {what}");
    let Some(state) = build_state(seed, false, 4) else {
        return Vec::new();
    };
    let Ok(temp) = TempDir::new() else {
        return Vec::new();
    };
    let path = temp.path().join("newer.stalx");
    if !state.save_project(Some(path.clone())).success {
        return vec![label("the survey could not be saved")];
    }
    drop(state);
    let text = String::from_utf8(std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
    let Some(injected) = inject_into_first_station(&text) else {
        return vec![label("the first station record was not found")];
    };
    if std::fs::write(&path, &injected).is_err() {
        return Vec::new();
    }
    let Some(state) = open_fresh(&path) else {
        return vec![label("the file would not open (tier 1 opens read-only)")];
    };
    let mut problems = Vec::new();
    if state.read_only_reason().is_none() {
        problems.push(label("the file did not open read-only"));
    }
    let before = canonical_snapshot(&state);
    let edit = mutate(&state, &mut Rng::new(seed));
    if edit.success || !edit.error.as_deref().unwrap_or("").starts_with(READ_ONLY_FILE_CODE) {
        problems.push(label(&format!(
            "an edit was not refused as {READ_ONLY_FILE_CODE}: {:?}",
            edit.error
        )));
    }
    if canonical_snapshot(&state) != before {
        problems.push(label("the refused edit changed the survey"));
    }
    let undo = state.undo();
    if undo.success {
        problems.push(label("undo ran on a read-only file"));
    }
    let save = state.save_project(None);
    if save.success {
        problems.push(label("the read-only file was saved"));
    }
    if let Err(e) = state.save_display_setting("showGrid", "true") {
        problems.push(label(&format!(
            "the local-only save failed on a read-only file: {e}"
        )));
    }
    // The body is copied verbatim: only `local` may differ.
    let now = String::from_utf8(std::fs::read(&path).unwrap_or_default()).unwrap_or_default();
    let body = |t: &str| -> Vec<String> {
        t.lines()
            .filter(|l| !l.starts_with("\"local\":"))
            .map(ToString::to_string)
            .collect()
    };
    if body(&now) != body(&injected) {
        problems.push(label(&format!(
            "the local-only save rewrote the body of a read-only file: {}",
            first_difference(&body(&injected).join("\n"), &body(&now).join("\n"))
        )));
    }
    problems
}

// ===========================================================================
// PROPERTY 5: old file shapes
// ===========================================================================

/// One surgical degradation of a legacy file, and what the open owes it.
struct Degradation {
    id: &'static str,
    /// What the frozen reader promises about a file in this shape.
    promise: &'static str,
    surgery: fn(&Connection) -> Result<(), rusqlite::Error>,
    must_open: bool,
}

const DEGRADATIONS: &[Degradation] = &[
    Degradation {
        id: "lines-without-the-gauge-columns",
        promise: "the reader selects NULL for absent gauge columns, so a pre-gauge file opens",
        surgery: |c| {
            c.execute_batch(
                "ALTER TABLE sections DROP COLUMN gauge_shot_uuid;
                 ALTER TABLE sections DROP COLUMN gauge_neighbour_uuid;",
            )
        },
        must_open: true,
    },
    // A CURRENT schema wearing an ancient version marker: no build wrote
    // it, the migrations run against tables that already have their later
    // shape, and refusing cleanly is the right answer.
    Degradation {
        id: "an-ancient-user-version",
        promise: "an impossible version/schema pairing is refused cleanly, never half-opened",
        surgery: |c| c.execute_batch("PRAGMA user_version = 1;"),
        must_open: false,
    },
    Degradation {
        id: "disagreeing-version-markers",
        promise: "the project-table version and PRAGMA user_version are reconciled on open",
        surgery: |c| c.execute_batch("UPDATE project SET schema_version = 2;"),
        must_open: true,
    },
    Degradation {
        id: "no-undo-tables-at-all",
        promise: "a file written before the undo tables is a project with no history",
        surgery: |c| {
            c.execute_batch(
                "DROP TABLE IF EXISTS undo_stack;
                 DROP TABLE IF EXISTS redo_stack;",
            )
        },
        must_open: true,
    },
    Degradation {
        id: "v18-file-without-station-names",
        promise: "an absent station_names table reads as no names",
        surgery: |c| c.execute_batch("DROP TABLE station_names;"),
        must_open: true,
    },
    Degradation {
        id: "v18-file-without-station-ids",
        promise: "an absent station_ids table and key re-mint the ids in line order",
        surgery: |c| {
            c.execute_batch(
                "DROP TABLE station_ids;
                 DELETE FROM project_settings WHERE key = 'next_station_id';",
            )
        },
        must_open: true,
    },
];

/// `None` when the degradation could not be built (a skip, not a pass).
fn check_degradation(seed: u32, degradation: &Degradation) -> Option<Vec<String>> {
    let label = |what: &str| format!("seed {seed} [{}]: {what}", degradation.id);
    crate::app_dirs::init_for_test();
    let temp = TempDir::new().ok()?;
    let path = temp.path().join(format!("degraded-{seed}-{}.stalx", degradation.id));
    write_legacy(&path, &mut Rng::new(seed), "Degraded");
    // A surgery SQLite refuses is a skip: nothing was built.
    let connection = Connection::open(&path).ok()?;
    (degradation.surgery)(&connection).ok()?;
    drop(connection);
    let original = std::fs::read(&path).unwrap_or_default();
    let mut problems = Vec::new();
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| open_fresh(&path))) {
        Err(payload) => problems.push(label(&format!(
            "opening the degraded file PANICKED ({})\n  promise: {}",
            panic_text(payload.as_ref()),
            degradation.promise
        ))),
        Ok(None) => {
            if degradation.must_open {
                problems.push(label(&format!(
                    "the degraded file did not open\n  promise: {}",
                    degradation.promise
                )));
            }
            if std::fs::read(&path).unwrap_or_default() != original {
                problems.push(label("the refused open changed the file"));
            }
        }
        Ok(Some(state)) => {
            // Opening a shape the reader may refuse is fine, as long as it is
            // a whole open: a valid survey that upgrades and reopens.
            let violations = state.integrity_violation_messages();
            if !violations.is_empty() {
                problems.push(label(&format!(
                    "the degraded file opened as an invalid survey: {violations:?}"
                )));
            }
            if std::fs::read(&path).unwrap_or_default() != original {
                problems.push(label("the open changed the legacy file"));
            }
            let migrated = canonical_snapshot(&state);
            // The first save upgrades the file; the v2 file reopens as the
            // same survey.
            let saved = state.save_project(None);
            if !saved.success {
                problems.push(label(&format!(
                    "the degraded file opened but could not be saved (upgraded): {:?}",
                    saved.error
                )));
            } else if stalx_v2::sniff(&std::fs::read(&path).unwrap_or_default()) != Format::V2 {
                problems.push(label("the upgrading save did not write a v2 file"));
            } else {
                match open_fresh(&path) {
                    Some(reopened) if canonical_snapshot(&reopened) == migrated => {}
                    Some(reopened) => problems.push(label(&format!(
                        "the upgraded file reopens as another survey: {}",
                        first_difference(&migrated, &canonical_snapshot(&reopened))
                    ))),
                    None => problems.push(label("the upgraded file would not open")),
                }
            }
        }
    }
    Some(problems)
}

// ===========================================================================
// The gate
// ===========================================================================

/// PINNED. `format_property_sweep` re-derives the set.
const GATE_SEEDS: &[u32] = &[1, 2, 3, 4, 5, 6];
const GATE_MOVES: usize = 16;
const GATE_LOADS: usize = 4;

/// THE FILE-FORMAT GATE. Not `#[ignore]`d.
#[test]
fn format_properties_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    let mut checked: BTreeMap<&str, usize> = BTreeMap::new();
    for &seed in GATE_SEEDS {
        let closures = seed % 2 == 0;
        let fixpoint = check_save_open_save(seed, closures, GATE_MOVES);
        *checked.entry("fixpoint").or_default() += usize::from(fixpoint.checked);
        problems.extend(fixpoint.problems);
        let function = check_open_is_a_function(seed, closures, GATE_MOVES, GATE_LOADS);
        *checked.entry("open").or_default() += usize::from(function.checked);
        problems.extend(function.problems);
    }
    for external in EXTERNALS {
        problems.extend(check_guard(GATE_SEEDS[0], external));
        *checked.entry("guard").or_default() += 1;
    }
    for &seed in &GATE_SEEDS[..2] {
        problems.extend(check_unknown_members(seed));
        problems.extend(check_unknown_record(seed));
        *checked.entry("unknown").or_default() += 2;
    }
    for degradation in DEGRADATIONS {
        match check_degradation(GATE_SEEDS[0], degradation) {
            Some(found) => {
                problems.extend(found);
                *checked.entry("degradation").or_default() += 1;
            }
            None => problems.push(format!("[{}]: the degradation was not built", degradation.id)),
        }
    }
    println!("format properties: {checked:?}");
    assert!(
        checked.get("fixpoint").copied().unwrap_or(0) >= GATE_SEEDS.len()
            && checked.get("open").copied().unwrap_or(0) >= GATE_SEEDS.len(),
        "the format gate checked only {checked:?}; the builder is not producing saveable projects"
    );
    assert!(
        problems.is_empty(),
        "{} file-format property failure(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// Determinism, so a failing seed is a repro.
#[test]
fn the_format_properties_are_deterministic() {
    let _config = isolated_config();
    for &seed in &GATE_SEEDS[..3] {
        let first = check_save_open_save(seed, false, GATE_MOVES);
        let second = check_save_open_save(seed, false, GATE_MOVES);
        assert_eq!(
            (first.checked, first.problems.len()),
            (second.checked, second.problems.len()),
            "seed {seed} did not replay identically"
        );
    }
}

/// The SWEEP, and the tool for re-choosing `GATE_SEEDS`.
///
/// ```text
/// STALAGMIGHT_RUST_FORMAT_SEEDS=60 STALAGMIGHT_RUST_FORMAT_CLOSURES=1 \
///   TELEMETRY_ENDPOINT=http://localhost/dummy cargo test \
///   --manifest-path src-tauri/Cargo.toml -p stalagmight-core \
///   --lib format_property_sweep -- --ignored --nocapture
/// ```
#[test]
#[ignore = "sweep: prints per-seed status, used to choose the format-property GATE_SEEDS"]
fn format_property_sweep() {
    let _config = isolated_config();
    let seeds: u32 = std::env::var("STALAGMIGHT_RUST_FORMAT_SEEDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);
    let moves: usize = std::env::var("STALAGMIGHT_RUST_FORMAT_MOVES")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_MOVES);
    let loads: usize = std::env::var("STALAGMIGHT_RUST_FORMAT_LOADS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(GATE_LOADS);
    let closures = std::env::var("STALAGMIGHT_RUST_FORMAT_CLOSURES").is_ok();
    let mut dirty = Vec::new();
    let mut skipped = 0;
    for seed in 1..=seeds {
        let mut found = Vec::new();
        let fixpoint = check_save_open_save(seed, closures, moves);
        if !fixpoint.checked {
            skipped += 1;
        }
        found.extend(fixpoint.problems);
        found.extend(check_open_is_a_function(seed, closures, moves, loads).problems);
        if seed <= 8 {
            for external in EXTERNALS {
                found.extend(check_guard(seed, external));
            }
            found.extend(check_unknown_members(seed));
            found.extend(check_unknown_record(seed));
        }
        if seed <= 4 {
            for degradation in DEGRADATIONS {
                found.extend(check_degradation(seed, degradation).unwrap_or_default());
            }
        }
        if !found.is_empty() {
            dirty.push(seed);
            for problem in &found {
                println!("{problem}");
            }
        }
    }
    println!(
        "format sweep: {seeds} seed(s), closures={closures}, {skipped} skipped; seeds with a \
         failure ({}): {dirty:?}",
        dirty.len()
    );
}
