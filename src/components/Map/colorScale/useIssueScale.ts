/**
 * The Survey Issues colour scale the map draws with: the per-error levels
 * (session, `colorScaleStore`), the switched-off errors and combine mode
 * (config `[map_palette]`), the project's warning thresholds the default levels
 * sit on, and the palette.
 *
 * MapCanvas reads only the committed values (through `useIssueRenderInput`); a
 * levels drag or a palette hover reaches the survey layer by subscription
 * instead ({@link useSurveyLayerIssuePreview}), so a pointer move does not
 * re-render the map component.
 */
import { useEffect, useMemo } from 'react';
import { useColorScaleStore, type ColorScaleState } from '../../../store/colorScaleStore';
import { currentMapPalette, useHiddenIssues, useIssueCombine, usePersistedPalette } from '../../../store/colorScalePalette';
import { useProjectStore, type ProjectStore } from '../../../store/projectStore';
import { DEFAULT_VALIDATION_CONFIG } from '../../../utils/validation';
import { getMapThemeColors } from '../../../utils/themeColors';
import { resolveChoice, THEME_PALETTE_ID, type PaletteChoice } from '../../../utils/palettes';
import type { ResolvedPalette } from '../../../utils/palettes/sample';
import {
  issueScaleFrom,
  withMetricLevels,
  type IssueScale,
  type IssueThresholds,
} from '../surveyLayer/issueScale';

/** The project's warning thresholds (the project's own, else the config's). */
export function issueThresholdsOf(s: Pick<ProjectStore, 'project' | 'config'>): IssueThresholds {
  const v = s.project?.validation ?? s.config?.validation ?? DEFAULT_VALIDATION_CONFIG;
  return {
    azimuthDiff: v.azimuth_diff_threshold,
    pitchDiff: v.pitch_diff_threshold,
    inclinationDepthDiff: v.inclination_depth_diff_threshold,
  };
}

/** The resolved Survey Issues palette, or null for the untouched default (so
 *  the map keeps its original green -> yellow -> red code path). */
export function issuePaletteFor(choice: PaletteChoice): ResolvedPalette | null {
  if (choice.id === THEME_PALETTE_ID && !choice.reversed) return null;
  return resolveChoice(choice, 'issues', getMapThemeColors());
}

/** The project thresholds as a stable object (hook). */
export function useIssueThresholds(): IssueThresholds {
  const project = useProjectStore((s) => s.project?.validation);
  const config = useProjectStore((s) => s.config?.validation);
  const v = project ?? config ?? DEFAULT_VALIDATION_CONFIG;
  const { azimuth_diff_threshold: az, pitch_diff_threshold: pitch, inclination_depth_diff_threshold: incl } = v;
  return useMemo(() => ({ azimuthDiff: az, pitchDiff: pitch, inclinationDepthDiff: incl }), [az, pitch, incl]);
}

/** The committed Survey Issues scale (hook). */
export function useIssueScale(): IssueScale {
  const thresholds = useIssueThresholds();
  const overrides = useColorScaleStore((s) => s.issueLevels);
  const hidden = useHiddenIssues();
  const combine = useIssueCombine();
  return useMemo(
    () => issueScaleFrom(thresholds, overrides, hidden, combine),
    [thresholds, overrides, hidden, combine],
  );
}

/** The committed Survey Issues palette, resolved (hook); null = default. */
export function useIssuePalette(): ResolvedPalette | null {
  const choice = usePersistedPalette('issues');
  return useMemo(() => issuePaletteFor(choice), [choice]);
}

/** What the survey layer previews while a levels drag or a palette hover is
 *  in progress. */
export interface IssueScalePreview {
  scale: IssueScale;
  palette: ResolvedPalette | null;
}

/** The preview for the current store state, or null when nothing is live. */
export function currentIssuePreview(cs: Pick<ColorScaleState, 'issuePreview' | 'issueLevels' | 'palettePreview'>): IssueScalePreview | null {
  const palettePreview = cs.palettePreview?.kind === 'issues' ? cs.palettePreview.choice : null;
  if (!cs.issuePreview && !palettePreview) return null;
  const mp = currentMapPalette();
  const committed = issueScaleFrom(issueThresholdsOf(useProjectStore.getState()), cs.issueLevels, mp.issues_hidden, mp.issues_combine);
  return {
    scale: cs.issuePreview ? withMetricLevels(committed, cs.issuePreview.id, cs.issuePreview.levels) : committed,
    palette: issuePaletteFor(palettePreview ?? mp.issues),
  };
}

/** The SurveyLayer surface the preview driver needs. */
export interface IssuePreviewTarget {
  previewIssueScale(preview: IssueScalePreview | null): void;
}

/** Drive a SurveyLayer's Survey Issues preview from the store, outside React. */
export function useSurveyLayerIssuePreview(
  layerRef: { readonly current: IssuePreviewTarget | null },
  mapLoaded: boolean,
): void {
  useEffect(() => {
    if (!mapLoaded) return undefined;
    const apply = (): void => {
      layerRef.current?.previewIssueScale(currentIssuePreview(useColorScaleStore.getState()));
    };
    apply();
    return useColorScaleStore.subscribe(apply);
  }, [layerRef, mapLoaded]);
}
