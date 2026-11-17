/**
 * GridColumn<Shot> factory for the new-line dialog's shots grid.
 *
 * Deliberately a thin wrapper over the data table's `buildShotColumns` rather
 * than a parallel set: headers, formatting, locale-aware `parse`, `validate`,
 * the N/J/R kind cycle and the multiline comment editor then have exactly one
 * definition, so a shot typed into the dialog and the same shot edited later in
 * the data table behave identically.
 *
 * Two knobs make it fit: `lineId: null` (there is no line yet), which also
 * short-circuits the frozen-shot check in `editableUnlessFrozenGeometry` so
 * every column is editable, and dropping the three columns that have nothing to
 * say about an uncommitted row.
 */
import type { TFunction } from 'i18next';
import type { Shot } from '../../types/project';
import type { GridColumn } from '../common/SpreadsheetGrid/types';
import { buildShotColumns } from '../SingleSystemView/dataTableShotColumns';
import { AZIMUTH_AVG_KEY, PITCH_AVG_KEY } from '../SingleSystemView/dataTableEdits';

/** Columns that only make sense for a committed shot: warnings and frozen state
 *  are computed per line, and the two id columns have no value to show. */
export const NEW_LINE_OMITTED_COLUMNS: ReadonlySet<string> = new Set([
  '_warnings',
  'line_id',
  'shot_id',
]);

/**
 * Columns hidden on a config that has never stored a layout. Mirrors
 * `default_new_line_hidden_columns` in `src-tauri/src/config/types.rs`; the
 * Rust side is authoritative (it seeds the config), this is the fallback for a
 * backend that predates the field.
 */
export const DEFAULT_NEW_LINE_HIDDEN_COLUMNS: readonly string[] = [
  'azimuth_in',
  'azimuth_out',
  PITCH_AVG_KEY,
  'pitch_in',
  'pitch_out',
  'left',
  'right',
  'up',
  'down',
];

/**
 * Which column to reveal for each problem `draftMissing` reports as hidden.
 *
 * Its `hiddenProblems` set speaks in menu keys (`depthIn`), not column keys
 * (`depth_in`), because the data table uses them to label a menu row. The dialog
 * uses them to un-hide the column instead, so it needs the mapping back. Azimuth
 * resolves to the average column: it is the one visible by default, and setting
 * it fills both legs.
 */
export const MISSING_PROBLEM_COLUMNS: Readonly<Record<string, readonly string[]>> = {
  length: ['length'],
  depthIn: ['depth_in'],
  depthOut: ['depth_out'],
  azimuth: [AZIMUTH_AVG_KEY],
};

export interface NewLineColumnContext {
  t: TFunction;
  precision: { length: number; azimuth: number; depth: number; inclination: number };
  /** The dialog is unreachable while editing is blocked (the "+" button is
   *  disabled), but the grid is gated anyway so a mode change with the dialog
   *  already open cannot leave editable cells behind. */
  readOnly: boolean;
  /** Advance a row's kind around Normal -> Jump -> Reference (the inline J/R
   *  cell); `-1` walks it backwards on shift-click. `rowIndex` is the grid row,
   *  which is what the dialog's edit funnel addresses rows by. */
  onCycleShotKind: (shot: Shot, direction: 1 | -1, rowIndex: number) => void;
}

export function buildNewLineShotColumns(ctx: NewLineColumnContext): GridColumn<Shot>[] {
  return buildShotColumns({
    t: ctx.t,
    precision: ctx.precision,
    lineId: null,
    geometryReadOnly: ctx.readOnly,
    isShotFrozen: () => false,
    getShotWarnings: () => [],
    shotIndexAt: () => null,
    onCycleShotKind: ctx.onCycleShotKind,
  }).filter((c) => !NEW_LINE_OMITTED_COLUMNS.has(c.key));
}

/** Re-exported so the dialog and its tests name the virtual columns through one
 *  import rather than reaching into the data table's edit helpers. */
export { AZIMUTH_AVG_KEY, PITCH_AVG_KEY };
