/**
 * The one-click station actions that used to skip a dialog (RS 7, U-P2-7):
 * the Station details' "Also on" detach button, the context menu's Remove
 * Closure on a station with one closure, the Closures section's remove button
 * and the warning resolution's remove. Each asks the backend's preview first
 * and opens its dialog whenever something comes back (an archived station,
 * the line's own station, a closure's hidden end) or a name would repeat, so
 * nothing the user sees changes name unasked; otherwise it acts at once, as
 * before.
 */
import { useProjectStore } from '../store/projectStore';
import { useUIStore } from '../store/uiStore';
import type { StationRef } from '../types/project';
import type { DetachLinePreview } from '../types/ipc/DetachLinePreview';
import type { RemoveClosureNaming } from '../types/ipc/RemoveClosureNaming';
import { invokeIpc } from './ipc';

/** What a one-click action did: acted, opened its dialog, or failed. */
export type AskingOutcome = 'done' | 'asked' | 'failed';

/** Whether a detach must be asked (RS 7): something comes back, other lines
 *  come off with it or keep another station, the line shows another name
 *  than a loop closure made it show, or a name would repeat. */
export function detachNeedsDialog(preview: DetachLinePreview): boolean {
  return (
    preview.restored ||
    preview.kept ||
    (preview.together ?? []).length > 0 ||
    (preview.rest_choices ?? []).length > 1 ||
    preview.shown_before !== undefined ||
    (preview.collisions ?? []).length > 0
  );
}

/** Whether a closure removal must be asked: its hidden end comes back, or a
 *  name would repeat. */
export function removalNeedsDialog(naming: RemoveClosureNaming): boolean {
  return naming.comes_back !== undefined || (naming.collisions ?? []).length > 0;
}

/**
 * Detach `partner`'s line from the station at `stationRef` (any two slots of
 * one station are a pair; the pair form detaches the partner's line), or open
 * Detach Line from Station on that line when the preview says it must ask. A
 * preview that fails detaches at once: the command reports its own refusal.
 */
export async function detachLineAsking(
  stationRef: StationRef,
  partner: StationRef,
  mergedStations: readonly StationRef[],
): Promise<AskingOutcome> {
  const preview = await Promise.resolve()
    .then(() =>
      invokeIpc<DetachLinePreview>(
        'preview_detach_line',
        { stationRef, lineId: partner.line_id },
        { purpose: 'preview detach line' },
      ),
    )
    .catch(() => null);
  if (preview && detachNeedsDialog(preview)) {
    useUIStore.getState().openDialog('unmerge-stations', {
      stationRef,
      mergedStations: [...mergedStations],
      chosenLine: partner.line_id,
    });
    return 'asked';
  }
  const result = await useProjectStore.getState().unmergeStationPair(stationRef, partner);
  return result.success ? 'done' : 'failed';
}

/**
 * Remove loop closure `closureId`, or open Remove Closure on it when the
 * preview says it must ask. `anchor` is the station the user acted on (the
 * dialog names the closure by its other end); without one, the closure's
 * first end. A preview that fails removes at once.
 */
export async function removeClosureAsking(closureId: bigint, anchor?: StationRef): Promise<AskingOutcome> {
  const naming = await Promise.resolve()
    .then(() =>
      invokeIpc<RemoveClosureNaming>(
        'preview_remove_loop_closure',
        { closureId: Number(closureId) },
        { purpose: 'preview remove loop closure' },
      ),
    )
    .catch(() => null);
  if (naming && removalNeedsDialog(naming)) {
    const closure = useProjectStore.getState().project?.loop_closures?.find((c) => c.id === closureId);
    const same = (a: StationRef, b: StationRef) => a.line_id === b.line_id && a.station_index === b.station_index;
    const anchorStation = anchor ?? closure?.station_a ?? naming.archived_slot;
    const other = closure && anchorStation
      ? same(closure.station_a, anchorStation)
        ? closure.station_b
        : closure.station_a
      : naming.archived_slot;
    if (anchorStation && other) {
      useUIStore.getState().openDialog('remove-closure', {
        anchorStation,
        closures: [{ closureId, stationRef: other }],
      });
      return 'asked';
    }
  }
  const result = await useProjectStore.getState().removeLoopClosure(closureId);
  return result.success ? 'done' : 'failed';
}
