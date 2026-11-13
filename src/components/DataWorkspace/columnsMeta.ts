/**
 * Static column metadata for the Data workspace: order, groups, defaults.
 * Logic (values, text, parsing, edit locks) lives in `model/columns.ts`.
 */
import type { DataColumnGroup, DataColumnKey, DataColumnMeta } from './types';

const m = (
  key: DataColumnKey,
  group: DataColumnGroup,
  appliesTo: DataColumnMeta['appliesTo'],
  valueType: DataColumnMeta['valueType'],
  width: number,
  opts: Partial<Pick<DataColumnMeta, 'align' | 'editable' | 'sortable' | 'searchable'>> = {},
): DataColumnMeta => ({
  key,
  labelKey: `dataWorkspace.columns.${key}`,
  group,
  appliesTo,
  valueType,
  width,
  align: opts.align ?? (valueType === 'number' ? 'right' : 'left'),
  editable: opts.editable ?? false,
  sortable: opts.sortable ?? true,
  searchable: opts.searchable ?? true,
});

/** Every column, in the default display order. Default widths fit the header
 *  label (`dataWorkspace.headers.*`) in full next to the sort button, pinned by
 *  `__tests__/columnsMeta.test.ts`; a width the user dragged is stored apart
 *  (`columnWidths`) and wins. */
export const DATA_COLUMNS: readonly DataColumnMeta[] = [
  m('issues', 'identity', 'both', 'text', 48, { align: 'center' }),
  m('type', 'identity', 'both', 'enum', 72),
  m('section', 'identity', 'both', 'text', 160),
  m('name', 'station', 'station', 'text', 96, { editable: true }),
  m('fromTo', 'identity', 'shot', 'text', 110),
  m('length', 'shot', 'shot', 'number', 92, { editable: true }),
  m('azimuth', 'angles', 'shot', 'number', 84, { editable: true }),
  m('azimuth_in', 'angles', 'shot', 'number', 80, { editable: true }),
  m('azimuth_out', 'angles', 'shot', 'number', 80, { editable: true }),
  m('inclination', 'angles', 'shot', 'number', 84, { editable: true }),
  m('pitch_in', 'angles', 'shot', 'number', 80, { editable: true }),
  m('pitch_out', 'angles', 'shot', 'number', 88, { editable: true }),
  m('depth_in', 'shot', 'shot', 'number', 100, { editable: true }),
  m('depth_out', 'shot', 'shot', 'number', 100, { editable: true }),
  m('depth', 'position', 'station', 'number', 84),
  m('left', 'lrud', 'shot', 'number', 60, { editable: true }),
  m('right', 'lrud', 'shot', 'number', 60, { editable: true }),
  m('up', 'lrud', 'shot', 'number', 60, { editable: true }),
  m('down', 'lrud', 'shot', 'number', 60, { editable: true }),
  m('kind', 'shot', 'shot', 'enum', 58, { editable: true, align: 'center' }),
  m('links', 'station', 'station', 'text', 320),
  m('comment', 'notes', 'both', 'text', 280, { editable: true }),
  m('x', 'position', 'station', 'number', 96),
  m('y', 'position', 'station', 'number', 96),
  m('lat', 'position', 'station', 'number', 110),
  m('long', 'position', 'station', 'number', 110),
  m('temperature', 'shot', 'shot', 'number', 80, { editable: true }),
  m('surveyors', 'credit', 'shot', 'text', 160, { editable: true }),
  m('explorers', 'credit', 'shot', 'text', 160, { editable: true }),
  m('date', 'credit', 'shot', 'date', 110, { editable: true }),
];

export const DATA_COLUMN_BY_KEY: ReadonlyMap<DataColumnKey, DataColumnMeta> = new Map(
  DATA_COLUMNS.map((c) => [c.key, c]),
);

/** Hidden until the user shows them (also the backend default for
 *  `panels.data_workspace_hidden_columns`). */
export const DATA_DEFAULT_HIDDEN: readonly DataColumnKey[] = [
  'type',
  'section',
  'name',
  'fromTo',
  'azimuth_in',
  'azimuth_out',
  'inclination',
  'pitch_in',
  'pitch_out',
  'x',
  'y',
  'lat',
  'long',
  'temperature',
  'surveyors',
  'explorers',
  'date',
];

/** Column groups in menu order (i18n: `dataWorkspace.columnGroups.<group>`). */
export const DATA_COLUMN_GROUPS: readonly DataColumnGroup[] = [
  'identity',
  'shot',
  'angles',
  'lrud',
  'station',
  'position',
  'notes',
  'credit',
];
