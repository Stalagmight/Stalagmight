/**
 * Display toggle defaults and selectors for computing effective display values.
 *
 * DISPLAY_DEFAULTS is the SINGLE SOURCE OF TRUTH for display toggle default values.
 * It is used by uiStore for both initial state and resetDisplaySettings.
 *
 * The effective value selectors compute the "effective" visibility of dependent toggles,
 * which depends on both the toggle's own state and its parent toggle's state.
 */

import type { ShotKind } from '../types/ipc/ShotKind';
import type { UIStore } from './uiStore';
import { colorModeOf, type ColorMode } from './colorMode';

/**
 * Display toggle defaults. This is the SINGLE SOURCE OF TRUTH for default values.
 * Used by uiStore for initial state and resetDisplaySettings.
 */
export const DISPLAY_DEFAULTS = {
  showStations: true,
  showShots: true,
  showSectionNames: true,
  showStationLabels: false,
  showStationNames: false,
  showShotData: false,
  showShotErrors: false,
  showReferenceShots: true,
  showJumps: true,
  showZeroDatum: true,
  showGrid: false,
  showSatellite: true,
  showDepthGradient: false,
  showStationComments: false,
  showShotComments: false,
  showDevStats: false,
  showMemoryStats: false,
  showMapStats: false,
  showZoomStats: false,
  gridSpacing: 10,
} as const;

/**
 * What the map's station labels show: nothing, each station's ID, or each
 * station's name (unnamed stations then get no label).
 */
export type StationLabelMode = 'off' | 'id' | 'name';

/**
 * The station-label mode from the two Display-menu toggles. They are mutually
 * exclusive (`toggleDisplay` clears the partner), so both being on only happens
 * through a stored pair that bypassed the menu; the IDs win then, because every
 * station has an ID and none of them goes unlabelled. The image export applies
 * the same precedence to its own two options.
 */
export function getEffectiveStationLabelMode(
  showStationLabels: boolean,
  showStationNames: boolean
): StationLabelMode {
  if (showStationLabels) return 'id';
  return showStationNames ? 'name' : 'off';
}

/**
 * Compute effective visibility of reference shots.
 * Reference shots are only visible when both showShots AND showReferenceShots are true.
 */
export function getEffectiveShowReferenceShots(showShots: boolean, showReferenceShots: boolean): boolean {
  return showShots && showReferenceShots;
}

/**
 * Compute effective visibility of jump shots (#27).
 * Same shape as the reference selector: jumps need the master shots toggle too.
 */
export function getEffectiveShowJumps(showShots: boolean, showJumps: boolean): boolean {
  return showShots && showJumps;
}

/**
 * Is a shot of this kind currently drawn on the map? (#27)
 *
 * The one predicate every "the user hid this geometry, so drop the thing
 * attached to it" effect should use. Jumps and References became independent
 * toggles, so testing `kind !== 'Normal'` against the reference toggle alone -
 * which is what the mechanical `is_reference` rewrite left behind - treats a
 * still-visible jump as hidden, and silently discards its selection, its
 * measure snap, or its contribution to a fit-bounds.
 *
 * An unresolvable shot (`undefined`, e.g. a lookup that raced a project
 * refresh) reads as Normal. That matches the backend's `ShotKind::from_u8`
 * fallback and is the conservative answer here too: "visible" means the caller
 * leaves it alone rather than throwing away user state on a transient miss.
 */
export function isShotKindVisible(
  kind: ShotKind | undefined,
  showShots: boolean,
  showJumps: boolean,
  showReferenceShots: boolean
): boolean {
  if (!showShots) return false;
  if (kind === 'Jump') return getEffectiveShowJumps(showShots, showJumps);
  if (kind === 'Reference') return getEffectiveShowReferenceShots(showShots, showReferenceShots);
  return true;
}

/**
 * Compute effective visibility of shot data labels.
 * Shot data is only visible when both showShots AND showShotData are true.
 */
export function getEffectiveShowShotData(showShots: boolean, showShotData: boolean): boolean {
  return showShots && showShotData;
}

/**
 * Compute effective visibility of station comments.
 * Station comments are visible when showStationComments is true, regardless of station visibility.
 */
export function getEffectiveShowStationComments(_showStations: boolean, showStationComments: boolean): boolean {
  return showStationComments;
}

/**
 * Compute effective visibility of shot comments.
 * Shot comments are only visible when both showShots AND showShotComments are true.
 */
export function getEffectiveShowShotComments(showShots: boolean, showShotComments: boolean): boolean {
  return showShots && showShotComments;
}

// ─── Survey Issues: the Warnings-tab visit ─────────────────────────────────

/**
 * What the user changed during the current Warnings-tab visit. While the tab is
 * on screen the graded Survey Issues colouring and the Errors labels are on by
 * default; a change the user makes in that time is kept HERE (never in the saved
 * flags, never written to disk) and dropped when the visit ends, so leaving the
 * tab brings back the user's own settings. Unset field = the visit default.
 */
export interface WarningsVisitOverride {
  colorMode?: ColorMode;
  showShotErrors?: boolean;
}

type VisitState = Pick<
  UIStore,
  'warningsAutoDisplay' | 'view' | 'appMode' | 'detailPanelOpen' | 'detailPanelTab' | 'dataTableFullScreen'
>;

/**
 * Whether a Warnings-tab visit is on: the Warnings tab is the visible tab of the
 * open detail panel in the Survey workspace, and the "Highlight warnings on tab
 * switch" setting is on. Plan and Export show a different panel there, and a
 * full-screen data table covers the map, so neither counts.
 */
export function isWarningsVisitActive(s: VisitState): boolean {
  return (
    s.warningsAutoDisplay &&
    s.view === 'single' &&
    s.appMode === 'survey' &&
    s.detailPanelOpen &&
    s.detailPanelTab === 'warnings' &&
    !s.dataTableFullScreen
  );
}

type IssueDisplayState = VisitState &
  Pick<UIStore, 'showIssueSeverity' | 'showDistanceGradient' | 'showDepthGradient' | 'showShotErrors' | 'warningsVisitOverride'>;

/** The colour mode the map actually paints: the saved one, or during a
 *  Warnings-tab visit Survey Issues unless the user picked another. */
export function selectEffectiveColorMode(s: IssueDisplayState): ColorMode {
  if (!isWarningsVisitActive(s)) return colorModeOf(s);
  return s.warningsVisitOverride?.colorMode ?? 'issues';
}

/** Whether the Errors labels are on: the saved toggle, or during a
 *  Warnings-tab visit on unless the user turned them off. */
export function selectEffectiveShowShotErrors(s: IssueDisplayState): boolean {
  if (!isWarningsVisitActive(s)) return s.showShotErrors;
  return s.warningsVisitOverride?.showShotErrors ?? true;
}

/** Whether the distance gradient is the painted colour mode. Every reader of
 *  the distance overlay goes through this, so a Distance picked during a visit
 *  behaves exactly like a saved one. */
export function selectDistanceOverlayActive(s: IssueDisplayState): boolean {
  return selectEffectiveColorMode(s) === 'distance';
}

/** For the Display menu: is this pill on only because of the visit default? */
export function selectIssueDisplayAuto(s: IssueDisplayState): { visit: boolean; color: boolean; errors: boolean } {
  const visit = isWarningsVisitActive(s);
  return {
    visit,
    color: visit && s.warningsVisitOverride?.colorMode === undefined,
    errors: visit && s.warningsVisitOverride?.showShotErrors === undefined,
  };
}
