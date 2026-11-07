//! [#576 A4] The forward-mutation epilogue: what every forward entry point runs
//! once its mutation has succeeded.
//!
//! A forward edit leaves derived state behind that has to be settled before it is
//! recorded: a loop closure that became a sole bridge is a merge (#8), a promoted
//! datum re-defines the georeference frame (#469 item 1), and merged stations must
//! line up (`reconcile_geometry`). Then the edit is recorded (an after-snapshot and
//! an undo entry) and, last, the open SpeleoDB copy is marked dirty. Every entry
//! point that takes `command_lock` and mutates used to write this sequence out by
//! hand - `execute`, the batch, Repair, the four delete/split paths, Insert
//! station, both imports, the two project-setting edits, the comment move - and
//! the copies had drifted: Insert station (split at distance) ran only the
//! reconcile. They all run [`AppState::forward_epilogue`] now, which owns the
//! ORDER; each passes the steps its edit needs and a `record` callback for the
//! part that differs (which undo entry, and whether it counts as an edit).
//!
//! Inverses (undo/redo) never run it: an inverse RESTORES, it does not re-derive.
//! Pinned by `forward_epilogue_tests.rs`, which runs every entry point and checks
//! through the test seam below that each one went through here.

use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::StationKey;

/// Which derived-state steps a forward edit needs before it is recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct EpilogueSteps {
    /// Convert loop closures the edit left as sole bridges into merges (#8). Only an
    /// edit that can REMOVE connectivity can need it, and the scan is
    /// O(closures x connectivity), so a pure addition (an import) skips it.
    pub(super) convert_orphans: bool,
    /// Renormalise onto the frame datum if the edit changed WHICH station defines it
    /// (#469 item 1), then reconcile derived geometry. Gated together, in that order:
    /// the reconcile solves the geometry that lands on the new frame.
    pub(super) reframe_and_reconcile: bool,
}

impl EpilogueSteps {
    /// Every step: a structural edit that can remove connectivity or move stations.
    pub(super) const ALL: Self = Self {
        convert_orphans: true,
        reframe_and_reconcile: true,
    };
    /// Geometry, no orphan scan: an edit that only ADDS sections, merges or closures.
    pub(super) const GEOMETRY: Self = Self {
        convert_orphans: false,
        reframe_and_reconcile: true,
    };
    /// No derived-state step: a metadata edit, or one that settles its own derived
    /// state in an order the shared steps cannot express (Repair). The record and
    /// the dirty flip still run here, in the shared order.
    pub(super) const NONE: Self = Self {
        convert_orphans: false,
        reframe_and_reconcile: false,
    };
}

#[cfg(test)]
thread_local! {
    /// Test seam: the steps of every epilogue run on this thread, oldest first.
    /// Entry points run on the calling thread, so a test that drives one and reads
    /// this sees exactly the runs it caused.
    pub(super) static EPILOGUE_RUNS: std::cell::RefCell<Vec<EpilogueSteps>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

impl AppState {
    /// Settle a successful forward edit, record it, and mark the open copy dirty.
    ///
    /// In this order, which is the point of having one function: orphan conversion,
    /// frame renormalisation, reconcile (as `steps` asks), then `record`, then the
    /// dirty flip. `record` takes the after-snapshot and pushes the undo entry - so
    /// the entry holds the settled state - and returns whether the edit counts as one
    /// for the dirty bit (`Ok(false)` for a lifecycle command or a no-op). An `Err`
    /// from it is returned as is, and the caller returns it: `record` has already
    /// rolled back whatever it needed to.
    ///
    /// Caller holds `command_lock`; `frame_before` is [`Self::frame_datum_key`] read
    /// before the mutation.
    pub(super) fn forward_epilogue(
        &self,
        frame_before: Option<StationKey>,
        steps: EpilogueSteps,
        record: impl FnOnce() -> Result<bool, CommandResult>,
    ) -> Result<(), CommandResult> {
        #[cfg(test)]
        EPILOGUE_RUNS.with(|runs| runs.borrow_mut().push(steps));
        if steps.convert_orphans {
            self.convert_orphan_closures_in_open_project();
        }
        if steps.reframe_and_reconcile {
            self.renormalize_frame_if_changed(frame_before);
            self.reconcile_geometry();
        }
        if record()? {
            // Dirty-bit flip (#191, D-B16): set-only; no-op for a local project.
            self.mark_remote_forward_edit();
        }
        Ok(())
    }

    /// [`Self::forward_epilogue`] for an entry point recorded as a snapshot pair:
    /// settle `steps`, take the after-snapshot (rolling back to `before` when it
    /// cannot be taken), push `cmd` with the pair, flip the dirty bit.
    pub(super) fn record_snapshot_edit(
        &self,
        frame_before: Option<StationKey>,
        steps: EpilogueSteps,
        cmd: Command,
        before: String,
    ) -> Result<(), CommandResult> {
        self.forward_epilogue(frame_before, steps, || {
            let after = self.capture_after_snapshot_or_rollback(&before)?;
            self.push_snapshot_undo(cmd, before, after);
            Ok(true)
        })
    }

    /// [`Self::forward_epilogue`] for an edit that is recorded only when it changed
    /// the project (the project settings, the entrance elevation, Repair): settle
    /// `steps`, then push `cmd` with the pair and flip the dirty bit when the
    /// after-snapshot differs from `before`. Returns whether it recorded. A
    /// snapshot that could not be taken records nothing, as before.
    ///
    /// Most callers pass [`EpilogueSteps::NONE`]. A settings save that changes the
    /// default north reference passes [`EpilogueSteps::GEOMETRY`] (#576 RV4-R-C1):
    /// it turns every section that declares no reference of its own about its own
    /// origin, so the stored section offsets must be re-derived before the
    /// after-snapshot is taken, or merged stations come apart in the live project
    /// and in the undo entry.
    pub(super) fn record_snapshot_edit_if_changed(
        &self,
        frame_before: Option<StationKey>,
        steps: EpilogueSteps,
        cmd: Command,
        before: Option<String>,
    ) -> bool {
        let mut changed = false;
        let _ = self.forward_epilogue(frame_before, steps, || {
            let Some(before) = before else {
                return Ok(false);
            };
            let Ok(after) = self.capture_project_snapshot() else {
                return Ok(false);
            };
            if before == after {
                return Ok(false);
            }
            self.push_snapshot_undo(cmd, before, after);
            changed = true;
            Ok(true)
        });
        changed
    }

    /// Convert any sole-bridge loop closures in the open project to merges.
    fn convert_orphan_closures_in_open_project(&self) {
        let mut guard = self.project.lock();
        let Some(project) = guard.as_mut() else {
            return;
        };
        let converted = project.convert_orphan_closures_to_merges();
        if converted > 0 {
            tracing::info!(
                converted,
                purpose = "enforce closures-are-loops invariant",
                "Converted sole-bridge loop closures to merges after a removal"
            );
        }
    }
}
