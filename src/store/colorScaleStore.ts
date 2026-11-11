import { create } from 'zustand';
import type { ColorScaleKind, LevelsScale, PaletteChoice, PaletteKind } from '../utils/palettes/types';
import type { IssueLevelOverrides, IssueLevels, IssueMetricId } from '../components/Map/surveyLayer/issueScale';

/**
 * Session-only colour-scale state for depth / distance colouring: the Levels
 * ranges (min / gamma / max) for the map and for the image export, and the
 * live preview while the user browses palettes or drags a Levels handle. Also
 * the Survey Issues per-error levels (min / warn / max) and their live drag.
 * Those only change what the map draws: the warnings and their Project
 * Settings thresholds are the backend validator's and never read this.
 *
 * The palette CHOICE is not here: it persists across projects in config.toml
 * (`[map_palette]`, read from `useProjectStore().config`) and is written by
 * `colorScalePalette.ts`. Nothing in this store is persisted anywhere - not in
 * config, not in the .stalx (never through `save_display_setting`), and so
 * never in what is uploaded to SpeleoDB.
 *
 * ## Per-project, so it resets on a project transition
 *
 * A range picked for one survey means nothing for the next, so `resetSession`
 * runs from `uiStore.applyDisplaySettings`, the load / create funnel (beside
 * the export and gas-plan store resets). Closing needs no reset: the next open
 * goes through that funnel. Not reset by `speleoTreeReplacement.ts`, which swaps
 * the tree of the project already open - same asymmetry as `exportStore`.
 *
 * Within a session a range survives Plan / Export round-trips, colour-mode
 * toggles, geometry-mode changes and a new distance origin.
 */

/** `null` = auto: the data's own extent, linear. */
export type ScaleByKind = Readonly<Record<ColorScaleKind, LevelsScale | null>>;

const AUTO: ScaleByKind = { depth: null, distance: null };

export interface ColorScaleState {
  /** Map Levels range per kind. */
  mapScale: ScaleByKind;
  /** Image-export Levels range per kind (its own: export distance is always
   *  measured from the entrance, so its domain differs from the map's). */
  exportScale: ScaleByKind;
  /** A palette the picker is hovering / arrowing over, drawn instead of the
   *  persisted choice until the picker commits or reverts. */
  palettePreview: { kind: PaletteKind; choice: PaletteChoice } | null;
  /** True while a Levels handle is being dragged: the map recolours through
   *  its fast preview path and rebuilds exactly when this drops back. */
  dragging: boolean;
  /** Survey Issues levels the user set, by error (absent = the default: warn
   *  on the project's threshold, max at three times it). */
  issueLevels: IssueLevelOverrides;
  /** The error whose levels handle is mid-drag, and its levels so far. The map
   *  previews them; `issueLevels` takes them on release. */
  issuePreview: { id: IssueMetricId; levels: IssueLevels } | null;

  setMapScale: (kind: ColorScaleKind, scale: LevelsScale | null) => void;
  setExportScale: (kind: ColorScaleKind, scale: LevelsScale | null) => void;
  setPalettePreview: (preview: { kind: PaletteKind; choice: PaletteChoice } | null) => void;
  setDragging: (dragging: boolean) => void;
  /** One error's levels: `live` while a handle is mid-drag (preview only);
   *  `null` restores the default. */
  setIssueLevels: (id: IssueMetricId, levels: IssueLevels | null, live: boolean) => void;
  /** Back to auto ranges and no preview (project transition). */
  resetSession: () => void;
}

export const useColorScaleStore = create<ColorScaleState>((set) => ({
  mapScale: AUTO,
  exportScale: AUTO,
  palettePreview: null,
  dragging: false,
  issueLevels: {},
  issuePreview: null,

  setMapScale: (kind, scale) => set((s) => ({ mapScale: { ...s.mapScale, [kind]: scale } })),
  setExportScale: (kind, scale) => set((s) => ({ exportScale: { ...s.exportScale, [kind]: scale } })),
  setPalettePreview: (palettePreview) => set({ palettePreview }),
  setDragging: (dragging) => set({ dragging }),
  setIssueLevels: (id, levels, live) =>
    set((s) => {
      if (live && levels) return { issuePreview: { id, levels } };
      const { [id]: _dropped, ...rest } = s.issueLevels;
      return { issueLevels: levels ? { ...rest, [id]: levels } : rest, issuePreview: null };
    }),
  resetSession: () =>
    set({ mapScale: AUTO, exportScale: AUTO, palettePreview: null, dragging: false, issueLevels: {}, issuePreview: null }),
}));

/** Whether the map should take its fast depth / distance preview recolour
 *  right now. A Survey Issues preview is the issues driver's
 *  (`colorScale/useIssueScale.ts`), not this one's. */
export function isColorScaleLive(s: Pick<ColorScaleState, 'palettePreview' | 'dragging'>): boolean {
  return s.dragging || (s.palettePreview !== null && s.palettePreview.kind !== 'issues');
}
