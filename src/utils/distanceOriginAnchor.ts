/**
 * The distance-gradient origin follows its physical station across survey edits
 * (#576 D39(g), "Follow the station").
 *
 * The origin is a positional `StationRef`, and an insert, a delete or a reverse
 * before it renumbers its section silently: the index stays in range and simply
 * starts naming a different station, so the ramp, legend, origin ring and the
 * DetailPanel badge re-root on a neighbour with no error. The gas-plan pins solved
 * the same problem with backend anchors (#310) - a station described by the shots
 * either side of it - and the origin rides the same seam: `captureDistanceOrigin`
 * takes an anchor before the edit, next to `flushPlanAnchors`, and
 * `remapDistanceOrigin` re-resolves it after, next to `remapPlanPins`. That covers
 * a reverse too (an anchor survives one by construction), which is why
 * `applyReverseAftermath` no longer mirrors an anchored origin - doing both would
 * move it twice.
 *
 * Only while a station is picked: the entrance origin (`null`) costs no round trip.
 * A failed re-resolve leaves the origin where it is (positional, as before #576),
 * the same degradation the plan pins accept. A failed CAPTURE is reported as such
 * (`anchored: false`): nothing re-resolves that origin, so a reverse - which the
 * origin followed by a mirror before the anchors - still mirrors it in
 * `applyReverseAftermath`; any other edit leaves it positional. One path or the
 * other moves it, never both. A station that no longer exists falls back to the
 * entrance, with a notice.
 *
 * ...and comes back if an undo or redo restores it (#576 D39(g)-UNDO, decided: "Undo
 * restoring the deleted origin station brings the origin back"). The fallback keeps
 * the anchor it was following, dormant, for the project it was taken in; the
 * undo/redo seam re-resolves it while the origin is still on the entrance, and puts
 * the origin back once it names a station again. Anything that replaces the choice it
 * stood for drops it (#576 RV4-FE-FEA2): a pick of another origin; turning the
 * distance overlay off (which clears the origin so re-enabling starts at the
 * entrance - Plan/Export's suppression is not that: it restores the overlay on
 * return); opening a project, another one or the same one again; and an in-place
 * SpeleoDB tree swap. The last two call {@link dropDormantDistanceOrigin}; the origin
 * is already the entrance then, so no change of it could tell.
 */
import type { StationRef } from '../types/project';
import { useUIStore } from '../store/uiStore';
import { planAnchorsForRefs, planRefsForAnchors, type PlanAnchor } from './divePlanApi';
import { TOAST_KEYS } from './toastKeys';
import i18n from '../i18n';

/** An anchor for the origin, with the ref it was taken against (`ref: null` for the dormant
 *  anchor of an origin a delete displaced, re-resolved while the origin is the entrance); or a
 *  picked origin that could not be anchored (`anchored: false`: the IPC failed, or the backend
 *  had no anchor for it), which only a reverse re-points, by mirroring (`applyReverseAftermath`).
 *  `anchored` is the discriminant because `PlanAnchor` is opaque (`unknown`). */
export type OriginCapture =
  | { anchored: true; ref: StationRef | null; anchor: PlanAnchor }
  | { anchored: false; ref: StationRef };

/** The anchor of the origin the last delete displaced, and the project (`created_at`) it
 *  belongs to; null when there is none. */
let dormant: { anchor: PlanAnchor; projectKey: string | null } | null = null;
let stopWatchingOrigin: (() => void) | null = null;

function setDormant(next: typeof dormant): void {
  stopWatchingOrigin?.();
  stopWatchingOrigin = null;
  dormant = next;
  if (next === null) return;
  stopWatchingOrigin = useUIStore.subscribe((state, prev) => {
    // Any change of origin after the fallback is the user's: drop it.
    if (state.distanceGradientOrigin !== prev.distanceGradientOrigin) setDormant(null);
    // So is turning the overlay off. Entering Plan/Export turns it off in the same
    // update that changes the mode, and returning restores it: not the user's.
    else if (prev.showDistanceGradient && !state.showDistanceGradient && state.appMode === prev.appMode) {
      setDormant(null);
    }
  });
}

/**
 * Forget the anchor a delete displaced: a project was opened (another one, or the same
 * one again) or the open tree was swapped in place, so an undo that brings its station
 * back no longer restores the user's choice (#576 RV4-FE-FEA2).
 */
export function dropDormantDistanceOrigin(): void {
  setDormant(null);
}

const sameRef = (a: StationRef | null, b: StationRef | null): boolean =>
  a === b || (a !== null && b !== null && a.section_id === b.section_id && a.station_index === b.station_index);

/** Anchor the picked origin against the survey as it stands BEFORE an edit; null when there is
 *  nothing to anchor (the entrance origin), and `anchored: false` when it cannot be anchored.
 *
 *  `dormantFor`: the undo/redo seam passes the open project's key, and while the origin is
 *  the entrance the anchor a delete displaced it from (taken in that project) is returned
 *  for re-resolving instead. A forward edit cannot bring back a deleted station, so it
 *  passes nothing and costs no round trip. */
export async function captureDistanceOrigin(dormantFor?: string | null): Promise<OriginCapture | null> {
  const ref = useUIStore.getState().distanceGradientOrigin;
  if (ref === null) {
    return dormant !== null && dormantFor !== undefined && dormant.projectKey === dormantFor
      ? { anchored: true, ref: null, anchor: dormant.anchor }
      : null;
  }
  try {
    const taken = await planAnchorsForRefs([ref]);
    const anchor = Array.isArray(taken) ? taken[0] : null;
    return anchor === null || anchor === undefined ? { anchored: false, ref } : { anchored: true, ref, anchor };
  } catch (e) {
    console.debug('[distance-origin] anchor capture failed', e);
    return { anchored: false, ref };
  }
}

/** Re-resolve a captured origin against the survey as it stands AFTER the edit; `projectKey`
 *  names the project the edit ran in, which a displaced origin's dormant anchor belongs to. */
export async function remapDistanceOrigin(
  capture: OriginCapture | null,
  projectKey: string | null = null
): Promise<void> {
  // No anchor, nothing to re-resolve: a reverse mirrors that origin instead (applyReverseAftermath).
  if (capture === null || !capture.anchored) return;
  let resolved: StationRef | null | undefined;
  try {
    const refs = await planRefsForAnchors([capture.anchor]);
    resolved = Array.isArray(refs) ? refs[0] : undefined;
  } catch (e) {
    console.debug('[distance-origin] re-resolve failed', e);
    return;
  }
  if (resolved === undefined) return;
  const ui = useUIStore.getState();
  // A pick (or a project switch, which clears the origin) during the round trip wins.
  if (!sameRef(ui.distanceGradientOrigin, capture.ref)) return;
  if (resolved === null) {
    // A dormant anchor that still names nothing: keep waiting, quietly.
    if (capture.ref === null) return;
    ui.setDistanceGradientOrigin(null);
    ui.showToast(i18n.t('map.distanceLegend.originRemoved'), TOAST_KEYS.DISTANCE_ORIGIN_RESET, 'info');
    setDormant({ anchor: capture.anchor, projectKey });
    return;
  }
  // Unchanged: leave the cached field alone (the setter clears it).
  if (!sameRef(resolved, capture.ref)) ui.setDistanceGradientOrigin(resolved);
}
