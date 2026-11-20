use super::AppState;
use crate::core::{Command, CommandResult};
use crate::models::{
    AzimuthReference, ClosureStrategy, Coordinates, LineData, LineId, Project, SectionId, ShotData,
    ShotKind, StationRef, FEATURE_ID_STRIDE, MAX_LINES_IN_ID_SPACE, MAX_LINE_ID,
};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// How a freshly-imported file's virtual shots should be classified (#27 q3).
///
/// A parser can only tell that a shot is virtual; which of the two virtual kinds
/// it is needs the project graph, which only exists once the import is placed
/// and merged. [`Auto`](VirtualShotMode::Auto) runs the chain rule at that
/// point. The two explicit modes let a user who knows the file override it in
/// one control, instead of the per-shot picker the issue originally proposed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum VirtualShotMode {
    /// Chain rule: a virtual run bounded by real survey on both ends is a Jump,
    /// anything else a Reference.
    #[default]
    Auto,
    /// Every virtual shot in this file is a swum shortcut.
    AllJumps,
    /// Every virtual shot in this file is a non-swimmable survey tie.
    AllReferences,
}
use std::sync::Mutex;

/// Which per-station text map an import config entry lands in.
#[derive(Clone, Copy)]
enum StationText {
    Comment,
    Name,
}

/// Tally closure-creation results from a `create_loop_closures_batch` call,
/// counting successes (and failures) directly from `results` rather than
/// deriving `created` by subtracting `failed` from the requested count.
///
/// That distinction matters for one specific case: when the project lock is
/// unavailable at the point `create_loop_closures_batch` would run, the
/// caller passes an empty `results` (it never ran the batch), so
/// `closure_pairs.iter().zip(results)` yields nothing at all - not one
/// success, not one failure. A subtraction-based count (`requested - skipped
/// - failed`) would then silently report every one of those pairs as
/// created, when in fact none were. Counting `Ok` arms directly reports the
/// correct zero.
fn tally_closure_results(
    closure_pairs: &[(StationRef, StationRef)],
    results: Vec<Result<crate::models::LoopClosure, String>>,
) -> (u32, u32) {
    let mut created = 0u32;
    let mut failed = 0u32;
    for ((station_a, station_b), result) in closure_pairs.iter().zip(results) {
        match result {
            Ok(lc) => {
                created += 1;
                tracing::debug!(
                    closure_id = lc.id,
                    ?station_a,
                    ?station_b,
                    purpose = "import loop closures",
                    "Created loop closure from TML import"
                );
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    ?station_a,
                    ?station_b,
                    error = %e,
                    purpose = "import loop closures",
                    "Failed to create loop closure from TML import, skipping"
                );
            }
        }
    }
    (created, failed)
}

/// RAII guard for [`Project::cascade_deferred`].
///
/// `execute_import_lines` flips the flag on so each per-line
/// `merge_stations` skips its O(N) cascade, then runs ONE cascade at the
/// end.  If the apply loop panics (poisoning the mutex) or any code path
/// returns early without clearing the flag, every subsequent command
/// silently skips its cascade and accumulates line-offset drift.  The
/// guard's `Drop` resets the flag to `false` even through a poisoned
/// mutex, closing that window.
///
/// Call [`Self::disarm`] before the final cascade so the cascade is
/// actually allowed to run (the cascade itself early-exits when the flag
/// is still `true`).  After `disarm`, `Drop` is a no-op.
struct CascadeDeferGuard<'a> {
    project: &'a Mutex<Option<Project>>,
    armed: bool,
}

impl<'a> CascadeDeferGuard<'a> {
    /// Set `cascade_deferred = true` on the project (if present) and
    /// return a guard.  If the lock is already poisoned the flag is set
    /// anyway via `into_inner` - the import is about to run regardless,
    /// and we still want the Drop reset to fire.
    fn arm(project: &'a Mutex<Option<Project>>) -> Self {
        Self::set_flag(project, true);
        Self {
            project,
            armed: true,
        }
    }

    /// Clear the flag now (allowing the final cascade to run) and skip
    /// the Drop reset.
    fn disarm(mut self) {
        Self::set_flag(self.project, false);
        self.armed = false;
    }

    fn set_flag(project: &Mutex<Option<Project>>, value: bool) {
        let mut guard = match project.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if let Some(p) = guard.as_mut() {
            p.cascade_deferred = value;
        }
    }
}

impl<'a> Drop for CascadeDeferGuard<'a> {
    fn drop(&mut self) {
        if self.armed {
            // Reset on the panic / early-return path.  `set_flag` tolerates
            // a poisoned mutex so the flag is cleared even if the apply
            // loop panicked while holding the lock.
            Self::set_flag(self.project, false);
        }
    }
}

impl AppState {
    /// Rename the shot UUIDs of `new_lines` that another line of the open
    /// project already holds ([`crate::models::Project::reassign_repeated_shot_uuids`]):
    /// the project keeps its ids, the newly created lines yield.
    pub(super) fn reassign_repeated_shot_uuids(&self, new_lines: &[LineId]) {
        let _ = (&new_lines,);
        crate::core::stub::not_ported("command_import::reassign_repeated_shot_uuids")
    }

    /// Execute a create-line-and-merge operation as a single atomic undo/redo unit.
    /// This is used when importing a new line and immediately merging one of its stations
    /// with an existing station. Both operations are combined into a single undo entry.
    pub fn execute_create_and_merge(
        &self,
        name: Option<String>,
        surveyor: Option<String>,
        date: Option<chrono::NaiveDate>,
        color: String,
        direction: Option<crate::models::SurveyDirection>,
        shots: Vec<ShotData>,
        import_station_index: u32,
        target_station: StationRef,
        closure_strategy: ClosureStrategy,
    ) -> CommandResult {
        self.execute_create_and_merge_in_cave_section(
            name,
            surveyor,
            date,
            color,
            direction,
            shots,
            import_station_index,
            target_station,
            closure_strategy,
            None,
        )
    }

    /// [`Self::execute_create_and_merge`] that also puts the new line in the
    /// Section `cave_section_id` (#327), inside the same undo step: a New
    /// Line placed by snapping onto a station is ONE undo, Section included.
    /// A Section that no longer exists refuses the whole step before anything
    /// changes, as `CreateLine` does (`create_line_command`).
    pub fn execute_create_and_merge_in_cave_section(
        &self,
        name: Option<String>,
        surveyor: Option<String>,
        date: Option<chrono::NaiveDate>,
        color: String,
        direction: Option<crate::models::SurveyDirection>,
        shots: Vec<ShotData>,
        import_station_index: u32,
        target_station: StationRef,
        _closure_strategy: ClosureStrategy,
        cave_section_id: Option<SectionId>,
    ) -> CommandResult {
        let _ = (&name, &surveyor, &date, &color, &direction, &shots, &import_station_index, &target_station, &cave_section_id);
        crate::core::stub::not_ported("command_import::execute_create_and_merge_in_cave_section")
    }

    /// Execute a batch import of lines as a single atomic undo/redo unit.
    /// Creates all lines with their placements (merges, zero datums, moves)
    /// and optionally tracks the file hash. Uses `ProjectSnapshotPair` so a
    /// single undo reverts everything including the hash.
    ///
    /// `mode` picks how this file's virtual shots are split into Jump vs
    /// Reference (#27). The pass runs inside this command, before the `after`
    /// snapshot: doing it afterwards (the first cut) left the classified kinds
    /// out of the redo state, so undo-then-redo of an import silently reverted
    /// every shot to the parser's conservative `Reference` seed and dropped the
    /// far side of each jump out of gas and penetration.
    pub fn execute_import_lines(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            None,
        )
    }

    /// Same as [`Self::execute_import_lines`], but also merges an Ariane
    /// file's project-level entrance elevation (#431) into `Project::surface_elevation`
    /// atomically with the import, so undo/redo covers both. The only caller
    /// that has this value is the Ariane TML/TMLU importer.
    pub fn execute_tml_import_sections(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        self.execute_import_lines_inner(
            lines,
            closures,
            file_hash,
            line_hashes,
            existing_shot_updates,
            mode,
            surface_elevation,
        )
    }

    fn execute_import_lines_inner(
        &self,
        lines: Vec<crate::models::ImportLineConfig>,
        closures: Vec<crate::models::ImportClosure>,
        file_hash: Option<String>,
        line_hashes: Vec<String>,
        existing_shot_updates: Vec<crate::models::ExistingShotUpdate>,
        mode: VirtualShotMode,
        surface_elevation: Option<f64>,
    ) -> CommandResult {
        let _ = (&lines, &closures, &file_hash, &line_hashes, &existing_shot_updates, &mode, &surface_elevation);
        crate::core::stub::not_ported("command_import::execute_import_lines_inner")
    }
}

#[cfg(test)]
mod tally_closure_results_tests {
    use super::tally_closure_results;
    use crate::models::StationRef;

    /// The empty-`results` branch: `create_loop_closures_batch` never ran (the
    /// project-lock-unavailable arm returns `Vec::new()`), so `results` is
    /// empty even though `closure_pairs` is not. Must report zero created and
    /// zero failed, never derive "all created" from a stale subtraction.
    #[test]
    fn empty_results_reports_zero_created_and_zero_failed() {
        let pairs = vec![
            (StationRef::new(1, 0), StationRef::new(2, 0)),
            (StationRef::new(3, 0), StationRef::new(4, 0)),
        ];
        let (created, failed) = tally_closure_results(&pairs, Vec::new());
        assert_eq!(created, 0);
        assert_eq!(failed, 0);
    }
}
