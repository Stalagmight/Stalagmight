//! The reload half of the coverage knobs (see `Knobs` in `fuzz_undo_tests.rs`),
//! and the two always-on oracles the coverage review asked for.
//!
//! * What a reopen must leave: the history whole, or (only after a
//!   `FORMAT_CORRUPT` rewrite) dropped whole; under `TRIM_CROSSINGS`, the
//!   persisted window of each stack and nothing more.
//! * A save that does not reopen (`SAVES`), which must not touch the history.
//! * The refusal probe at the edges of the reachable window: a step past the
//!   floor or the top has to be refused and change nothing.
//! * ALWAYS ON, every mode: after every step the id allocators sit above every
//!   id they could hand out again and no section holds a shot id twice
//!   (`expect_ids_sound`); after every reload every compact entry the history
//!   kept fits the state it will be applied to (`expect_entries_fit`), and a
//!   file this run wrote and nothing touched never loses its history.

use std::collections::BTreeSet;

use serde_json::Value;

use super::fuzz_corrupt_tests::Corruption;
use super::{Fuzzer, HistoryRow, StaleTargets, UNDO_PERSIST_LIMIT};
use crate::core::UndoData;

impl Fuzzer {
    /// After a reopen: hold the history the file gave back to the model, and
    /// move the model to what the reopen legitimately left.
    ///
    /// The checks the crossing always made run here in the order it made them
    /// (`expect_stacks`, then `expect_history_rows`), so with every knob off a
    /// failure reads exactly as it did.
    pub(super) fn settle_reload(&mut self, before: &[HistoryRow], corruption: Option<&Corruption>) {
        if self.state.history_discarded() {
            let Some(corruption) = corruption else {
                self.fail(
                    "the reload DROPPED the history of a file this run wrote and nothing touched",
                    &format!(
                        "  undo={} redo={} after the reopen; a file the app saved itself must \
                         load its own history whole",
                        self.undo_len(),
                        self.redo_len()
                    ),
                );
            };
            let (undo, redo) = (self.undo_len(), self.redo_len());
            if undo != 0 || redo != 0 {
                self.fail(
                    &format!(
                        "a damaged history was reported discarded but PART of it was kept ({})",
                        corruption.kind_name()
                    ),
                    &format!(
                        "  after {corruption}: undo={undo} redo={redo} survived the load. A \
                         compact history is usable only whole (docs/undo-redo.md, \"Persisted \
                         rows\"), so a load that drops it drops both stacks"
                    ),
                );
            }
            if corruption.must_be_kept() {
                self.fail(
                    &format!(
                        "the load dropped a history over a row the engine tolerates by design ({})",
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
            // Nothing below or above is reachable again, exactly as a FIFO trim
            // leaves it below: `snapshots` stays indexed by absolute depth.
            self.floor = self.cursor;
            self.truncate_above(self.cursor);
            self.expect_stacks("reload");
            return;
        }
        if let Some(corruption) = corruption {
            if corruption.must_be_refused() {
                self.fail(
                    &format!(
                        "a damaged row the load is bound to refuse was read as history ({})",
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
        }
        let (undo_kept, redo_kept) = self.apply_persisted_window();
        self.expect_stacks("reload");
        let kept: Vec<HistoryRow> = before
            .iter()
            .filter(|row| in_window(&row.label, undo_kept, redo_kept))
            .cloned()
            .collect();
        self.expect_history_rows(&kept);
        self.expect_entries_fit("reload");
    }

    /// `TRIM_CROSSINGS`: only the newest `undo_persist_limit` entries of each
    /// stack survive a save (`UndoStack::rows_to_persist`), so after the reopen
    /// the reachable floor is `cursor - window` and the top `cursor + window`.
    /// Returns how many entries each stack kept.
    ///
    /// Without the knob a crossing that would trim is never taken
    /// (`crossing_would_truncate`), so nothing is cut and the model is left
    /// exactly as it was.
    fn apply_persisted_window(&mut self) -> (usize, usize) {
        let undo_held = self.cursor - self.floor;
        let redo_held = self.top - self.cursor;
        if !self.knobs.trim_crossings {
            return (undo_held, redo_held);
        }
        let undo_kept = undo_held.min(UNDO_PERSIST_LIMIT);
        let redo_kept = redo_held.min(self.history_limit).min(UNDO_PERSIST_LIMIT);
        if undo_kept == undo_held && redo_kept == redo_held {
            return (undo_kept, redo_kept);
        }
        let cut = (undo_held - undo_kept) + (redo_held - redo_kept);
        self.outcome.coverage.trimmed_crossings += 1;
        self.outcome.coverage.window_cut += cut;
        self.note(format!(
            "  ^ the save kept the persisted window: undo {undo_held}->{undo_kept}, redo \
             {redo_held}->{redo_kept}"
        ));
        self.floor = self.cursor - undo_kept;
        self.truncate_above(self.cursor + redo_kept);
        (undo_kept, redo_kept)
    }

    /// Forget every depth above `depth`: the model's top becomes `depth`.
    pub(super) fn truncate_above(&mut self, depth: usize) {
        self.top = depth;
        self.snapshots.truncate(depth + 1);
        self.verdicts.truncate(depth + 1);
        self.adjusted.truncate(depth + 1);
        self.epochs.truncate(depth + 1);
        self.entry_meta.truncate(depth + 1);
        self.entry_prints.truncate(depth + 1);
    }

    /// ALWAYS ON, after every reload: every compact entry the loaded history
    /// holds fits the state it will be applied to.
    ///
    /// Each stack is walked from the live project outwards - newest entry
    /// first, each delta applied to what the one before it produced - which is
    /// the route the steps will take. The load's own check
    /// (`drop_history_that_does_not_fit`) walks the same route over section
    /// and shot ownership only; this applies the whole delta, so a top-level
    /// list whose recorded length no longer matches is seen too. A typed entry
    /// cannot be followed without executing it, so the walk of a stack stops
    /// there, as the load's does.
    ///
    /// A miss is "loads, then a step refuses": the history came back, and the
    /// walk will be refused at that entry, for good, with every entry beyond it.
    fn expect_entries_fit(&mut self, what: &str) {
        let Ok(live) = self.state.capture_project_snapshot() else {
            return;
        };
        let live: Value = serde_json::from_str(&live).expect("the live snapshot is JSON");
        let mut misfits = Vec::new();
        let mut checked = 0;
        if let Ok(stack) = self.state.undo_stack.lock() {
            let stacks: [(&str, Vec<&UndoData>); 2] = [
                ("undo", stack.undo_entries().map(|e| &e.undo_data).collect()),
                ("redo", stack.redo_entries().map(|e| &e.undo_data).collect()),
            ];
            for (side, entries) in stacks {
                let mut state = live.clone();
                for (index, data) in entries.iter().enumerate() {
                    let UndoData::Delta(delta) = data else {
                        break;
                    };
                    match delta.delta.apply(&state) {
                        Ok(next) => {
                            state = next;
                            checked += 1;
                        }
                        Err(reason) => {
                            misfits.push(format!("  {side}[{index}]: {reason}"));
                            break;
                        }
                    }
                }
            }
        }
        self.outcome.coverage.fit_checked += checked;
        if !misfits.is_empty() {
            self.fail(
                &format!("{what} kept a compact entry that does not fit the state it applies to"),
                &format!(
                    "  a step through it would be refused, and every entry beyond it with it:\n{}",
                    misfits.join("\n")
                ),
            );
        }
    }

    /// ALWAYS ON, after every step: per section, every shot id is unique and
    /// below the section's `next_shot_id`, and `next_section_id` is above every
    /// section id. The snapshot oracle cannot see either - it replaces shot ids
    /// with positions once a run has reloaded, and it drops the allocators -
    /// so an allocator left behind the ids it hands out next would otherwise
    /// surface only as a later collision, if at all.
    pub(super) fn expect_ids_sound(&self, what: &str) {
        let Some(project) = self.state.get_project_state() else {
            return;
        };
        let mut wrong = Vec::new();
        for section in &project.sections {
            if section.id >= project.next_section_id {
                wrong.push(format!(
                    "  section {} is not below next_section_id {}",
                    section.id, project.next_section_id
                ));
            }
            let mut seen = BTreeSet::new();
            for shot in &section.shots {
                if !seen.insert(shot.id) {
                    wrong.push(format!(
                        "  section {} holds shot id {} twice",
                        section.id, shot.id
                    ));
                }
                if shot.id >= section.next_shot_id {
                    wrong.push(format!(
                        "  section {} holds shot id {}, not below its next_shot_id {}",
                        section.id, shot.id, section.next_shot_id
                    ));
                }
            }
        }
        if wrong.is_empty() {
            return;
        }
        wrong.truncate(8);
        self.fail(
            &format!("{what} left an id allocator behind the ids it hands out"),
            &wrong.join("\n"),
        );
    }

    /// `SAVES`: save where the project is, with no close and no reopen.
    ///
    /// A save makes undo-less changes (the gauge capture, the offset heals), so
    /// it opens a new epoch exactly as the save half of a crossing does - and
    /// it may not touch the history at all: the stacks and every row of the
    /// dropdown are what they were.
    pub(super) fn save_in_place(&mut self) {
        let before = self.history_rows();
        let temp = self
            .temp
            .get_or_insert_with(|| tempfile::TempDir::new().unwrap());
        let path = temp.path().join("fuzz.stalx");
        self.epoch += 1;
        self.starting_move("save in place".to_string());
        let saved = self.state.save_project(Some(path));
        self.finished_move();
        if !saved.success {
            self.fail(
                "save_project failed",
                &format!("  error: {:?}", saved.error),
            );
        }
        self.outcome.saves += 1;
        self.outcome.coverage.saves_in_place += 1;
        self.note(format!("save in place -> ok (depth {})", self.cursor));
        self.expect_stacks("save in place");
        self.expect_history_rows(&before);
        self.expect_state(
            self.cursor,
            "save in place",
            Vec::new(),
            StaleTargets::default(),
        );
        self.expect_ids_sound("save in place");
    }

    /// At an edge of the reachable window - the floor after unwinding, the top
    /// after rewinding - one step further has to be REFUSED, and a refused step
    /// may change nothing: not the stacks, not the project.
    ///
    /// No draw: it runs at a fixed point of the closing walk, under any coverage
    /// knob. A trimmed reopen, a dropped history and a drawn limit all move the
    /// edges, which is what makes the probe worth having.
    pub(super) fn probe_refusal(&mut self, back: bool) {
        let what = if back {
            "an undo past the floor"
        } else {
            "a redo past the top"
        };
        let stacks = (self.undo_len(), self.redo_len());
        if (if back { stacks.0 } else { stacks.1 }) != 0 {
            return;
        }
        let before = self.snapshot();
        self.starting_move(what.to_string());
        let result = if back {
            self.state.undo()
        } else {
            self.state.redo()
        };
        self.finished_move();
        if result.success {
            self.fail(
                &format!("{what} SUCCEEDED"),
                &format!(
                    "  the model's window is [{}, {}] at depth {}, and the app stepped out of it",
                    self.floor, self.top, self.cursor
                ),
            );
        }
        if (self.undo_len(), self.redo_len()) != stacks {
            self.fail(
                &format!("{what} was refused but changed the history stacks"),
                &format!(
                    "  undo/redo {:?} -> {:?}",
                    stacks,
                    (self.undo_len(), self.redo_len())
                ),
            );
        }
        if self.snapshot() != before {
            self.fail(
                &format!("{what} was refused but changed the project"),
                "  a refused history move must be a strict no-op",
            );
        }
        self.outcome.coverage.refusals += 1;
        self.note(format!("{what} -> refused, nothing changed"));
    }
}

/// Is the history row labelled `undo[i]` / `redo[j]` inside the persisted
/// window? Index 0 is the newest entry on both stacks, and the window keeps the
/// newest.
fn in_window(label: &str, undo_kept: usize, redo_kept: usize) -> bool {
    let index = |prefix: &str| -> Option<usize> {
        label.strip_prefix(prefix)?.strip_suffix(']')?.parse().ok()
    };
    if let Some(i) = index("undo[") {
        return i < undo_kept;
    }
    if let Some(j) = index("redo[") {
        return j < redo_kept;
    }
    true
}
