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

use super::format_property_tests::open_fresh;
use super::fuzz_undo_tests::{random_shot, Rng};
use super::persist_roundtrip_fuzz_tests::{
    adopt_live_bookkeeping, check_file, edit, knob, observe_state, side_tables_of, side_write,
    Session, SideModel,
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

    let outcome = match fault {
        Fault::Kill(kill) => check_kill(
            &mut session,
            request.as_deref(),
            &destination,
            old_bytes.as_deref(),
            kill,
        ),
        _ => check_failure(
            &mut session,
            request.as_deref(),
            &destination,
            old_bytes.as_deref(),
            fault,
            detached,
        ),
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

/// A save under `fault`, then the retry. `Ok` names what the save did.
fn check_failure(
    session: &mut Session,
    request: Option<&Path>,
    destination: &Path,
    old_bytes: Option<&[u8]>,
    fault: Fault,
    detached: bool,
) -> Result<&'static str, Vec<String>> {
    let mut problems = Vec::new();
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
    } else {
        let now = std::fs::read(destination).ok();
        if now.as_deref() != old_bytes {
            problems.push(format!(
                "the save failed ({:?}) but the destination changed: {} -> {} bytes",
                result.error,
                old_bytes.map_or(0, <[u8]>::len),
                now.as_ref().map_or(0, Vec::len)
            ));
        }
        if temp_of(destination).exists() {
            problems.push("a failed save left its .stalx.tmp behind".to_owned());
        }
    }
    // The session goes on: the same save again, with nothing failing.
    let retry = session.state.save_project(request.map(Path::to_path_buf));
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
        Ok(what)
    } else {
        Err(problems)
    }
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

/// The CI seed list. Seeds that ever failed are added here as regressions.
const GATE_SEEDS: std::ops::RangeInclusive<u32> = 1..=24;

#[test]
fn save_fault_fuzz_gate() {
    let _config = isolated_config();
    let mut problems = Vec::new();
    for seed in GATE_SEEDS {
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
    for seed in start..start.saturating_add(count) {
        let (problems, counts) = run_seed(seed);
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
