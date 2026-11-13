/**
 * The Data workspace's row filters: row type, sections, quick filters and the
 * ANDed column conditions.
 *
 * ## Cross-kind semantics (one rule for quick filters and conditions)
 *
 * Station rows and shot rows carry different columns. A predicate on a column
 * that both kinds have (comment, section, issues...) is checked on every row
 * directly. A predicate on a column only ONE kind has is checked on that kind,
 * and the other kind follows its neighbours in the section's chain, so the
 * chain stays readable:
 *
 *  - a SHOT-only predicate (Length > 30, frozen, jumps/references, has issues):
 *    a shot passes when it satisfies them; a station passes when a shot that
 *    starts or ends at it does (the stations bracketing the kept shots);
 *  - a STATION-only predicate (Depth > 50, has links): a station passes when it
 *    satisfies them; a shot passes when its from- or to-station does.
 *
 * All shot-only predicates are judged together on the SAME neighbouring shot
 * (and all station-only ones on the same neighbouring station): a station next
 * to one long shot and another north-facing shot does not pass
 * "Length > 30 AND Azimuth < 10". Predicates on shared columns always judge the
 * row itself. With both kinds of predicate, a row needs both: "Length > 30 AND
 * Depth > 50" keeps the long shots that touch a deep station and the deep
 * stations that touch a long shot.
 *
 * The row-type control only hides a kind afterwards; it never changes which
 * rows match ("Stations" + "Length > 30" = the stations at the ends of long
 * shots). The section filter applies to every row directly.
 */
import { getLocaleSeparators, parseLocaleNumber } from '../../common/SpreadsheetGrid/gridClipboard';
import { shotKindFromFlags } from '../../../types/survey-data';
import type {
  DataColumnDef,
  DataColumnMeta,
  DataEntityRow,
  DataModelCtx,
  DataRow,
  DataValueType,
  FilterCondition,
  FilterOp,
  FilterState,
  QuickFilters,
  ShotRow,
  StationRow,
} from '../types';

type RowPred = (row: DataEntityRow) => boolean;
type StationPred = (row: StationRow) => boolean;
type ShotPred = (row: ShotRow) => boolean;

/** Operators offered per value type, in menu order. */
const OPS_BY_TYPE: Record<DataValueType, readonly FilterOp[]> = {
  number: ['eq', 'neq', 'lt', 'lte', 'gt', 'gte', 'empty', 'notEmpty'],
  text: ['contains', 'notContains', 'eq', 'neq', 'empty', 'notEmpty'],
  enum: ['eq', 'neq', 'empty', 'notEmpty'],
  date: ['eq', 'neq', 'lt', 'lte', 'gt', 'gte', 'empty', 'notEmpty'],
};

export function opsForValueType(type: DataValueType): readonly FilterOp[] {
  return OPS_BY_TYPE[type];
}

/** `empty` / `notEmpty` take no value. */
export function opNeedsValue(op: FilterOp): boolean {
  return op !== 'empty' && op !== 'notEmpty';
}

const ISO_DAY_RE = /^\d{4}-\d{2}-\d{2}$/;

export type ConditionProblem = 'needsValue' | 'notANumber' | 'notADate';

/**
 * Why a condition can't be applied, or null when it can. The model ignores an
 * invalid condition (it filters nothing) and the builder refuses to add one.
 */
export function conditionProblem(
  cond: Pick<FilterCondition, 'op' | 'value'>,
  meta: Pick<DataColumnMeta, 'valueType'>,
  locale: string,
): ConditionProblem | null {
  if (!opNeedsValue(cond.op)) return null;
  const raw = cond.value.trim();
  if (raw === '') return 'needsValue';
  if (meta.valueType === 'number' && parseLocaleNumber(raw, locale) === undefined) return 'notANumber';
  if (meta.valueType === 'date' && !ISO_DAY_RE.test(raw)) return 'notADate';
  return null;
}

/** Decimal places the user typed ("10" -> 0, "10.50" -> 2), for `eq` on numbers. */
function typedDecimals(raw: string, locale: string): number {
  const { decimal } = getLocaleSeparators(locale);
  const s = raw.trim();
  const at = Math.max(s.lastIndexOf(decimal), decimal === '.' ? -1 : s.lastIndexOf('.'));
  if (at < 0) return 0;
  const tail = s.slice(at + 1);
  return /^\d+$/.test(tail) ? Math.min(tail.length, 10) : 0;
}

const isEmptyValue = (v: number | string | null): boolean =>
  v === null || (typeof v === 'number' ? !Number.isFinite(v) : v.trim() === '');

/**
 * Compile one condition into a row predicate, or null when it is invalid (then
 * it is ignored). Numbers compare at the displayed precision or the typed one,
 * whichever is finer, so a filter agrees with what the cells show (and with the
 * float32 storage, where 10.4 is 10.3999996): with 1 decimal shown, "= 10"
 * keeps 10.04 (shown as 10.0) but not 10.4, and "> 10" drops 10.04. Text
 * comparisons ignore case; `eq` on text also ignores surrounding spaces.
 */
export function compileCondition(cond: FilterCondition, col: DataColumnDef, ctx: DataModelCtx): RowPred | null {
  if (conditionProblem(cond, col, ctx.locale) !== null) return null;
  const { op } = cond;
  if (op === 'empty') return (row) => isEmptyValue(col.value(row, ctx));
  if (op === 'notEmpty') return (row) => !isEmptyValue(col.value(row, ctx));

  if (col.valueType === 'number' && op !== 'contains' && op !== 'notContains') {
    const target = parseLocaleNumber(cond.value, ctx.locale)!;
    const decimals = Math.max(ctx.precision, typedDecimals(cond.value, ctx.locale));
    const half = 0.5 * 10 ** -decimals + 1e-9;
    /** -1 / 0 / 1 against the target at the comparison precision; null = no value. */
    const cmp = (row: DataEntityRow): number | null => {
      const v = col.value(row, ctx);
      if (typeof v !== 'number' || !Number.isFinite(v)) return null;
      return Math.abs(v - target) < half ? 0 : Math.sign(v - target);
    };
    switch (op) {
      case 'eq': return (row) => cmp(row) === 0;
      case 'neq': return (row) => cmp(row) !== 0;
      case 'lt': return (row) => { const c = cmp(row); return c !== null && c < 0; };
      case 'lte': return (row) => { const c = cmp(row); return c !== null && c <= 0; };
      case 'gt': return (row) => { const c = cmp(row); return c !== null && c > 0; };
      case 'gte': return (row) => { const c = cmp(row); return c !== null && c >= 0; };
      default: break;
    }
  }

  if (col.valueType === 'date' && op !== 'contains' && op !== 'notContains') {
    const target = cond.value.trim();
    const day = (row: DataEntityRow): string | null => {
      const v = col.value(row, ctx);
      return typeof v === 'string' && v.trim() !== '' ? v.trim().slice(0, 10) : null;
    };
    switch (op) {
      case 'eq': return (row) => day(row) === target;
      case 'neq': return (row) => day(row) !== target;
      case 'lt': return (row) => { const d = day(row); return d !== null && d < target; };
      case 'lte': return (row) => { const d = day(row); return d !== null && d <= target; };
      case 'gt': return (row) => { const d = day(row); return d !== null && d > target; };
      case 'gte': return (row) => { const d = day(row); return d !== null && d >= target; };
    }
  }

  const needle = cond.value.trim().toLowerCase();
  const text = (row: DataEntityRow) => col.text(row, ctx).toLowerCase();
  switch (op) {
    case 'contains': return (row) => text(row).includes(needle);
    case 'notContains': return (row) => !text(row).includes(needle);
    case 'eq': return (row) => text(row).trim() === needle;
    case 'neq': return (row) => text(row).trim() !== needle;
    default: {
      // lt / gt on a text column (not offered by the builder, but a valid state).
      const sign = op === 'lt' || op === 'lte' ? -1 : 1;
      const inclusive = op === 'lte' || op === 'gte';
      return (row) => {
        const t = text(row).trim();
        if (t === '') return false;
        const c = t.localeCompare(needle, ctx.locale, { numeric: true, sensitivity: 'base' });
        return c === 0 ? inclusive : Math.sign(c) === sign;
      };
    }
  }
}

/** `${sectionId}:${shotId}` -> Set per section of shot ids with an UNRESOLVED warning. */
function unresolvedShotIds(ctx: DataModelCtx): Map<number, Set<number>> {
  const out = new Map<number, Set<number>>();
  for (const list of ctx.warningsByShot.values()) {
    for (const w of list) {
      if (w.isResolved) continue;
      let set = out.get(w.sectionId);
      if (!set) out.set(w.sectionId, (set = new Set()));
      set.add(w.shotId);
    }
  }
  return out;
}

/** `${sectionId}:${shotIndex}` keys -> Set of shot indexes per section. */
function frozenShotIndexes(ctx: DataModelCtx): Map<number, Set<number>> {
  const out = new Map<number, Set<number>>();
  for (const key of ctx.frozenShots) {
    const colon = key.indexOf(':');
    const sid = Number(key.slice(0, colon));
    const idx = Number(key.slice(colon + 1));
    if (!Number.isFinite(sid) || !Number.isFinite(idx)) continue;
    let set = out.get(sid);
    if (!set) out.set(sid, (set = new Set()));
    set.add(idx);
  }
  return out;
}

interface PredicateGroups {
  station: StationPred[];
  shot: ShotPred[];
  both: RowPred[];
}

function quickPredicates(quick: QuickFilters, ctx: DataModelCtx, groups: PredicateGroups): void {
  const { data } = ctx;
  if (quick.issues) {
    const bad = unresolvedShotIds(ctx);
    groups.shot.push((row) => bad.get(row.sectionId)?.has(row.shotId) === true);
  }
  if (quick.frozen) {
    const frozen = frozenShotIndexes(ctx);
    groups.shot.push((row) => frozen.get(row.sectionId)?.has(row.shotIndex) === true);
  }
  if (quick.virtualShots) {
    groups.shot.push((row) => shotKindFromFlags(data.shotFlags[row.g] ?? 0) !== 'Normal');
  }
  if (quick.links) {
    groups.station.push(
      (row) =>
        (data.stationMergedWith.get(row.g)?.length ?? 0) > 0 ||
        (data.stationClosuresWith.get(row.g)?.length ?? 0) > 0,
    );
  }
  if (quick.comments) {
    groups.both.push((row) =>
      row.kind === 'shot'
        ? (data.shotComments.get(row.g)?.trim() ?? '') !== ''
        : (data.stationComments.get(row.g)?.trim() ?? '') !== '',
    );
  }
}

function conditionPredicates(
  conditions: readonly FilterCondition[],
  columns: readonly DataColumnDef[],
  ctx: DataModelCtx,
  groups: PredicateGroups,
): void {
  const byKey = new Map(columns.map((c) => [c.key, c]));
  for (const cond of conditions) {
    const col = byKey.get(cond.colKey);
    if (!col) continue;
    const pred = compileCondition(cond, col, ctx);
    if (!pred) continue;
    if (col.appliesTo === 'station') groups.station.push(pred);
    else if (col.appliesTo === 'shot') groups.shot.push(pred);
    else groups.both.push(pred);
  }
}

/** True when the filters narrow anything at all (row type, sections, quick, conditions). */
export function filtersActive(filters: FilterState): boolean {
  return (
    filters.rowTypes !== 'all' ||
    filters.sectionIds !== null ||
    Object.values(filters.quick).some(Boolean) ||
    filters.conditions.length > 0
  );
}

const all = <R>(preds: readonly ((r: R) => boolean)[], row: R): boolean => {
  for (const p of preds) if (!p(row)) return false;
  return true;
};

/**
 * Which rows pass the filters: a mask aligned with `rows` (1 = passes).
 * Section headers are always 0 here - the view decides which headers to keep.
 */
export function applyFilters(
  rows: readonly DataRow[],
  columns: readonly DataColumnDef[],
  ctx: DataModelCtx,
  filters: FilterState,
): Uint8Array {
  const pass = new Uint8Array(rows.length);
  const groups: PredicateGroups = { station: [], shot: [], both: [] };
  quickPredicates(filters.quick, ctx, groups);
  conditionPredicates(filters.conditions, columns, ctx, groups);
  const sections = filters.sectionIds === null ? null : new Set(filters.sectionIds);
  const keepKind = (kind: DataRow['kind']) =>
    filters.rowTypes === 'all' || (filters.rowTypes === 'stations' ? kind === 'station' : kind === 'shot');

  const crossKind = groups.station.length > 0 || groups.shot.length > 0;
  // Per section: the shot-only verdict per shot index and the station-only
  // verdict per station index, for the neighbour projection.
  const shotOwn = new Map<number, Uint8Array>();
  const stationOwn = new Map<number, Uint8Array>();
  if (crossKind) {
    const maxShot = new Map<number, number>();
    const maxStation = new Map<number, number>();
    for (const row of rows) {
      if (row.kind === 'shot') maxShot.set(row.sectionId, Math.max(maxShot.get(row.sectionId) ?? -1, row.shotIndex));
      else if (row.kind === 'station')
        maxStation.set(row.sectionId, Math.max(maxStation.get(row.sectionId) ?? -1, row.stationIndex));
    }
    for (const [sid, max] of maxShot) shotOwn.set(sid, new Uint8Array(max + 1));
    for (const [sid, max] of maxStation) stationOwn.set(sid, new Uint8Array(max + 2));
    for (const row of rows) {
      if (sections && !sections.has(row.sectionId)) continue;
      if (row.kind === 'shot' && groups.shot.length > 0 && all(groups.shot, row)) {
        shotOwn.get(row.sectionId)![row.shotIndex] = 1;
      } else if (row.kind === 'station' && groups.station.length > 0 && all(groups.station, row)) {
        stationOwn.get(row.sectionId)![row.stationIndex] = 1;
      }
    }
  }

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]!;
    if (row.kind === 'section') continue;
    if (sections && !sections.has(row.sectionId)) continue;
    if (!keepKind(row.kind)) continue;
    if (!all(groups.both, row)) continue;
    if (row.kind === 'shot') {
      if (groups.shot.length > 0 && shotOwn.get(row.sectionId)?.[row.shotIndex] !== 1) continue;
      if (groups.station.length > 0) {
        const st = stationOwn.get(row.sectionId);
        if (!st || (st[row.shotIndex] !== 1 && st[row.shotIndex + 1] !== 1)) continue;
      }
    } else {
      if (groups.station.length > 0 && stationOwn.get(row.sectionId)?.[row.stationIndex] !== 1) continue;
      if (groups.shot.length > 0) {
        const sh = shotOwn.get(row.sectionId);
        const i0 = row.stationIndex;
        if (!sh || (sh[i0 - 1] !== 1 && sh[i0] !== 1)) continue;
      }
    }
    pass[i] = 1;
  }
  return pass;
}
