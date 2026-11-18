/**
 * The data table's column groups as its menus name them: the shots view's
 * header right-click "Hide <name>" targets and the stations view's eye-menu
 * rows. Each `nameKey` is a key of `dataTable.columnsMenu` (shots) or
 * `dataTable.stationColumnsMenu` (stations); `dataTableColumnNames.test.ts`
 * holds them to the English strings.
 */

/** Header right-click hide targets: the clicked column's display name (a key
 *  of `dataTable.columnsMenu`) and the key group it hides with (L/R/U/D hide
 *  together as LRUD). */
export const HEADER_HIDE_GROUPS: Record<string, { nameKey: string; keys: string[] }> = {
  _warnings: { nameKey: 'warnings', keys: ['_warnings'] },
  length: { nameKey: 'length', keys: ['length'] },
  depth_in: { nameKey: 'depthIn', keys: ['depth_in'] },
  depth_out: { nameKey: 'depthOut', keys: ['depth_out'] },
  left: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  right: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  up: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  down: { nameKey: 'lrud', keys: ['left', 'right', 'up', 'down'] },
  kind: { nameKey: 'reference', keys: ['kind'] },
  comment: { nameKey: 'comment', keys: ['comment'] },
  line_id: { nameKey: 'lineName', keys: ['line_id'] },
  shot_id: { nameKey: 'shotLabel', keys: ['shot_id'] },
};

/** Stations-view column groups in the eye menu / header hide entries (plain
 *  hide - no angle modes), named by keys of `dataTable.stationColumnsMenu`.
 *  X/Y toggle together as "Coordinates (m)", lat/long as the GPS pair. The
 *  index pill lives in the gutter, so it never hides. */
export const STATION_MENU_COLS: { keys: string[]; nameKey: string }[] = [
  { keys: ['name'], nameKey: 'name' },
  { keys: ['_stationId'], nameKey: 'stationId' },
  { keys: ['_x', '_y'], nameKey: 'coordsMeters' },
  { keys: ['_lat', '_long'], nameKey: 'coordsGps' },
  { keys: ['_depth'], nameKey: 'depth' },
  { keys: ['_zeroDatum'], nameKey: 'zeroDatum' },
  { keys: ['_connections'], nameKey: 'connections' },
  { keys: ['comment'], nameKey: 'comment' },
];
