//! Undo, redo and the history jumps on the changeset engine (plan 8,
//! `docs/undo-redo.md`).
//!
//! A step applies the entry's changeset either way with its preconditions
//! (`changeset::step`): a refusal changes nothing and keeps the entry where
//! it was. An entry decoded from a file (no session ids) is also validated
//! after its step and rolled back with the inverse step on a violation
//! (U10: only a crafted file can get there). Nothing derived is stored, so
//! nothing is re-measured or healed after a step: `undo_to` / `redo_to`
//! apply N steps and bump the revision once.

use crate::core::undo::history_item::history_item;
use crate::core::{CommandResult, ReversedLine, UndoHistoryItem};
use crate::models::survey::changeset::{step, Direction};

use super::AppState;

impl AppState {
    /// Undo the most recent step.
    pub fn undo(&self) -> CommandResult {
        self.step_history(Direction::Undo, None)
    }

    /// Redo the most recently undone step.
    pub fn redo(&self) -> CommandResult {
        self.step_history(Direction::Redo, None)
    }

    /// Undo through history item `index` (0 = most recent): `index + 1`
    /// steps. A refusal mid-jump reports how far it got, and the reversed
    /// lines of the steps already applied.
    pub fn undo_to(&self, index: usize) -> CommandResult {
        self.step_history(Direction::Undo, Some(index))
    }

    /// Redo through redo item `index` (0 = next redo).
    pub fn redo_to(&self, index: usize) -> CommandResult {
        self.step_history(Direction::Redo, Some(index))
    }

    fn step_history(&self, direction: Direction, to: Option<usize>) -> CommandResult {
        let Ok(_cmd_guard) = self.command_lock.lock() else {
            return CommandResult::failure("Failed to acquire command lock".to_string());
        };
        let verb = match direction {
            Direction::Undo => "Undo",
            Direction::Redo => "Redo",
        };
        // Read-only edit gate (#191, D-B6): a read-only remote project cannot
        // be changed even to revert.
        if let Err(f) = self.guard_mutation(verb) {
            return f;
        }
        let read_only = self.with_file(|file| file.extras.read_only.is_some());
        if read_only {
            return CommandResult::failure(format!(
                "{}: {verb} is disabled: this project was made with a newer Stalagmight",
                super::open_file::READ_ONLY_FILE_CODE
            ));
        }
        let mut guard = self
            .project
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(project) = guard.as_mut() else {
            return CommandResult::failure("No project open".to_string());
        };
        let available = {
            let stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match direction {
                Direction::Undo => stack.len(),
                Direction::Redo => stack.redo_len(),
            }
        };
        let count = match to {
            None if available == 0 => {
                return CommandResult::failure(format!("Nothing to {}", verb.to_lowercase()));
            }
            None => 1,
            Some(index) if index >= available => {
                return CommandResult::failure(format!(
                    "Invalid {} index: {index} (stack has {available} entries)",
                    verb.to_lowercase()
                ));
            }
            Some(index) => index + 1,
        };
        let mut reversed: Vec<ReversedLine> = Vec::new();
        let mut failure = None;
        let mut applied = 0usize;
        for i in 0..count {
            let entry = {
                let mut stack = self
                    .undo_stack
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                match direction {
                    Direction::Undo => stack.pop_undo(),
                    Direction::Redo => stack.pop_redo(),
                }
            };
            let Some(entry) = entry else {
                break;
            };
            let refused = match step(project, &entry.entry().changes, direction) {
                Err(refusal) => Some(refusal.to_string()),
                Ok(()) if !entry.entry().changes.has_session_ids() => {
                    let violations = project.validate();
                    violations.first().map(|first| {
                        let back = match direction {
                            Direction::Undo => Direction::Redo,
                            Direction::Redo => Direction::Undo,
                        };
                        if let Err(e) = step(project, &entry.entry().changes, back) {
                            tracing::error!(error = %e, "rolling back an invalid history step failed");
                        }
                        format!("the step would leave the survey invalid: {first}")
                    })
                }
                Ok(()) => None,
            };
            let mut stack = self
                .undo_stack
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(reason) = refused {
                tracing::warn!(step = i + 1, of = count, reason = %reason, "history step refused");
                match direction {
                    Direction::Undo => stack.push_undo(entry),
                    Direction::Redo => stack.push_redo(entry),
                }
                failure = Some(if count == 1 {
                    format!("{verb} failed: {reason}")
                } else {
                    format!("{verb} failed at step {} of {count}: {reason}", i + 1)
                });
                break;
            }
            reversed.extend(entry.entry().changes.digest().reversed_lines.iter().map(
                |&(line_id, shot_count)| ReversedLine {
                    line_id,
                    shot_count,
                },
            ));
            match direction {
                Direction::Undo => stack.push_redo(entry),
                Direction::Redo => stack.push_undo(entry),
            }
            applied += 1;
        }
        if applied > 0 {
            project.touch();
            self.bump_revision();
            self.with_file(|file| file.dirty = true);
        }
        drop(guard);
        if applied > 0 {
            // A step is an edit of a linked project (set-only dirty bit).
            self.mark_remote_dirty();
        }
        let result = match failure {
            Some(message) => CommandResult::failure(message),
            None => CommandResult::success(),
        };
        if reversed.is_empty() {
            result
        } else {
            result.with_reversed_lines(reversed)
        }
    }

    pub fn can_undo(&self) -> bool {
        self.undo_stack.lock().is_ok_and(|s| s.can_undo())
    }

    pub fn can_redo(&self) -> bool {
        self.undo_stack.lock().is_ok_and(|s| s.can_redo())
    }

    /// The undo history, most recent first: the record-time labels and the
    /// previews of each entry.
    pub fn get_undo_stack(&self) -> Vec<UndoHistoryItem> {
        self.undo_stack
            .lock()
            .map(|stack| {
                stack
                    .undo_entries()
                    .enumerate()
                    .map(|(i, e)| history_item(i, e.entry()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The redo history, next redo first.
    pub fn get_redo_stack(&self) -> Vec<UndoHistoryItem> {
        self.undo_stack
            .lock()
            .map(|stack| {
                stack
                    .redo_entries()
                    .enumerate()
                    .map(|(i, e)| history_item(i, e.entry()))
                    .collect()
            })
            .unwrap_or_default()
    }
}
