//! [#576 FZ-PERSIST] FAULT-INJECTION fuzzer for the atomic save path: a save that
//! fails part-way, or a process killed part-way, never costs the user the file.
//!
//! # What it adds to the crash-timing engine
//!
//! [`super::crash_property_tests`] builds the files a KILL leaves at each pipeline
//! boundary, over projects without side tables. This engine makes the save itself
//! FAIL - the way a full disk, an I/O error or a vanished file fails it - at a
//! random point, in every save mode, over a session that owns every side table
//! ([`super::persist_roundtrip_fuzz_tests::Session`]), and then keeps using the
//! session. Kills are here too, with the temp file torn at a random byte.
//!
//! # The faults
//!
//! | fault | how | the step it hits |
//! |---|---|---|
//! | `SideTable(t)` | `save_sidecars::fault::fail_writes_to(t)` | the carry of one side table into the temp |
//! | `DiskFull` | the temp database capped at a random number of pages past its size when created (`PRAGMA max_page_count`, via `AFTER_TEMP_OPENED`) | wherever the temp stops growing: the survey transaction, a side table, the history |
//! | `TempVanishes` | the finished temp deleted before the rename (`AFTER_TEMP_WRITTEN`) | the rename (`ENOENT`) |
//! | `TempNameTaken` | a non-empty folder at `<name>.stalx.tmp` | creating the temp |
//! | `ReopenFails` | `FAIL_REOPEN_AFTER_RENAME` | the reopen after the rename |
//! | `Kill(..)` | the files a kill leaves, built from a real save's bytes | before the temp, temp torn at a random byte, temp whole, after the rename |
//!
//! # The oracle
//!
//! - No panic, ever.
//! - A FAILED save leaves the destination byte for byte as it was (or absent if
//!   it was), leaves no `.stalx.tmp` behind, and leaves a session that saves
//!   again: the retry succeeds and its file passes the full round-trip check
//!   (survey, history window, every side table, row-by-row fixpoint).
//! - A save that SUCCEEDS under a fault does so only where the side-table policy
//!   says it may (docs/saving.md, "Side tables and save modes"), says so with the
//!   policy's warning code, and wrote exactly what that warning says: display
//!   settings absent (never a partial set), or an EMPTY history (never a partial
//!   one). Everything else in the file passes the full check.
//! - After a kill, the destination opens, and as exactly the old file before the
//!   rename and exactly the new one after it; the next save from the reopened file
//!   succeeds, leaves no temp, and passes the full check.
//!
//! # Running it
//!
//! `save_fault_fuzz_gate` runs a fixed seed list in CI. The long run:
//!
//! ```text
//! STALAGMIGHT_RUST_PERSIST_SEEDS=500 STALAGMIGHT_RUST_PERSIST_SEED_START=1 \
//!   cargo test --manifest-path src-tauri/Cargo.toml -p stalagmight-core --lib \
//!   save_fault_fuzz_sweep -- --ignored --nocapture
//! ```
//!
//! Not reached: a failing `fsync` (SQLite issues it inside its commit; there is no
//! VFS seam), and a power cut that reorders writes on the device.

#![expect(
    clippy::expect_used,
    reason = "test module: see docs/linting.md for the per-test-module expect idiom"
)]
#![expect(
    clippy::print_stdout,
    reason = "a fuzz sweep's per-seed failure lines and summary ARE its output"
)]

use std::collections::{BTreeMap, BTreeSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::time::Instant;

use super::format_property_tests::{dump_database, open_fresh};
use super::fuzz_undo_tests::{random_shot, Rng};
use super::persist_roundtrip_fuzz_tests::{
    adopt_live_bookkeeping, check_file, edit, knob, observe_state, side_tables_of, side_write,
    trace, trace_take, Session, SideModel,
};
use super::project_io::{AFTER_TEMP_OPENED, AFTER_TEMP_WRITTEN, FAIL_REOPEN_AFTER_RENAME};
use super::save_sidecars::fault::fail_writes_to;
use crate::core::{Command, CommandResult};
use crate::test_helpers::isolated_config;

/// The side tables `fail_writes_to` can break, as the registry names them.
const SIDE_TABLES: &[&str] = &[
    "display_settings",
    "resolved_warnings",
    "speleo_link",
    "speleo_undo_chain",
    "speleo_undo_watermark",
    "undo_stack",
];

const DISPLAY_NOT_SAVED: &str = "DISPLAY_SETTINGS_NOT_SAVED";
const HISTORY_DROPPED: &str = "UNDO_HISTORY_DROPPED";
const NOT_REOPENED: &str = "DATABASE_NOT_REOPENED";

#[derive(Clone, Copy, Debug)]
enum Fault {
    SideTable(&'static str),
    /// The temp may grow by `extra` pages past its freshly created schema.
    DiskFull {
        extra: u32,
    },
    TempVanishes,
    TempNameTaken,
    ReopenFails,
    Kill(Kill),
}

#[derive(Clone, Copy, Debug)]
enum Kill {
    BeforeTemp,
    /// The temp holds its first `per_mille`/1000 of the finished bytes.
    TempTorn {
        per_mille: u32,
    },
    TempWhole,
    AfterRename,
}

/// Which save the fault hits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    /// The first save of a new project.
    First,
    InPlace,
    /// Save As to a new file: the session moves (a linked project: a detached
    /// copy, and it stays).
    SaveAs,
}

fn pick_fault(rng: &mut Rng, old_pages: u32) -> Fault {
    match rng.below(12) {
        0..=2 => Fault::SideTable(rng.pick(SIDE_TABLES)),
        // Mostly a few pages (a save of these projects grows the fresh schema
        // by a handful), sometimes up to the old file's whole size.
        3..=5 => Fault::DiskFull {
            extra: if rng.chance(75) {
                rng.range(0, 8) as u32
            } else {
                rng.range(0, i64::from(old_pages)) as u32
            },
        },
        6 => Fault::TempVanishes,
        7 => Fault::TempNameTaken,
        8 => Fault::ReopenFails,
        9 => Fault::Kill(Kill::TempTorn {
            per_mille: rng.range(0, 999) as u32,
        }),
        10 => Fault::Kill(*rng.pick(&[Kill::BeforeTemp, Kill::TempWhole])),
        _ => Fault::Kill(Kill::AfterRename),
    }
}

/// The save the user makes to `destination` from where the session is NOW: a
/// plain Save once the session is on it (an explicit Save As to the open file of
/// a linked project is refused by design, "Save a Copy needs a different
/// destination"), else Save As when the run asked for one - a detached copy if
/// the project is linked. `(request, detached)`.
fn plan_save(session: &Session, destination: &Path, explicit: bool) -> (Option<PathBuf>, bool) {
    if session.file.as_deref() == Some(destination) || !explicit {
        (None, false)
    } else {
        (
            Some(destination.to_path_buf()),
            session.model.link.is_some(),
        )
    }
}

/// The file's logical content without its display settings (a dump, see
/// `dump_database`).
fn dump_without_display_settings(bytes: &[u8]) -> Result<String, String> {
    let scratch = tempfile::TempDir::new().map_err(|e| e.to_string())?;
    let path = scratch.path().join("Dumped.stalx");
    std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
    let dump = dump_database(&path)?;
    let mut out = String::new();
    let mut skipping = false;
    for line in dump.lines() {
        if line.starts_with("== ") {
            skipping = line.starts_with("== display_settings ");
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    Ok(out)
}

/// A fault that is not a kill (a chain goes on after it).
fn pick_injected(rng: &mut Rng, old_pages: u32) -> Fault {
    loop {
        let fault = pick_fault(rng, old_pages);
        if !matches!(fault, Fault::Kill(_)) {
            return fault;
        }
    }
}

/// The names in `dir`, sorted.
fn names_in(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

fn temp_of(path: &Path) -> PathBuf {
    path.with_extension("stalx.tmp")
}

/// Clear every seam, whether or not the save reached it.
fn disarm() {
    AFTER_TEMP_OPENED.with(|hook| hook.borrow_mut().take());
    AFTER_TEMP_WRITTEN.with(|hook| hook.borrow_mut().take());
    FAIL_REOPEN_AFTER_RENAME.with(|flag| flag.set(false));
}

/// Run one seed.
fn run_seed(seed: u32) -> (Vec<String>, BTreeMap<String, usize>) {
    let mut rng = Rng::new(seed);
    let mut session = Session::new(&mut rng);
    let mut problems = Vec::new();
    let mut counts = BTreeMap::new();
    let mode = *rng.pick(&[Mode::First, Mode::InPlace, Mode::InPlace, Mode::SaveAs]);
    for _ in 0..rng.below(3) {
        side_write(&mut session, &mut rng);
    }
    if mode != Mode::First {
        let first = session.next_path();
        let result = session.state.save_project(Some(first.clone()));
        if !result.success {
            // A built project with a dangling reference: nothing to fault.
            return (problems, counts);
        }
        session.model.prune_dead_ignores(&session.state);
        session.file = Some(first);
        for _ in 0..rng.range(2, 12) {
            if rng.chance(55) {
                edit(&session.state, &mut rng);
            } else {
                side_write(&mut session, &mut rng);
            }
        }
    }
    // Sometimes a long section, so the survey itself outgrows the fresh file's
    // pages and a full disk can land inside the survey's own transaction.
    if rng.chance(40) {
        let shots = (0..rng.range(40, 260))
            .map(|_| random_shot(&mut rng))
            .collect();
        session.state.execute(Command::CreateSection {
            name: Some("Long Passage".to_owned()),
            surveyor: None,
            date: None,
            color: "#4ecdc4".to_owned(),
            direction: None,
            shots,
        });
        *counts.entry("long-section".to_owned()).or_insert(0) += 1;
    }
    if session
        .state
        .get_project_state()
        .is_some_and(|p| p.has_unresolvable_cross_references())
    {
        return (problems, counts);
    }
    if let Some(problem) = adopt_live_bookkeeping(&mut session) {
        problems.push(format!("seed {seed}: before the faulted save, {problem}"));
    }
    let destination = match mode {
        Mode::InPlace => session.file.clone().expect("saved above"),
        Mode::First | Mode::SaveAs => session.next_path(),
    };
    let request = (mode != Mode::InPlace).then(|| destination.clone());
    let old_bytes = std::fs::read(&destination).ok();
    let old_pages = old_bytes.as_ref().map_or(40, |b| (b.len() / 4096) as u32);
    let fault = pick_fault(&mut rng, old_pages);
    let detached = mode == Mode::SaveAs && session.model.link.is_some();
    *counts
        .entry(format!(
            "{mode:?}{}",
            if detached { "-detached" } else { "" }
        ))
        .or_insert(0) += 1;
    let label = format!(
        "{fault:?} on a {mode:?} save{}",
        if detached { " (detached copy)" } else { "" }
    );
    let fault_name = format!("{fault:?}");
    let fault_kind = fault_name
        .split([' ', '(', '{'])
        .next()
        .unwrap_or_default()
        .to_owned();

    let outcome = if let Fault::Kill(kill) = fault {
        check_kill(
            &mut session,
            request.as_deref(),
            &destination,
            old_bytes.as_deref(),
            kill,
        )
    } else {
        // Sometimes a chain: two or three faulted saves before the clean one,
        // so a session that already failed once is the one failing again.
        let mut faults = vec![fault];
        if rng.chance(30) {
            for _ in 0..rng.range(1, 2) {
                faults.push(pick_injected(&mut rng, old_pages));
            }
            *counts.entry("chained".to_owned()).or_insert(0) += 1;
        }
        check_failure(
            &mut session,
            &mut rng,
            &destination,
            mode != Mode::InPlace,
            &faults,
        )
    };
    match outcome {
        Ok(what) => {
            *counts.entry(format!("{fault_kind}:{what}")).or_insert(0) += 1;
        }
        Err(found) => {
            *counts.entry(format!("{fault_kind}:PROBLEM")).or_insert(0) += 1;
            problems.extend(
                found
                    .into_iter()
                    .map(|p| format!("seed {seed}: {label}: {p}")),
            );
        }
    }
    (problems, counts)
}

/// A chain of saves, each under one of `faults`, the session going on between
/// them, then the retry with nothing failing. `Ok` names what the FIRST faulted
/// save did. Problems found after a later fault are prefixed with its kind.
fn check_failure(
    session: &mut Session,
    rng: &mut Rng,
    destination: &Path,
    explicit: bool,
    faults: &[Fault],
) -> Result<&'static str, Vec<String>> {
    let mut problems = Vec::new();
    let mut first_outcome = "refused";
    for (index, &fault) in faults.iter().enumerate() {
        let (request, detached) = plan_save(session, destination, explicit);
        trace(|| format!("faulted save {fault:?} request {request:?} detached {detached}"));
        let (outcome, found) =
            save_under_fault(session, request.as_deref(), destination, fault, detached)?;
        if index == 0 {
            first_outcome = outcome;
        }
        let prefix = if index == 0 {
            String::new()
        } else {
            let kind = format!("{fault:?}");
            let kind = kind
                .split([' ', '(', '{'])
                .next()
                .unwrap_or_default()
                .to_owned();
            format!("then-{kind}: ")
        };
        problems.extend(found.into_iter().map(|p| format!("{prefix}{p}")));
        // The session goes on - a few more edits and side-table writes (after a
        // failed reopen they land in the in-memory cache, or are refused).
        for _ in 0..rng.below(4) {
            if rng.chance(50) {
                edit(&session.state, rng);
            } else {
                side_write(session, rng);
            }
        }
        if let Some(problem) = adopt_live_bookkeeping(session) {
            problems.push(format!("{prefix}after the save, {problem}"));
        }
    }
    if session
        .state
        .get_project_state()
        .is_some_and(|p| p.has_unresolvable_cross_references())
    {
        // An edit since left a dangling reference: the retry is refused by design.
        return if problems.is_empty() {
            Ok(first_outcome)
        } else {
            Err(problems)
        };
    }
    // The same save again, with nothing failing.
    let (request, detached) = plan_save(session, destination, explicit);
    let retry = session.state.save_project(request);
    if retry.success {
        let unexpected: Vec<&str> = retry
            .warnings
            .iter()
            .map(|w| w.code.as_str())
            .filter(|code| *code != NOT_REOPENED)
            .collect();
        if !unexpected.is_empty() {
            problems.push(format!("the retry warned {unexpected:?}"));
        }
        let model = session.model_after_save(detached);
        let expected = observe_state(&session.state);
        for problem in check_file(destination, &expected, &model) {
            problems.push(format!("after the retry: {problem}"));
        }
        if temp_of(destination).exists() {
            problems.push("the retry left a .stalx.tmp behind".to_owned());
        }
    } else {
        problems.push(format!(
            "the retry with nothing failing was refused: {:?} {:?}",
            retry.code, retry.error
        ));
    }
    if problems.is_empty() {
        Ok(first_outcome)
    } else {
        Err(problems)
    }
}

/// One save under `fault`, checked: what it did, and every problem with it.
fn save_under_fault(
    session: &mut Session,
    request: Option<&Path>,
    destination: &Path,
    fault: Fault,
    detached: bool,
) -> Result<(&'static str, Vec<String>), Vec<String>> {
    let mut problems = Vec::new();
    let old_bytes = std::fs::read(destination).ok();
    // A session that lost its handle (a failed reopen) reopens the file first and
    // writes the display settings changed meanwhile into it
    // (`restore_missing_database`) - a write to the open file a failed save does
    // not undo, and is not meant to.
    let handle_lost = request.is_none() && !session.has_database();
    let side_fault = match fault {
        Fault::SideTable(table) => Some(fail_writes_to(table)),
        _ => None,
    };
    let folder = temp_of(destination);
    match fault {
        Fault::DiskFull { extra } => AFTER_TEMP_OPENED.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move |db| {
                // A cap below the file's size is raised to it: asking for one
                // page reads the size.
                let size = db.cap_page_count_for_test(1).expect("read the temp size");
                db.cap_page_count_for_test(size + extra)
                    .expect("cap the temp database");
            }));
        }),
        Fault::TempVanishes => {
            let temp = folder.clone();
            AFTER_TEMP_WRITTEN.with(|hook| {
                *hook.borrow_mut() = Some(Box::new(move || {
                    std::fs::remove_file(&temp).expect("remove the finished temp");
                }));
            });
        }
        Fault::TempNameTaken => {
            std::fs::create_dir_all(folder.join("inside"))
                .map_err(|e| vec![format!("setup: a folder at the temp name: {e}")])?;
        }
        Fault::ReopenFails => FAIL_REOPEN_AFTER_RENAME.with(|flag| flag.set(true)),
        Fault::SideTable(_) | Fault::Kill(_) => {}
    }
    let folder_of_destination = destination.parent().expect("a file in a folder");
    let names_before = names_in(folder_of_destination);
    let saved = catch_unwind(AssertUnwindSafe(|| {
        session.state.save_project(request.map(Path::to_path_buf))
    }));
    disarm();
    drop(side_fault);
    if matches!(fault, Fault::TempNameTaken) {
        let _ = std::fs::remove_dir_all(&folder);
    }
    let Ok(result) = saved else {
        return Err(vec!["the save PANICKED".to_owned()]);
    };
    let what = match (result.success, result.warnings.is_empty()) {
        (false, _) => "refused",
        (true, true) => "saved",
        (true, false) => "saved-warned",
    };
    let destination_name = destination
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let stray: Vec<String> = names_in(folder_of_destination)
        .difference(&names_before)
        .filter(|name| **name != destination_name)
        .cloned()
        .collect();
    if !stray.is_empty() {
        problems.push(format!("the save left files behind: {stray:?}"));
    }
    if result.success {
        let model = session.model_after_save(detached);
        problems.extend(check_saved_under_fault(
            session,
            model,
            destination,
            &result,
            fault,
            detached,
        ));
        // "They will be back to their defaults when the project is next opened"
        // (`Sidecar::lost_warning`): the session's file has none from here on.
        // A detached copy leaves the session's own file, and its settings, alone.
        if !detached && result.warnings.iter().any(|w| w.code == DISPLAY_NOT_SAVED) {
            session.model.display.clear();
        }
        // A save that is not a detached copy moves the session onto its file.
        if !detached {
            session.file = Some(destination.to_path_buf());
        }
    } else {
        let now = std::fs::read(destination).ok();
        let only_settings_flushed = || match (&old_bytes, &now) {
            (Some(old), Some(now)) => {
                handle_lost
                    && matches!(
                        (dump_without_display_settings(old), dump_without_display_settings(now)),
                        (Ok(a), Ok(b)) if a == b
                    )
            }
            _ => false,
        };
        if now != old_bytes && !only_settings_flushed() {
            problems.push(format!(
                "the save failed ({:?}) but the destination changed: {} -> {} bytes",
                result.error,
                old_bytes.as_ref().map_or(0, Vec::len),
                now.as_ref().map_or(0, Vec::len)
            ));
        }
        if temp_of(destination).exists() {
            problems.push("a failed save left its .stalx.tmp behind".to_owned());
        }
    }
    Ok((what, problems))
}

/// A save that went through under `fault`: allowed only where the policy says,
/// with its warning, having written exactly what the warning says.
fn check_saved_under_fault(
    session: &Session,
    mut model: SideModel,
    destination: &Path,
    result: &CommandResult,
    fault: Fault,
    detached: bool,
) -> Vec<String> {
    let mut problems = Vec::new();
    let codes: BTreeSet<&str> = result.warnings.iter().map(|w| w.code.as_str()).collect();
    let allowed: &[&str] = match fault {
        Fault::SideTable("display_settings") => &[DISPLAY_NOT_SAVED],
        Fault::SideTable("undo_stack") => &[HISTORY_DROPPED],
        // A link and its markers are not carried into a detached copy, so their
        // write never runs and cannot fail it.
        Fault::SideTable("speleo_link" | "speleo_undo_chain" | "speleo_undo_watermark")
            if detached =>
        {
            &[]
        }
        Fault::DiskFull { .. } => &[DISPLAY_NOT_SAVED, HISTORY_DROPPED],
        // A detached copy is not reopened: the session stays on its own file.
        Fault::ReopenFails if detached => &[],
        Fault::ReopenFails => &[NOT_REOPENED],
        Fault::SideTable(table) => {
            return vec![format!(
                "a save whose {table} write failed went through (the policy refuses it), \
                 warnings {codes:?}"
            )];
        }
        Fault::TempVanishes | Fault::TempNameTaken | Fault::Kill(_) => {
            return vec![format!(
                "a save whose {fault:?} should have failed it went through, warnings {codes:?}"
            )];
        }
    };
    if let Some(stray) = codes.iter().find(|code| !allowed.contains(code)) {
        problems.push(format!(
            "the save warned {stray}, which this fault cannot cause"
        ));
    }
    if matches!(fault, Fault::SideTable(_) | Fault::ReopenFails) {
        if let Some(code) = allowed.iter().find(|code| !codes.contains(*code)) {
            problems.push(format!("the save went through without its {code} warning"));
        }
    }
    let mut expected = observe_state(&session.state);
    if codes.contains(HISTORY_DROPPED) {
        expected.undo.clear();
        expected.redo.clear();
    }
    if codes.contains(DISPLAY_NOT_SAVED) {
        model.display.clear();
    }
    for problem in check_file(destination, &expected, &model) {
        problems.push(format!("the file the faulted save wrote: {problem}"));
    }
    problems
}

/// The files a kill at `kill` leaves, opened; then the save after the restart.
fn check_kill(
    session: &mut Session,
    request: Option<&Path>,
    destination: &Path,
    old_bytes: Option<&[u8]>,
    kill: Kill,
) -> Result<&'static str, Vec<String>> {
    // The real save, with the finished temp's bytes captured on their way to the
    // rename: those are exactly the bytes a kill after this point leaves.
    let captured = std::rc::Rc::new(std::cell::RefCell::new(None));
    {
        let captured = std::rc::Rc::clone(&captured);
        let temp = temp_of(destination);
        AFTER_TEMP_WRITTEN.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                *captured.borrow_mut() = std::fs::read(&temp).ok();
            }));
        });
    }
    let result = session.state.save_project(request.map(Path::to_path_buf));
    disarm();
    if !result.success {
        return Err(vec![format!(
            "the unfaulted save failed: {:?}",
            result.error
        )]);
    }
    let Some(new_bytes) = captured.borrow_mut().take() else {
        return Err(vec!["the finished temp could not be read".to_owned()]);
    };
    let detached = request.is_some()
        && session.model.link.is_some()
        && session.file.as_deref() != Some(destination);
    let model = session.model_after_save(detached);
    let expected_new = observe_state(&session.state);
    if !detached {
        session.file = Some(destination.to_path_buf());
    }

    // The machine after the kill, in a folder of its own.
    let setup = |what: &str, e: std::io::Error| vec![format!("setup: {what}: {e}")];
    let scratch = tempfile::TempDir::new().map_err(|e| setup("scratch dir", e))?;
    let dest = scratch.path().join("Killed.stalx");
    let temp = temp_of(&dest);
    let new_side = match kill {
        Kill::AfterRename => true,
        Kill::BeforeTemp | Kill::TempTorn { .. } | Kill::TempWhole => false,
    };
    let dest_bytes = if new_side {
        Some(new_bytes.as_slice())
    } else {
        old_bytes
    };
    if let Some(bytes) = dest_bytes {
        std::fs::write(&dest, bytes).map_err(|e| setup("write the destination", e))?;
    }
    match kill {
        Kill::TempTorn { per_mille } => {
            let keep = new_bytes.len() * per_mille as usize / 1000;
            std::fs::write(&temp, &new_bytes[..keep])
                .map_err(|e| setup("write the torn temp", e))?;
        }
        Kill::TempWhole => {
            std::fs::write(&temp, &new_bytes).map_err(|e| setup("write the whole temp", e))?;
        }
        Kill::BeforeTemp | Kill::AfterRename => {}
    }
    let mut problems = Vec::new();
    // A leftover temp is a restore candidate in the recovery UI: opening it must
    // not take the process down, whatever it holds.
    if temp.exists() {
        let probe = scratch.path().join("Candidate.stalx");
        std::fs::copy(&temp, &probe).map_err(|e| setup("copy the leftover temp", e))?;
        if catch_unwind(AssertUnwindSafe(|| open_fresh(&probe).is_some())).is_err() {
            problems.push("opening the leftover temp PANICKED".to_owned());
        }
    }
    if dest_bytes.is_none() {
        // A first save killed before its rename leaves no project at all: nothing
        // to open, and the leftover temp is all the check above can look at.
        return if problems.is_empty() {
            Ok("no-file-yet")
        } else {
            Err(problems)
        };
    }
    let reopened = catch_unwind(AssertUnwindSafe(|| open_fresh(&dest)));
    let Ok(reopened) = reopened else {
        problems.push("opening the destination after the kill PANICKED".to_owned());
        return Err(problems);
    };
    let Some(reopened) = reopened else {
        problems.push("the destination does not open after the kill".to_owned());
        return Err(problems);
    };
    // Before the rename the destination IS the old file, byte for byte, so it can
    // only be checked as itself; after it, it must be the new save exactly.
    if new_side {
        for problem in check_file(&dest, &expected_new, &model) {
            problems.push(format!("the file after a kill past the rename: {problem}"));
        }
    }
    // The restarted session saves; the leftover temp must not get in its way, and
    // the file it writes is the file it opened.
    let before = match side_tables_of(&reopened) {
        // Dead ignore rows are allowed in a file a kill interrupted (see
        // `side_tables_of`); the save below must drop them.
        Ok((mut sides, _dead)) => {
            sides.prune_dead_ignores(&reopened);
            sides
        }
        Err(e) => {
            problems.push(format!("after the kill: {e}"));
            return Err(problems);
        }
    };
    let resaved = reopened.save_project(None);
    if resaved.success {
        if temp.exists() {
            problems.push("the save after the kill left a .stalx.tmp behind".to_owned());
        }
        let expected = observe_state(&reopened);
        for problem in check_file(&dest, &expected, &before) {
            problems.push(format!("the save after the kill: {problem}"));
        }
    } else if before.link.is_none() {
        problems.push(format!(
            "the restarted session cannot save: {:?} {:?}",
            resaved.code, resaved.error
        ));
    }
    if problems.is_empty() {
        Ok(if new_side { "new-file" } else { "old-file" })
    } else {
        Err(problems)
    }
}

/// [#576 FZ-PERSIST-fs-3] (write-policy fuzzer seeds 437, 10402) A link left at
/// the save's temp name that leads nowhere yet is removed like any leftover
/// temp, and the save writes nothing where it led.
///
/// The cleanup asked `exists()`, which follows the link and says no for one that
/// leads nowhere, so the link stayed and the temp database was created THROUGH
/// it - wherever it pointed, the policy having judged only the project's own
/// name: a whole project in a system or settings folder.
#[cfg(unix)]
#[test]
fn a_dangling_link_at_the_temp_name_is_not_followed_by_the_save() {
    let _config = isolated_config();
    let mut rng = Rng::new(3);
    let session = Session::new(&mut rng);
    let elsewhere = tempfile::TempDir::new().expect("a folder the save must not write");
    let target = elsewhere.path().join("planted.toml");
    let destination = session.dir.path().join("CaveA.stalx");
    std::os::unix::fs::symlink(&target, temp_of(&destination)).expect("plant the link");

    let saved = session.state.save_project(Some(destination.clone()));

    assert!(
        !target.exists() && names_in(elsewhere.path()).is_empty(),
        "the save wrote through the link at its temp name: {:?}",
        names_in(elsewhere.path())
    );
    assert!(saved.success, "the save failed: {:?}", saved.error);
    assert!(
        std::fs::symlink_metadata(temp_of(&destination)).is_err(),
        "the link at the temp name is still there"
    );
    assert!(
        open_fresh(&destination).is_some(),
        "the saved file does not open"
    );
}

/// What a link at a temp name leads to, for the [#576 RVF-b-3] tests: a file not
/// there yet, or another project (a database a save would write into), each in a
/// folder of its own. Returns the target and its bytes before anything runs.
#[cfg(unix)]
fn plant_temp_link(temp: &Path, elsewhere: &Path, existing: bool) -> (PathBuf, Option<Vec<u8>>) {
    let target = elsewhere.join(if existing {
        "Other.stalx"
    } else {
        "planted.stalx"
    });
    if existing {
        crate::persistence::Database::open(&target)
            .expect("create the other project")
            .save_project(&crate::models::Project::new("Other".to_owned(), None))
            .expect("write the other project");
    }
    std::os::unix::fs::symlink(&target, temp).expect("plant the link");
    let before = std::fs::read(&target).ok();
    (target, before)
}

/// Make `dir` read-only (and prove it bites), run `f`, give `dir` its
/// permissions back so the folder can be cleaned up, and return what `f` did.
#[cfg(unix)]
fn in_read_only_dir<T>(dir: &Path, f: impl FnOnce() -> T) -> T {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555))
        .expect("make the folder read-only");
    crate::models::test_support::assert_dir_refuses_new_files(dir);
    let done = f();
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755))
        .expect("give the folder its permissions back");
    done
}

/// What is wrong with a write that had to be refused over a link at its temp
/// name: the link's target was written (created, or its bytes changed), or the
/// write was not refused with the write policy's wording for `file`.
#[cfg(unix)]
fn refused_over_the_link(
    existing: bool,
    target: &Path,
    before: Option<&[u8]>,
    result: &CommandResult,
    file: &str,
) -> Vec<String> {
    let mut problems = Vec::new();
    let after = std::fs::read(target).ok();
    if after.as_deref() != before {
        problems.push(format!(
            "existing {existing}: written through the link at the temp name ({:?} bytes \
             before, {:?} after)",
            before.map(<[u8]>::len),
            after.as_ref().map(Vec::len)
        ));
    }
    let expected = format!("Refused to change {file}: {file}.tmp");
    let message = result.error.as_deref().unwrap_or_default();
    if result.success || !message.starts_with(&expected) {
        problems.push(format!(
            "existing {existing}: not refused as `{expected}...` (success {}): {message}",
            result.success
        ));
    }
    problems
}

/// [#576 RVF-b-3] A link at the save's temp name that cannot be removed - here the
/// destination folder is read-only, as a shared folder is for a link someone else
/// owns - refuses the save, as the write policy words a refusal, and writes
/// nothing where the link leads: neither a new file for a dangling link nor
/// another project the link leads to.
///
/// The removal's error was ignored, so the temp database was opened THROUGH the
/// surviving link: a dangling one got a whole project created where it pointed,
/// one to another project got this project saved into it, and only the final
/// rename (in the read-only folder) failed.
#[cfg(unix)]
#[test]
fn a_link_at_the_temp_name_that_cannot_be_removed_refuses_the_save() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for existing in [false, true] {
        let mut rng = Rng::new(3);
        let session = Session::new(&mut rng);
        let elsewhere = tempfile::TempDir::new().expect("a folder the save must not write");
        let locked = session.dir.path().join("locked");
        std::fs::create_dir(&locked).expect("the destination folder");
        let destination = locked.join("CaveA.stalx");
        let (target, before) = plant_temp_link(&temp_of(&destination), elsewhere.path(), existing);

        let saved = in_read_only_dir(&locked, || {
            session.state.save_project(Some(destination.clone()))
        });

        problems.extend(refused_over_the_link(
            existing,
            &target,
            before.as_deref(),
            &saved,
            "CaveA.stalx",
        ));
        if names_in(&locked)
            .iter()
            .any(|name| name != "CaveA.stalx.tmp")
        {
            problems.push(format!(
                "existing {existing}: written beside the link: {:?}",
                names_in(&locked)
            ));
        }
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// [#576 RVF-b-3] The merge's equivalent: a link at the merged file's temp name
/// that cannot be removed refuses the merge, and nothing is written where it
/// leads.
#[cfg(unix)]
#[test]
fn a_link_at_the_merge_temp_name_that_cannot_be_removed_refuses_the_merge() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for existing in [false, true] {
        let sources_dir = tempfile::TempDir::new().expect("the sources' folder");
        let sources: Vec<PathBuf> = ["CaveA", "CaveB"]
            .iter()
            .map(|name| {
                let path = sources_dir.path().join(format!("{name}.stalx"));
                crate::persistence::Database::open(&path)
                    .expect("create a source")
                    .save_project(&crate::models::Project::new((*name).to_owned(), None))
                    .expect("write a source");
                path
            })
            .collect();
        let elsewhere = tempfile::TempDir::new().expect("a folder the merge must not write");
        let output_dir = tempfile::TempDir::new().expect("the output's folder");
        let output = output_dir.path().join("Merged.stalx");
        let (target, before) = plant_temp_link(&temp_of(&output), elsewhere.path(), existing);
        let state = super::AppState::new().expect("state");

        let merged = in_read_only_dir(output_dir.path(), || {
            state.merge_projects(sources.clone(), output.clone(), "Merged".to_owned(), false)
        });

        problems.extend(refused_over_the_link(
            existing,
            &target,
            before.as_deref(),
            &merged,
            "Merged.stalx",
        ));
    }
    assert!(problems.is_empty(), "{problems:#?}");
}

/// The CI seed list. Seeds that ever failed are added here as regressions.
const GATE_SEEDS: std::ops::RangeInclusive<u32> = 1..=24;

/// Seeds the sweep once failed on, kept as regressions: 196 (display settings a
/// save could not keep are gone for the session too), 285 (a kill leaves an
/// ignore for a shot that only existed in memory), 6268 (a kill leaves a
/// uuid-keyed ignore at a stale position), 27349 (FZ-PERSIST-2: an unsaved shot's
/// ignore moved to another shot by a reverse and back), 35039 (FZ-PERSIST-3: a
/// 0.04 degree north-reference change opened a merge by 4 mm, under the
/// reconcile's 1 cm gate, and the reopen closed it), 3121675 (W27: a reverse
/// could not move a position-keyed ignore onto the slot a uuid-keyed one of the
/// same code was leaving, and the save gave it the other shot's uuid).
const REGRESSION_SEEDS: &[u32] = &[196, 285, 6268, 27349, 35039, 3121675];

#[test]
fn save_fault_fuzz_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for seed in GATE_SEEDS.chain(REGRESSION_SEEDS.iter().copied()) {
        problems.extend(run_seed(seed).0);
    }
    assert!(
        problems.is_empty(),
        "save fault fuzz found {} problem(s):\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
#[ignore = "long run: STALAGMIGHT_RUST_PERSIST_SEEDS / _SEED_START"]
fn save_fault_fuzz_sweep() {
    let _config = isolated_config();
    let start = knob("STALAGMIGHT_RUST_PERSIST_SEED_START", 1) as u32;
    let count = knob("STALAGMIGHT_RUST_PERSIST_SEEDS", 300) as u32;
    let began = Instant::now();
    let mut failed = 0;
    let mut coverage: BTreeMap<String, usize> = BTreeMap::new();
    let want_trace = knob("STALAGMIGHT_RUST_PERSIST_TRACE", 0) == 1;
    for seed in start..start.saturating_add(count) {
        trace_take(want_trace);
        let (problems, counts) = run_seed(seed);
        if (seed - start + 1) % 100 == 0 {
            // A run cut short by a timeout still says how far it got.
            println!(
                "FZ-PERSIST PROGRESS fault seeds={start}..{seed} failed={failed} secs={}",
                began.elapsed().as_secs()
            );
        }
        let steps_taken = trace_take(false);
        for (what, n) in counts {
            *coverage.entry(what).or_insert(0) += n;
        }
        if let Some(first) = problems.first() {
            failed += 1;
            println!(
                "FZ-PERSIST FAIL fault seed={seed} {}",
                first.lines().next().unwrap_or_default()
            );
            for problem in &problems {
                println!("  {problem}");
            }
            for line in &steps_taken {
                println!("  trace: {line}");
            }
        }
    }
    println!("FZ-PERSIST COVERAGE fault {coverage:?}");
    println!(
        "FZ-PERSIST SUMMARY fault seeds={start}..{} failed={failed} secs={}",
        start.saturating_add(count).saturating_sub(1),
        began.elapsed().as_secs()
    );
    assert_eq!(failed, 0, "save fault sweep: {failed} failing seed(s)");
}
