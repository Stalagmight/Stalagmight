import { create } from 'zustand';
import type { ExportImageOptions } from '../types/commands';

export type ExportType = 'data' | 'image';
export type DataFormat = 'csv' | 'json' | 'tml';

/**
 * Default image-export options. Mirrors the backend `ExportImageOptions::default`
 * (PNG / 300 DPI / threshold 2 / labels on) with colors off - coloring is
 * opt-in and raster-only (SVG stays monochrome). Station ID and name labels are opt-in too.
 */
export const DEFAULT_IMAGE_OPTIONS: ExportImageOptions = {
  format: 'png',
  dpi: 300,
  branching_threshold: 2,
  show_penetration_labels: true,
  color_mode: 'none',
  // `depth_shallow` / `depth_deep` are deliberately absent: they are optional on
  // the generated type and are stamped per call by `withThemeDepthRamp`, which
  // reads the live theme tokens. A default here would be a second source of
  // truth for the ramp and would be the stale one.
  show_station_labels: false,
  // Station names instead of IDs; exclusive with the IDs (setStationLabelOption).
  show_station_names: false,
  show_section_names: false,
  show_station_comments: false,
  show_shot_comments: false,
  show_shot_data: false,
};

/**
 * Shared Export workspace UI state. Lets the `ExportControls` in the Export
 * workspace panel and the large WYSIWYG preview in the map area read/write the
 * same options. (The retired modal Export dialog kept its own local state and
 * opened on data/CSV; the workspace is the only export surface now.)
 *
 * Defaults to the image type because the Export workspace is map-centric:
 * entering it should immediately show the map preview. A data export is one
 * click away (the "Survey Data" type, CSV preselected).
 *
 * ## It is per-PROJECT state, and so it resets on a project transition
 *
 * This store outlives the project the options were chosen for: nothing in
 * `loadProject`, `createProject` or `closeProject` used to touch it, and
 * `setAppMode` explicitly does not either. So a 600-DPI PNG framed for one
 * survey stayed selected when the next one opened, and the Export-mode preview
 * rendered a different cave at settings nobody chose for it. `reset` is called
 * from `uiStore.applyDisplaySettings` (the load/create funnel, beside
 * `useGasPlanStore.getState().reset()` and for the same reason) and from
 * `projectStore`'s `clearOpenProjectState` (close and delete).
 *
 * NOT from `speleoTreeReplacement.ts`, the third transition funnel, and the
 * asymmetry with the gas store there is deliberate: that path swaps the survey
 * tree of the project that is already open, and a gas plan has to reset because
 * it points at stations that no longer exist. An export option points at
 * nothing - it is a rendering choice about whatever is open - so resetting it
 * there would discard the user's framing for no reason.
 */
interface ExportState {
  exportType: ExportType;
  dataFormat: DataFormat;
  imageOptions: ExportImageOptions;
  /**
   * An export is being written (set by `ExportControls` around the export
   * call, cleared when it settles - success or failure). While it is on, the
   * Export workspace can't be closed: `closeExport` refuses, and every close
   * control is disabled with "Export in progress". Not a per-project option,
   * so `reset` leaves it alone: only the run that set it clears it.
   */
  exporting: boolean;
  setExportType: (t: ExportType) => void;
  setDataFormat: (f: DataFormat) => void;
  setImageOptions: (o: ExportImageOptions) => void;
  setExporting: (on: boolean) => void;
  /** Back to the defaults above. Called on every project transition - see below. */
  reset: () => void;
}

/**
 * The value slice, as a factory rather than a frozen object.
 *
 * `imageOptions` is replaced wholesale by `setImageOptions`, never mutated, so a
 * shared constant would be safe today - but `reset` is the one caller that
 * exists to un-do a user's edits, and having it hand back a fresh object is the
 * difference between a rule and a coincidence of the setters' current shape.
 * Mirrors `gasPlanStore.ts`'s `initialState()`, which `reset` there also spreads.
 */
const initialState = (): Pick<ExportState, 'exportType' | 'dataFormat' | 'imageOptions'> => ({
  exportType: 'image',
  dataFormat: 'csv',
  imageOptions: { ...DEFAULT_IMAGE_OPTIONS },
});

export const useExportStore = create<ExportState>((set) => ({
  ...initialState(),
  exporting: false,
  setExporting: (exporting) => set({ exporting }),
  setExportType: (exportType) => set({ exportType }),
  setDataFormat: (dataFormat) => set({ dataFormat }),
  setImageOptions: (imageOptions) => set({ imageOptions }),
  reset: () => set(initialState()),
}));
