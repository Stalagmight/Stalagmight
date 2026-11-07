/**
 * The map-side guards of a move carry (GHOST-2, #576; docs/move-tools.md,
 * "Integrity while carrying"). The registry and the mutation-chokepoint half live
 * in `src/store/moveCarry.ts`; these are the ways the carry is invalidated without
 * any mutation being issued:
 *
 * - R2: the measured survey changed underneath the ghost (`projectTreeGeneration`
 *   moved off the one it was picked up against - a refresh after a mutation that
 *   was already in flight, a SpeleoDB tree replacement, load/create/close, an
 *   import, a config refetch, the save-path heal), or editing became blocked
 *   (Plan/Export, Closures Computed View, a lost SpeleoDB lock).
 * - R4: the active tool left the three move tools, or placement mode (DMP /
 *   new-section placement) began, so one click can never both drop the ghost and
 *   run another tool's action. Switching AMONG the move tools keeps the carry.
 *
 * A project close or switch also cancels the carry, silently: it is the R2 case
 * where the survey is not edited but replaced, so it is told apart by the
 * project's identity (`created_at`, the key `projectSwitched` uses) before the
 * generation check could report it as a change.
 *
 * Each cancels through the carry's registered cancel routine, the same one Escape
 * runs. The carry's OWN commit cannot trip R2: the commit ends the carry before
 * its refresh bumps the generation.
 *
 * Store subscriptions rather than render-time effects: they run inside the very
 * `set` that changed the state, so there is no frame in which a click could still
 * drop a ghost that change has already invalidated.
 */
import { useEffect, type MutableRefObject } from 'react';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore, type ToolType } from '../../store/uiStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { cancelMoveCarry, endMoveCarry, isMoveCarryInFlight, moveCarryGeneration } from '../../store/moveCarry';

const MOVE_TOOLS: ReadonlySet<ToolType> = new Set<ToolType>(['move_station', 'move_shot', 'move_segment']);

export function useMoveCarryGuards(carryTokenRef: MutableRefObject<number | null>): void {
  useEffect(() => {
    const cancelIfEditingBlocked = () => {
      if (isMoveCarryInFlight() && isEditingBlockedNow()) cancelMoveCarry('editingBlocked');
    };
    const unsubscribeProject = useProjectStore.subscribe((state, prev) => {
      const pickedUpOn = moveCarryGeneration();
      if (pickedUpOn === null) return;
      if ((state.project?.created_at ?? null) !== (prev.project?.created_at ?? null)) {
        cancelMoveCarry('projectSwitched');
      } else if (state.projectTreeGeneration !== pickedUpOn) {
        cancelMoveCarry('surveyChanged');
      }
    });
    // Read-only is checked before the tool: entering Plan/Export also drops a move
    // tool back to select (MapCanvas's isMutatingTool effect), and the user should
    // hear why the move went away, which the silent tool-switch cancel would not say.
    const unsubscribeUI = useUIStore.subscribe((state, prev) => {
      cancelIfEditingBlocked();
      if (state.activeTool !== prev.activeTool && !MOVE_TOOLS.has(state.activeTool)) cancelMoveCarry('toolSwitch');
      if (state.placementMode !== null && prev.placementMode === null) cancelMoveCarry('placementMode');
    });
    const unsubscribeSpeleo = useSpeleoStore.subscribe(cancelIfEditingBlocked);
    return () => {
      unsubscribeProject();
      unsubscribeUI();
      unsubscribeSpeleo();
      // The map is going away with the ghost; nothing is left to clean up but the registration.
      endMoveCarry(carryTokenRef.current);
    };
  }, [carryTokenRef]);
}
