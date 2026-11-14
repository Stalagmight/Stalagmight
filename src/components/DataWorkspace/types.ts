/**
 * Shared contract for the Data workspace (the full-project data table that
 * `appMode === 'data'` shows over the map area).
 *
 * Every Data-workspace module codes against these types. Change them only
 * deliberately: the row model, the column registry, the view pipeline and the
 * toolbar all meet here.
 */
import type { SurveyData, SectionMetadata } from '../../types/survey-data';
import type { Project } from '../../types/project';
import type { ProjectWarning, BatchCommand } from '../../types/commands';
import type { DecimalPrecision } from '../../types/config';
import type { TFunction } from 'i18next';

// ─── Rows ──────────────────────────────────────────────────────────────────

export type DataRowKind = 'section' | 'station' | 'shot';

interface DataRowBase {
  /** Unique across kinds and stable while the topology is unchanged:
   *  `sec:<sectionId>`, `st:<sectionId>:<stationIndex>`, `sh:<sectionId>:<shotId>`. */
  key: string;
  kind: DataRowKind;
  sectionId: number;
  /** Index of the section in `SurveyData.sections` (declaration order). */
  sectionOrder: number;
  /** Position in the full, unfiltered, unsorted survey-order row list. Used as
   *  the stable tiebreak when sorting and to decide rail adjacency. */
  order: number;
}

/** Optional group header that opens each section (toggle `showSectionHeaders`).
 *  Not editable; carries the collapse chevron and section-level facts. */
export interface SectionHeaderRow extends DataRowBase {
  kind: 'section';
}

export interface StationRow extends DataRowBase {
  kind: 'station';
  /** 0-based station index within the section (station i = start of shot i). */
  stationIndex: number;
  /** Global station index into the SurveyData station arrays. */
  g: number;
}

export interface ShotRow extends DataRowBase {
  kind: 'shot';
  /** 0-based index of the shot within its section (from station shotIndex to shotIndex+1). */
  shotIndex: number;
  /** 1-based position shown to users, as `formatShotId(sectionId, position)`. */
  position: number;
  shotId: number;
  /** Global shot index into the SurveyData shot arrays. */
  g: number;
}

export type DataRow = SectionHeaderRow | StationRow | ShotRow;
export type DataEntityRow = StationRow | ShotRow;

// ─── Columns ───────────────────────────────────────────────────────────────

export type DataColumnKey =
  | 'type'
  | 'section'
  | 'name'
  | 'fromTo'
  | 'length'
  | 'azimuth'
  | 'azimuth_in'
  | 'azimuth_out'
  | 'inclination'
  | 'pitch_in'
  | 'pitch_out'
  | 'depth_in'
  | 'depth_out'
  | 'depth'
  | 'left'
  | 'right'
  | 'up'
  | 'down'
  | 'kind'
  | 'issues'
  | 'comment'
  | 'x'
  | 'y'
  | 'lat'
  | 'long'
  | 'temperature'
  | 'surveyors'
  | 'explorers'
  | 'date';

export type DataColumnGroup = 'identity' | 'shot' | 'angles' | 'lrud' | 'station' | 'position' | 'notes' | 'credit';

export type DataValueType = 'number' | 'text' | 'enum' | 'date';

/** Why a cell can't take an edit right now. */
export type DataLockReason =
  /** The column does not apply to this row kind (a shot field on a station row). */
  | 'na'
  /** Computed / derived column (X, Y, depth, issues...). */
  | 'computed'
  /** Geometry field of a frozen shot. */
  | 'frozen'
  /** The whole workspace is read-only (closure-adjusted view, SpeleoDB lock). */
  | 'blocked';

/** Static metadata about a column (no logic): the columns menu, the filter
 *  builder and the search scope picker read this. */
export interface DataColumnMeta {
  key: DataColumnKey;
  /** i18n key of the header text (under `dataWorkspace.columns.*`). */
  labelKey: string;
  group: DataColumnGroup;
  appliesTo: 'station' | 'shot' | 'both';
  valueType: DataValueType;
  align: 'left' | 'right' | 'center';
  width: number;
  /** Can a user ever edit this column (some row kind, some state)? */
  editable: boolean;
  sortable: boolean;
  /** Searchable through find / replace. */
  searchable: boolean;
}

/** A column with its pure logic. Implemented in `model/columns.ts`. */
export interface DataColumnDef extends DataColumnMeta {
  /** Raw value for sort / filter: a number, a string, or null when empty /
   *  not applicable. Numbers stay numbers (sort numerically). */
  value(row: DataEntityRow, ctx: DataModelCtx): number | string | null;
  /** Display text. This is what the cell shows AND what search matches, so the
   *  two can never disagree. '' when empty or not applicable. */
  text(row: DataEntityRow, ctx: DataModelCtx): string;
  /** null when the cell takes an edit now; otherwise why not. */
  lockReason(row: DataEntityRow, ctx: DataModelCtx): DataLockReason | null;
  /** Coerce user text (typed, pasted, or a find/replace result) into the value
   *  an edit writes. `undefined` = invalid -> the write is skipped and reported.
   *  Present on editable columns only. */
  parse?(raw: string, row: DataEntityRow, ctx: DataModelCtx): unknown;
}

// ─── Model context ─────────────────────────────────────────────────────────

/** Everything a column needs to compute a cell. Built once per data change by
 *  the workspace shell; cheap lookups only (no per-cell scans). */
export interface DataModelCtx {
  /** Active geometry (measured or closure-adjusted) - coordinates come from here. */
  data: SurveyData;
  project: Project;
  sectionsById: Map<number, SectionMetadata>;
  /** Unique, <= 16 char section labels used in row names. */
  sectionLabels: Map<number, string>;
  /** Full display name per section (`name` or "Section <id>"). */
  sectionNames: Map<number, string>;
  /** Unresolved + resolved warnings keyed `${sectionId}:${shotId}`. */
  warningsByShot: Map<string, ProjectWarning[]>;
  /** Frozen shots keyed `${sectionId}:${shotIndex}`. */
  frozenShots: Set<string>;
  /** Decimal places for lengths / depths / coordinates. */
  precision: number;
  /** BCP-47 locale for number formatting / parsing. */
  locale: string;
  /** useEditingBlocked() at build time. */
  editingBlocked: boolean;
  /** Per-category decimal places (`useDecimalPrecision`). Optional for
   *  backward compatibility: absent = `precision` everywhere, 6 for GPS. */
  decimals?: DecimalPrecision;
  /** Translator for the few localized cell texts (row type, links, issues).
   *  Optional: absent = English fallbacks. */
  t?: TFunction;
}

// ─── Edits ─────────────────────────────────────────────────────────────────

export interface DataCellEdit {
  row: DataEntityRow;
  colKey: DataColumnKey;
  /** Already-parsed value (from `DataColumnDef.parse`). */
  value: unknown;
}

export interface DataEditPlan {
  /** Commands for one `executeBatch` call (one undo step). */
  commands: BatchCommand[];
  /** Edits dropped, with why (frozen, invalid, not applicable...).
   *  'conflict': an averaged-angle edit (azimuth / inclination) whose shot also
   *  had an in or out leg edited in the same batch - the explicit leg wins. */
  skipped: { edit: DataCellEdit; reason: DataLockReason | 'invalid' | 'conflict' }[];
}

// ─── View state (store) ────────────────────────────────────────────────────

export type TintMode = 'none' | 'section' | 'issues' | 'depth' | 'distance' | 'type';
export const TINT_MODES: readonly TintMode[] = ['none', 'section', 'issues', 'depth', 'distance', 'type'];

export type RowTypeFilter = 'all' | 'stations' | 'shots';

export type FilterOp = 'contains' | 'notContains' | 'eq' | 'neq' | 'lt' | 'lte' | 'gt' | 'gte' | 'empty' | 'notEmpty';

export interface FilterCondition {
  id: string;
  colKey: DataColumnKey;
  op: FilterOp;
  /** Raw user text; numeric ops parse it with the locale. */
  value: string;
}

export interface QuickFilters {
  /** Rows with an unresolved warning / error (shots) or touching one (stations). */
  issues: boolean;
  /** Rows with a non-empty comment. */
  comments: boolean;
  /** Stations with a merge or a closure; shots ending at one. */
  links: boolean;
  /** Frozen shots. */
  frozen: boolean;
  /** Jump / Reference shots. */
  virtualShots: boolean;
}

export interface FilterState {
  rowTypes: RowTypeFilter;
  /** null = every section. */
  sectionIds: number[] | null;
  quick: QuickFilters;
  /** ANDed. */
  conditions: FilterCondition[];
}

export interface SortState {
  colKey: DataColumnKey;
  dir: 'asc' | 'desc';
}

export interface SearchState {
  /** The find bar is shown. */
  open: boolean;
  /** The replace row is shown under it. */
  replaceOpen: boolean;
  query: string;
  replacement: string;
  matchCase: boolean;
  wholeWord: boolean;
  regex: boolean;
  /** 'visible': only rows the filters show. 'all': every row; matching rows the
   *  filters / collapse hide are revealed temporarily (and marked as such). */
  rowScope: 'visible' | 'all';
  /** 'visible': only shown columns. 'all': every searchable column; hidden
   *  columns that hold a match are revealed temporarily. 'custom': `columns`. */
  columnScope: 'visible' | 'all' | 'custom';
  columns: DataColumnKey[];
  /** Narrow the view to rows that hold a match. */
  onlyMatchingRows: boolean;
}

/**
 * The find's row-name pseudo-column: the row ID the gutter shows
 * ("0:5", `rowIdLabel`) followed by the section's full name, so a search for a
 * section's name finds its rows although no column holds it. Searched under the
 * 'visible' and 'all' column scopes (the gutter is always shown), not under
 * 'custom'; read-only (`lockReason: 'computed'`); never a grid column - a match
 * highlights the row name in the gutter. At most one match per row.
 */
export const ROW_NAME_KEY = 'row-name' as const;

/**
 * The find's links pseudo-column: a station's link chips (`linksSearchText`:
 * the chip labels, one per line). Like the row name it is no grid column (the
 * chips span the station row's shot-only cells, or sit in the gutter as a
 * count when no shot column is shown), is searched under the 'visible' and
 * 'all' column scopes only, never takes an edit (`lockReason: 'computed'`),
 * and a match outlines the chip(s) it falls in.
 */
export const ROW_LINKS_KEY = 'row-links' as const;

/** Find keys that are not grid columns (the row name, the links). */
export type PseudoSearchKey = typeof ROW_NAME_KEY | typeof ROW_LINKS_KEY;

export function isPseudoSearchKey(key: string): key is PseudoSearchKey {
  return key === ROW_NAME_KEY || key === ROW_LINKS_KEY;
}

/** Where a find match can sit: a column, the row name or the links. */
export type SearchColKey = DataColumnKey | PseudoSearchKey;

/** One match: a substring of one cell's display text (or of the row name,
 *  `colKey === ROW_NAME_KEY`, offsets into `rowNameSearchText`). */
export interface SearchMatch {
  rowKey: string;
  colKey: SearchColKey;
  /** [start, end) into `DataColumnDef.text(row)`. */
  start: number;
  end: number;
  /** null = can be replaced; otherwise why not. */
  lockReason: DataLockReason | null;
}

/** A request for the grid to bring a row (and optionally a column) into view
 *  and make it active. `nonce` lets the same target be requested twice. */
export interface FocusRequest {
  rowKey: string;
  colKey?: DataColumnKey;
  align?: 'auto' | 'center';
  /** false: scroll to and activate the cell WITHOUT moving DOM focus into the
   *  grid (the find box keeps the keyboard while the user types). Default true.
   *  Optional, so older requests keep their meaning. */
  takeFocus?: boolean;
  nonce: number;
}

// ─── View pipeline output ──────────────────────────────────────────────────

export interface DataView {
  /** The rows the grid shows, in display order. */
  rows: DataRow[];
  /** All matches in display order (row order, then column order, then offset). */
  matches: SearchMatch[];
  /** rowKey -> colKey -> indexes into `matches` (for cell highlighting; the
   *  row name under `ROW_NAME_KEY`). */
  matchIndex: Map<string, Map<SearchColKey, number[]>>;
  /** Rows shown only because they hold a match (filtered out / collapsed). */
  revealedRowKeys: Set<string>;
  /** Hidden columns shown only because they hold a match. */
  revealedColumns: Set<DataColumnKey>;
  /** Entity rows before search narrowing (for "N of M rows" in the status bar). */
  filteredCount: number;
  /** Entity rows in the project. */
  totalCount: number;
  /** True when the order is survey order (no sort) - the link rail is drawn only then. */
  surveyOrder: boolean;
  /** The search stopped at `MAX_SEARCH_MATCHES` (model/search.ts): `matches` is
   *  the first that many in survey order. Optional for older producers. */
  matchLimitReached?: boolean;
  /** "Show only rows with matches" is narrowing the rows now (the find bar is
   *  open with a valid query and the option on). Optional for older producers. */
  searchNarrowing?: boolean;
}
