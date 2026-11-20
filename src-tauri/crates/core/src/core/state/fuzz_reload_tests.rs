//! The persistence half of the undo fuzzer (`fuzz_undo_tests.rs`): the
//! save/close/reopen crossing, saves in place, and the probes that hold a
//! REFUSED step to being a strict no-op.
//!
//! * What a reopen must leave: the history whole (every entry, every
//!   history item as it read before the save), or - only after a
//!   `FORMAT_CORRUPT` rewrite the load is bound to refuse - dropped whole;
//!   under `TRIM_CROSSINGS`, the newest `undo_persist_limit` entries of each
//!   stack and nothing more.
//! * A save that does not reopen (`SAVES`) touches neither the history nor
//!   the survey, and leaves the project clean.
//! * At the edges of the reachable window an undo or a redo is refused and
//!   changes nothing; a step through an entry whose preconditions were
//!   broken behind the engine's back (`testkit::tamper`, or a damaged entry a
//!   reopen kept) is refused, changes nothing and KEEPS its entry.

use super::fuzz_corrupt_tests::{self, Corruption};
use super::{canonical_bytes, item_text, Fuzzer};
use crate::core::Command;
use crate::models::survey::changeset::{testkit, Direction};

impl Fuzzer {
    /// The lowest depth a walk can reach: the floor, or a damaged entry the
    /// reopen kept (a step through it refuses), whichever is higher.
    pub(super) fn reach_floor(&self) -> usize {
        self.walls
            .range(..=self.cursor)
            .next_back()
            .map_or(self.floor, |&wall| wall.max(self.floor))
    }

    /// The highest depth a walk can reach.
    pub(super) fn reach_top(&self) -> usize {
        self.walls
            .range(self.cursor + 1..)
            .next()
            .map_or(self.top, |&wall| (wall - 1).min(self.top))
    }

    /// One save/close/reopen.
    pub(super) fn cross_once(&mut self) {
        let path = self.save_path();
        self.starting_move("save".to_string());
        let saved = self.catching(|f| f.state.save_project(Some(path.clone())));
        self.finished_move();
        let Some(saved) = saved else {
            return;
        };
        if !saved.success {
            self.fail(
                "save_project failed",
                &format!("  error: {:?}", saved.error),
            );
        }
        self.outcome.saves += 1;
        self.note(format!("save -> ok (depth {})", self.cursor));
        if self.state.has_unsaved_changes() {
            self.fail("a save left the project dirty", "");
        }
        self.expect_stacks("save");
        self.expect_state(self.cursor, "save", true);

        self.starting_move("close/reopen".to_string());
        let closed = self.state.execute(Command::CloseProject);
        if !closed.success {
            self.fail(
                "CloseProject failed",
                &format!("  error: {:?}", closed.error),
            );
        }
        let corruption = if self.knobs.format_corrupt && self.aux.chance(CORRUPT_PERCENT) {
            fuzz_corrupt_tests::corrupt_one_entry(&path, &mut self.aux)
        } else {
            None
        };
        if let Some(corruption) = &corruption {
            self.outcome.coverage.corrupted += 1;
            self.note(format!("  ^ corrupted {corruption}"));
        }
        let opened =
            self.catching(|f| f.state.execute(Command::OpenProject { path: path.clone() }));
        self.finished_move();
        let Some(opened) = opened else {
            return;
        };
        if !opened.success {
            self.fail(
                "OpenProject failed",
                &format!("  error: {:?}", opened.error),
            );
        }
        self.outcome.reloads += 1;
        // Every entry on the stacks is a decoded one now: no session ids, and
        // the loader assigned fresh ones.
        self.id_gen += 1;
        self.id_taint = false;
        for depth in self.depths.iter_mut().flatten() {
            depth.session = false;
        }
        self.note(format!("reload -> depth {}", self.cursor));
        self.settle_reload(corruption.as_ref());
        self.expect_state(self.cursor, "reload", false);
    }

    /// The file a crossing saves to: the run's own temp dir.
    fn save_path(&mut self) -> std::path::PathBuf {
        self.temp
            .get_or_insert_with(|| tempfile::TempDir::new().unwrap())
            .path()
            .join("fuzz.stalx")
    }

    /// After a reopen: hold the history the file gave back to the model.
    fn settle_reload(&mut self, corruption: Option<&Corruption>) {
        if self.state.history_discarded() {
            let Some(corruption) = corruption else {
                self.fail(
                    "the reload DROPPED the history of a file this run wrote and nothing touched",
                    &format!(
                        "  undo={} redo={} after the reopen",
                        self.undo_len(),
                        self.redo_len()
                    ),
                );
            };
            if self.undo_len() != 0 || self.redo_len() != 0 {
                self.fail(
                    &format!(
                        "a damaged history was reported dropped but PART of it was kept ({})",
                        corruption.kind_name()
                    ),
                    &format!(
                        "  after {corruption}: undo={} redo={}",
                        self.undo_len(),
                        self.redo_len()
                    ),
                );
            }
            if corruption.must_be_kept() {
                self.fail(
                    &format!(
                        "the load dropped a history over an entry it decodes ({})",
                        corruption.kind_name()
                    ),
                    &format!("  after {corruption}"),
                );
            }
            self.outcome.coverage.dropped += 1;
            self.note(format!(
                "  ^ the reload dropped the whole history ({} below, {} above)",
                self.cursor - self.floor,
                self.top - self.cursor
            ));
            self.walls.clear();
            self.raise_floor(self.cursor);
            self.truncate_above(self.cursor);
            self.expect_stacks("reload");
            return;
        }
        let (undo_kept, redo_kept) = self.apply_persisted_window();
        if let Some(corruption) = corruption {
            if !corruption.must_be_kept() {
                self.fail(
                    &format!(
                        "a damaged entry the load is bound to refuse was read as history ({})",
                        corruption.kind_name()
                    ),
                    &format!(
                        "  after {corruption}: undo={} redo={} loaded",
                        self.undo_len(),
                        self.redo_len()
                    ),
                );
            }
            self.outcome.coverage.kept_corrupted += 1;
            // The damaged entry fits nothing: a step through it refuses. A
            // walled entry rides every later save as it was read, so a second
            // `line-hash` damage of it flips the same bit back (the first
            // `"h"` of the entry, xor 1): it fits again.
            if let Some(depth) = corruption.depth(self.cursor, undo_kept, redo_kept) {
                if self.walls.remove(&depth) {
                    self.note(format!(
                        "  ^ the entry into depth {depth} was damaged twice: it is whole again"
                    ));
                } else {
                    self.walls.insert(depth);
                    self.note(format!(
                        "  ^ a step through the entry into depth {depth} must refuse"
                    ));
                }
            }
        }
        self.expect_stacks("reload");
        self.expect_entries_fit("reload");
    }

    /// ALWAYS ON, after every reopen that kept the history: every entry it
    /// kept steps, on a clone of the live survey, onto the state recorded at
    /// the depth it leads to - the whole undo stack newest first, the whole
    /// redo stack next first - and leaves the survey valid, so no kept entry
    /// "loads, then refuses" (except the one a `line-hash` damage walls off,
    /// which must refuse). The closing walk executes the reachable ones for
    /// real; this also covers the ones a later push clears unstepped.
    fn expect_entries_fit(&mut self, what: &str) {
        let (undo, redo) = {
            let stack = self.state.undo_stack.lock().unwrap();
            (
                stack
                    .undo_entries()
                    .map(|e| e.entry().changes.clone())
                    .collect::<Vec<_>>(),
                stack
                    .redo_entries()
                    .map(|e| e.entry().changes.clone())
                    .collect::<Vec<_>>(),
            )
        };
        let live = self.live();
        for (direction, entries) in [(Direction::Undo, undo), (Direction::Redo, redo)] {
            let mut survey = live.clone();
            for (i, changes) in entries.iter().enumerate() {
                let (entry_depth, target) = match direction {
                    Direction::Undo => (self.cursor - i, self.cursor - i - 1),
                    Direction::Redo => (self.cursor + 1 + i, self.cursor + 1 + i),
                };
                let stepped =
                    crate::models::survey::changeset::step(&mut survey, changes, direction);
                if self.walls.contains(&entry_depth) {
                    if stepped.is_ok() {
                        self.fail(
                            &format!("{what} kept a damaged entry that still steps"),
                            &format!("  the entry into depth {entry_depth}"),
                        );
                    }
                    break;
                }
                if let Err(refusal) = stepped {
                    self.fail(
                        &format!("{what} kept an entry that does not fit the state it applies to"),
                        &format!(
                            "  {direction:?} through the entry into depth {entry_depth}: {refusal}"
                        ),
                    );
                }
                let violations = survey.validate();
                if !violations.is_empty() {
                    self.fail(
                        &format!("{what} kept an entry whose step leaves the survey invalid"),
                        &format!("  {direction:?} into depth {target}: {}", violations[0]),
                    );
                }
                self.outcome.coverage.fit_checked += 1;
                let recorded = self.depth(target);
                if canonical_bytes(&survey, recorded.project.counters) != recorded.bytes {
                    self.fail(
                        &format!("{what} kept an entry that steps somewhere else"),
                        &format!("  {direction:?} into depth {target} misses its record"),
                    );
                }
            }
        }
    }

    /// `TRIM_CROSSINGS`: only the newest `undo_persist_limit` entries of each
    /// stack ride the file. Without the knob a crossing that would cut is
    /// never taken, so nothing is cut. Returns how many each stack kept.
    fn apply_persisted_window(&mut self) -> (usize, usize) {
        let undo_held = self.cursor - self.floor;
        let redo_held = self.top - self.cursor;
        let undo_kept = undo_held.min(self.persist_limit);
        let redo_kept = redo_held.min(self.persist_limit);
        if undo_kept == undo_held && redo_kept == redo_held {
            return (undo_kept, redo_kept);
        }
        self.outcome.coverage.trimmed_crossings += 1;
        self.outcome.coverage.window_cut += (undo_held - undo_kept) + (redo_held - redo_kept);
        self.note(format!(
            "  ^ the save kept the persisted window: undo {undo_held}->{undo_kept}, redo \
             {redo_held}->{redo_kept}"
        ));
        self.raise_floor(self.cursor - undo_kept);
        self.truncate_above(self.cursor + redo_kept);
        let (floor, top) = (self.floor, self.top);
        self.walls.retain(|&wall| wall > floor && wall <= top);
        (undo_kept, redo_kept)
    }

    /// `SAVES`: save where the project is, with no close and no reopen. The
    /// history and the survey are what they were; the project is clean.
    pub(super) fn save_in_place(&mut self) {
        let path = self.save_path();
        self.starting_move("save in place".to_string());
        let saved = self.catching(|f| f.state.save_project(Some(path.clone())));
        self.finished_move();
        let Some(saved) = saved else {
            return;
        };
        if !saved.success {
            self.fail(
                "save_project failed",
                &format!("  error: {:?}", saved.error),
            );
        }
        if self.state.has_unsaved_changes() {
            self.fail("a save in place left the project dirty", "");
        }
        self.outcome.saves += 1;
        self.outcome.coverage.saves_in_place += 1;
        self.note(format!("save in place -> ok (depth {})", self.cursor));
        self.expect_stacks("save in place");
        self.expect_state(self.cursor, "save in place", true);
    }

    /// At an edge of the window the app holds, one step further is refused
    /// and changes nothing. Probed only where the stack in that direction is
    /// empty: a wall (a damaged entry) is `probe_wall`'s.
    pub(super) fn probe_refusal(&mut self, back: bool) {
        let (undo, redo) = (self.undo_len(), self.redo_len());
        if (if back { undo } else { redo }) != 0 {
            return;
        }
        let what = if back {
            "an undo past the floor"
        } else {
            "a redo past the top"
        };
        self.refused_step(back, what);
    }

    /// A step through an entry a reopen kept although it fits nothing.
    pub(super) fn probe_wall(&mut self, back: bool) {
        let at = if back { self.cursor } else { self.cursor + 1 };
        if !self.walls.contains(&at) {
            return;
        }
        self.refused_step(back, "a step through a damaged entry");
        self.outcome.coverage.refused_corrupted += 1;
    }

    /// Change, behind the engine's back, something the next step in that
    /// direction checks (`testkit::tamper`); the step must be refused, leave
    /// the tampered survey exactly as it was and keep its entry. Then the
    /// untampered survey is put back.
    pub(super) fn probe_tamper(&mut self, back: bool) {
        let direction = if back {
            Direction::Undo
        } else {
            Direction::Redo
        };
        let changes = {
            let stack = self.state.undo_stack.lock().unwrap();
            let entry = if back {
                stack.peek_undo()
            } else {
                stack.peek_redo()
            };
            let Some(entry) = entry else {
                return;
            };
            entry.entry().changes.clone()
        };
        let original = self.live();
        let mut tampered = original.clone();
        let Some(how) = testkit::tamper(&mut tampered, &changes, direction) else {
            return;
        };
        self.put_project(tampered);
        self.note(format!("  tamper: {how}"));
        self.refused_step(back, &format!("a step over a tampered survey ({how})"));
        self.put_project(original);
        self.outcome.tampered += 1;
        self.expect_stacks("the tamper probe");
        self.expect_state(self.cursor, "the tamper probe", true);
    }

    /// Replace the survey in the app's slot (a test-only back door), as a
    /// stale view or a third-party edit would leave it.
    fn put_project(&self, survey: crate::models::survey::Project) {
        let mut guard = self.state.project.lock().unwrap();
        *guard = Some(survey);
        drop(guard);
        self.state.bump_revision();
    }

    /// Take one step that MUST be refused, and hold the refusal to changing
    /// nothing at all.
    fn refused_step(&mut self, back: bool, what: &str) {
        let before = self.live();
        let undo: Vec<String> = self.state.get_undo_stack().iter().map(item_text).collect();
        let redo: Vec<String> = self.state.get_redo_stack().iter().map(item_text).collect();
        let dirty = self.state.has_unsaved_changes();
        self.starting_move(what.to_string());
        let result = self.catching(|f| if back { f.state.undo() } else { f.state.redo() });
        self.finished_move();
        let Some(result) = result else {
            return;
        };
        if result.success {
            self.fail(
                &format!("{what} SUCCEEDED"),
                &format!(
                    "  the window is [{}, {}] at depth {}, and the app stepped out of it",
                    self.floor, self.top, self.cursor
                ),
            );
        }
        let undo_after: Vec<String> = self.state.get_undo_stack().iter().map(item_text).collect();
        let redo_after: Vec<String> = self.state.get_redo_stack().iter().map(item_text).collect();
        if undo_after != undo || redo_after != redo {
            self.fail(
                &format!("{what} was refused but changed the history stacks"),
                &format!(
                    "  undo {} -> {}, redo {} -> {} (the refused entry must stay where it was)",
                    undo.len(),
                    undo_after.len(),
                    redo.len(),
                    redo_after.len()
                ),
            );
        }
        let after = self.live();
        if after != before
            || after.modified_at != before.modified_at
            || testkit::session_ids(&after) != testkit::session_ids(&before)
        {
            self.fail(
                &format!("{what} was refused but changed the survey"),
                &format!("  error: {:?}", result.error),
            );
        }
        if self.state.has_unsaved_changes() != dirty {
            self.fail(
                &format!("{what} was refused but changed the dirty flag"),
                "",
            );
        }
        self.outcome.refusals += 1;
        self.note(format!("{what} -> refused, nothing changed"));
    }
}

/// Half the crossings of a `FORMAT_CORRUPT` run damage an entry.
const CORRUPT_PERCENT: u32 = 50;

/// FOUND by this port (`TRIM_CROSSINGS`), fixed: a save persists only the
/// newest `undo_persist_limit` entries of each stack, as 0.6.0 did
/// (`history_rows_to_persist`); the station-graph save wrote the whole
/// in-memory history (up to `undo_history_limit`, 100), so the user's
/// `undo_persist_limit` setting did nothing and every file carried five times
/// the history it should.
#[test]
fn a_save_persists_only_the_newest_undo_persist_limit_entries() {
    let _config = crate::test_helpers::isolated_config();
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("window.stalx");
    let mut fuzzer = Fuzzer::new(3, false);
    let limit = fuzzer.persist_limit;
    let mut pushed = 0;
    let mut length = 1.0;
    while pushed < limit + 5 {
        let targets = super::read_targets(&fuzzer.state);
        let (line_id, shot_id) = fuzzer.pick_shot(&targets).unwrap();
        length += 0.25;
        let r = fuzzer.state.execute(Command::UpdateShot {
            line_id,
            shot_id,
            updates: crate::models::ShotUpdates {
                length: Some(length),
                ..crate::models::ShotUpdates::default()
            },
        });
        assert!(r.success, "{:?}", r.error);
        pushed += 1;
    }
    // Two undone: a redo branch too.
    assert!(fuzzer.state.undo().success && fuzzer.state.undo().success);
    let (undo_held, redo_held) = (fuzzer.undo_len(), fuzzer.redo_len());
    assert!(undo_held > limit);
    let newest_undo: Vec<String> = fuzzer
        .state
        .get_undo_stack()
        .iter()
        .take(limit)
        .map(item_text)
        .collect();
    let redo: Vec<String> = fuzzer
        .state
        .get_redo_stack()
        .iter()
        .map(item_text)
        .collect();
    assert!(fuzzer.state.save_project(Some(path.clone())).success);
    assert_eq!(
        fuzzer.undo_len(),
        undo_held,
        "a save trims nothing in memory"
    );
    let decoded = crate::persistence::stalx_v2::decode(
        &std::fs::read(&path).unwrap(),
        &crate::persistence::stalx_v2::DecodeOptions::default(),
    )
    .unwrap();
    assert_eq!(decoded.extras.history.undo.len(), limit);
    assert_eq!(decoded.extras.history.redo.len(), redo_held.min(limit));
    assert!(fuzzer.state.execute(Command::CloseProject).success);
    assert!(fuzzer.state.execute(Command::OpenProject { path }).success);
    assert!(!fuzzer.state.history_discarded());
    let reopened: Vec<String> = fuzzer
        .state
        .get_undo_stack()
        .iter()
        .map(item_text)
        .collect();
    assert_eq!(
        reopened, newest_undo,
        "the newest entries, in order, came back"
    );
    let reopened_redo: Vec<String> = fuzzer
        .state
        .get_redo_stack()
        .iter()
        .map(item_text)
        .collect();
    assert_eq!(reopened_redo, redo);
}
