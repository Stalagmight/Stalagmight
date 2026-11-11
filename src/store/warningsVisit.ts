/**
 * The Warnings-tab visit (see `displaySelectors.isWarningsVisitActive`): while
 * the Warnings tab is on screen the map paints Survey Issues and draws the Errors
 * labels by default. These pure helpers give the store patches for a colour or
 * Errors change made DURING a visit, and the clean-up when a visit ends. A
 * change in a visit lands only in `warningsVisitOverride`; the saved display
 * flags are never touched, so nothing is written to the project and leaving the
 * tab brings the user's own settings back.
 */
import type { ColorMode } from './colorMode';
import { isWarningsVisitActive, selectEffectiveColorMode, selectEffectiveShowShotErrors } from './displaySelectors';
import type { UIStore } from './uiStore';

type Patch = Partial<UIStore>;

/** The distance overlay's cache (origin + field) is empty outside distance
 *  mode; drop it when the visit leaves a Distance the saved flags don't have. */
function dropVisitDistance(s: UIStore, nextMode: ColorMode | undefined): Patch {
  return nextMode !== 'distance' && !s.showDistanceGradient
    ? { distanceGradientOrigin: null, distanceField: null }
    : {};
}

/** Pick a colour mode during a visit. */
export function visitColorPatch(s: UIStore, mode: ColorMode): Patch {
  return {
    warningsVisitOverride: { ...s.warningsVisitOverride, colorMode: mode },
    ...dropVisitDistance(s, mode),
  };
}

/** Toggle one colour mode during a visit (on, or back to Section). */
export function visitToggleColorPatch(s: UIStore, mode: ColorMode): Patch {
  return visitColorPatch(s, selectEffectiveColorMode(s) === mode ? 'section' : mode);
}

/** Flip the Errors labels during a visit. */
export function visitErrorsPatch(s: UIStore): Patch {
  return { warningsVisitOverride: { ...s.warningsVisitOverride, showShotErrors: !selectEffectiveShowShotErrors(s) } };
}

/** After any store change: once no visit is on, a leftover override is
 *  dropped (and with it a visit-only distance cache). Null = nothing to do. */
export function visitEndPatch(s: UIStore): Patch | null {
  if (s.warningsVisitOverride === null || isWarningsVisitActive(s)) return null;
  return { warningsVisitOverride: null, ...dropVisitDistance(s, undefined) };
}
