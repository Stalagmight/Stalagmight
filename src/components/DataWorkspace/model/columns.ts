/**
 * Column logic for the Data workspace (values, display text, edit locks, parse).
 *
 * `text()` is the exact string the cell shows - and what find / replace
 * matches - so it reuses the docked DataTable's formatting: `formatFixed` with
 * the per-category decimal precision in the ctx locale, the averaged azimuth /
 * inclination, the J/R glyph for the shot kind. `parse()` mirrors the
 * DataTable's coercion and validation (azimuth 0..<360, pitch +-90, length > 0,
 * LRUD >= 0, required vs nullable numbers, locale decimals) and returns
 * `undefined` for anything it would reject. Every lookup is O(1) per cell.
 */
import { DATA_COLUMNS } from '../columnsMeta';
import type {
  DataColumnDef,
  DataColumnKey,
  DataColumnMeta,
  DataEntityRow,
  DataLockReason,
  DataModelCtx,
  ShotRow,
  StationRow,
} from '../types';
import type { ProjectWarning } from '../../../types/commands';
import type { Person, ShotKind } from '../../../types/ipc.generated';
import { nanToNull, shotKindFromFlags } from '../../../types/survey-data';
import { meanOrLoneLeg } from '../../../utils/angles';
import { displayStationName } from '../../../utils/formatters';
import { shotKindGlyph } from '../../../utils/shotKind';
import { xyToGps } from '../../../utils/coordinates';
import { parseLocaleNumber } from '../../common/SpreadsheetGrid/gridClipboard';
import { formatPeopleCell, parseDateCell, parsePeopleCell } from '../../SingleSystemView/dataTableCreditColumns';
import { decimalsOf, fmt, gpsOriginOf, shotCredit, tr } from './ctx';
import { stationGlobalIndex } from './rows';

// ─── Shared value readers ─────────────────────────────────────────────────

/** Fields whose edits move the adjusted centreline: locked on a frozen shot
 *  (the Rust `ShotUpdates::touches_geometry` set, minus LRUD). */
const FROZEN_LOCKED = new Set<DataColumnKey>([
  'length',
  'azimuth',
  'azimuth_in',
  'azimuth_out',
  'inclination',
  'pitch_in',
  'pitch_out',
  'depth_in',
  'depth_out',
]);

export function isShotFrozen(ctx: DataModelCtx, row: ShotRow): boolean {
  return ctx.frozenShots.has(`${row.sectionId}:${row.shotIndex}`);
}

export function shotWarnings(ctx: DataModelCtx, row: ShotRow): ProjectWarning[] {
  return ctx.warningsByShot.get(`${row.sectionId}:${row.shotId}`) ?? [];
}

const stationWarningCache = new WeakMap<Map<string, ProjectWarning[]>, Map<string, ProjectWarning[]>>();
/** Warnings that name a station (an over-anchored zero datum), by `sec:idx`. */
export function stationWarnings(ctx: DataModelCtx, row: StationRow): ProjectWarning[] {
  let index = stationWarningCache.get(ctx.warningsByShot);
  if (!index) {
    index = new Map();
    const add = (key: string, w: ProjectWarning) => {
      const list = index!.get(key);
      if (!list) index!.set(key, [w]);
      else if (!list.includes(w)) list.push(w);
    };
    for (const list of ctx.warningsByShot.values()) {
      for (const w of list) {
        for (const ref of w.datumRefs ?? []) add(`${ref.section_id}:${ref.station_index}`, w);
        if (typeof w.datumStationIndex === 'number') add(`${w.sectionId}:${w.datumStationIndex}`, w);
      }
    }
    stationWarningCache.set(ctx.warningsByShot, index);
  }
  return index.get(`${row.sectionId}:${row.stationIndex}`) ?? [];
}

export function rowWarnings(ctx: DataModelCtx, row: DataEntityRow): ProjectWarning[] {
  return row.kind === 'shot' ? shotWarnings(ctx, row) : stationWarnings(ctx, row);
}

/**
 * A Float32Array slot as the number the user typed: the shortest decimal that
 * is the same float32 (8.300000190734863 -> 8.3). The SoA stores shots as
 * float32, so without this the editor would seed, and a fill / move would
 * write, float32 noise. NaN stays NaN.
 */
export function f32(v: number): number {
  if (!Number.isFinite(v)) return v;
  for (let p = 1; p <= 9; p++) {
    const n = Number(v.toPrecision(p));
    if (Math.fround(n) === v) return n;
  }
  return v;
}

/** The angle legs of a shot as the IPC types them (NaN pitch = null). */
export function shotLegs(ctx: DataModelCtx, row: ShotRow) {
  const d = ctx.data;
  return {
    azimuth_in: f32(d.shotAzimuthIn[row.g]!),
    azimuth_out: f32(d.shotAzimuthOut[row.g]!),
    pitch_in: nanToNull(f32(d.shotPitchIn[row.g]!)),
    pitch_out: nanToNull(f32(d.shotPitchOut[row.g]!)),
  };
}

function meanAzimuth(ctx: DataModelCtx, row: ShotRow): number {
  const { azimuth_in: a, azimuth_out: b } = shotLegs(ctx, row);
  return meanOrLoneLeg(a, b);
}

function meanPitch(ctx: DataModelCtx, row: ShotRow): number | null {
  const { pitch_in: a, pitch_out: b } = shotLegs(ctx, row);
  if (a !== null && b !== null) return (a + b) / 2;
  return a ?? b ?? null;
}

export function shotKindOf(ctx: DataModelCtx, row: ShotRow): ShotKind {
  return shotKindFromFlags(ctx.data.shotFlags[row.g] ?? 0);
}

function stationCoord(ctx: DataModelCtx, row: StationRow, axis: 0 | 1 | 2): number {
  return ctx.data.stationCoords[row.g * 3 + axis]!;
}

function stationGps(ctx: DataModelCtx, row: StationRow): { latitude: number; longitude: number } | null {
  const own = ctx.data.stationGps.get(row.g);
  if (own) return own;
  const origin = gpsOriginOf(ctx.project);
  if (!origin) return null;
  const gps = xyToGps(
    { x: stationCoord(ctx, row, 0), y: stationCoord(ctx, row, 1), z: stationCoord(ctx, row, 2) },
    { ...origin, depth_reference: 0 },
  );
  return { latitude: gps.latitude, longitude: gps.longitude };
}

/** A station's display name ('' when it has none). */
export function stationDisplayName(ctx: DataModelCtx, g: number): string {
  return displayStationName(ctx.data.stationNames.get(g));
}

// ─── Text pieces shared with the cells / hover card ───────────────────────

/** "Δ 0.7 m · 1.2 %" for a closure (empty when the closure is unknown). */
export function misclosureText(ctx: DataModelCtx, misclosure: number | null, errorPct: number | null): string {
  if (misclosure === null || errorPct === null) return '';
  const d = decimalsOf(ctx);
  return `Δ ${fmt(ctx, misclosure, d.length)} m · ${fmt(ctx, errorPct, 1)} %`;
}

/** Unresolved warnings as searchable text (the find's issues pseudo-column,
 *  `ROW_ISSUES_KEY`): "message [CODE]; ...". */
export function issuesText(warnings: readonly ProjectWarning[]): string {
  return warnings
    .filter((w) => !w.isResolved)
    .map((w) => `${w.message} [${w.code}]`)
    .join('; ');
}

// ─── Parsing helpers ──────────────────────────────────────────────────────

type Parsed = number | null | undefined;

/** A required number: '' and junk -> undefined. */
function parseRequired(raw: string, ctx: DataModelCtx): Parsed {
  return parseLocaleNumber(raw, ctx.locale);
}

/** A nullable number: '' -> null (clears), junk -> undefined. */
function parseNullable(raw: string, ctx: DataModelCtx): Parsed {
  if (raw.trim() === '') return null;
  return parseLocaleNumber(raw, ctx.locale);
}

/** Why a parsed value is rejected (an i18n key under dataTable.validation),
 *  or null. Mirrors the DataTable column validators. */
export function validationError(key: DataColumnKey, value: unknown, row: DataEntityRow, ctx: DataModelCtx): string | null {
  if (typeof value !== 'number') return null;
  switch (key) {
    case 'length':
      return value <= 0 ? 'dataTable.validation.nonPositiveLength' : null;
    case 'azimuth':
    case 'azimuth_in':
    case 'azimuth_out':
      return value < 0 || value >= 360 ? 'dataTable.validation.azimuthRange' : null;
    case 'pitch_in':
    case 'pitch_out':
      return value < -90 || value > 90 ? 'dataTable.validation.pitchRange' : null;
    case 'inclination': {
      if (value < -90 || value > 90) return 'dataTable.validation.pitchRange';
      if (row.kind !== 'shot') return null;
      // A shift of both legs must keep each inside +-90 (pitch doesn't wrap).
      const { pitch_in: a, pitch_out: b } = shotLegs(ctx, row);
      if (a !== null && b !== null) {
        const delta = value - (a + b) / 2;
        if (Math.abs(a + delta) > 90 || Math.abs(b + delta) > 90) return 'dataTable.validation.pitchRange';
      }
      return null;
    }
    case 'left':
    case 'right':
    case 'up':
    case 'down':
      return value < 0 ? 'dataTable.validation.negativeLrud' : null;
    default:
      return null;
  }
}

/** The shot-kind cell's accepted spellings (glyphs, words, legacy booleans). */
export function parseShotKind(raw: string): ShotKind | undefined {
  const v = raw.trim().toLowerCase();
  if (v === '' || v === 'n' || v === 'normal' || v === 'permanent') return 'Normal';
  if (v === 'j' || v === 'jump') return 'Jump';
  if (v === 'r' || v === 'ref' || v === 'reference') return 'Reference';
  if (v === 'false' || v === '0' || v === 'no') return 'Normal';
  if (v === 'true' || v === '1' || v === 'yes') return 'Jump';
  return undefined;
}

/** Comment / name text: blank clears to null. */
const parseOptionalText = (raw: string): string | null => (raw.trim() === '' ? null : raw);

// ─── Column logic table ───────────────────────────────────────────────────

interface Logic {
  value(row: DataEntityRow, ctx: DataModelCtx): number | string | null;
  text(row: DataEntityRow, ctx: DataModelCtx): string;
  /** Coerce raw text (no range checks); `validationError` adds those. */
  coerce?(raw: string, row: DataEntityRow, ctx: DataModelCtx): unknown;
  /** Extra lock beyond row kind / computed / blocked (frozen geometry). */
  frozenLocks?: boolean;
}

const asShot = (row: DataEntityRow): ShotRow | null => (row.kind === 'shot' ? row : null);
const asStation = (row: DataEntityRow): StationRow | null => (row.kind === 'station' ? row : null);

/** A float field of the shot arrays, formatted with a precision category. */
function shotNumber(
  field: 'shotLength' | 'shotDepthIn' | 'shotDepthOut' | 'shotAzimuthIn' | 'shotAzimuthOut' | 'shotPitchIn' | 'shotPitchOut' | 'shotLeft' | 'shotRight' | 'shotUp' | 'shotDown' | 'shotTemperature',
  places: (ctx: DataModelCtx) => number,
  nullable: boolean,
): Logic {
  const read = (row: DataEntityRow, ctx: DataModelCtx): number | null => {
    const s = asShot(row);
    if (!s) return null;
    const v = ctx.data[field][s.g]!;
    return Number.isFinite(v) ? f32(v) : null;
  };
  return {
    value: read,
    text: (row, ctx) => {
      const v = read(row, ctx);
      return v === null ? '' : fmt(ctx, v, places(ctx));
    },
    coerce: (raw, _row, ctx) => (nullable ? parseNullable(raw, ctx) : parseRequired(raw, ctx)),
    frozenLocks: true,
  };
}

function stationNumber(read: (row: StationRow, ctx: DataModelCtx) => number | null, places: (ctx: DataModelCtx) => number): Logic {
  const value = (row: DataEntityRow, ctx: DataModelCtx) => {
    const s = asStation(row);
    if (!s) return null;
    const v = read(s, ctx);
    return v !== null && Number.isFinite(v) ? v : null;
  };
  return {
    value,
    text: (row, ctx) => {
      const v = value(row, ctx);
      return v === null ? '' : fmt(ctx, v, places(ctx));
    },
  };
}

const textValue = (text: (row: DataEntityRow, ctx: DataModelCtx) => string): Logic['value'] => (row, ctx) => {
  const v = text(row, ctx);
  return v === '' ? null : v;
};

function peopleLogic(role: 'surveyors' | 'explorers'): Logic {
  const people = (row: DataEntityRow, ctx: DataModelCtx): readonly Person[] => {
    const s = asShot(row);
    return s ? (shotCredit(ctx, s)?.[role] ?? []) : [];
  };
  const text = (row: DataEntityRow, ctx: DataModelCtx) => formatPeopleCell(people(row, ctx));
  return { text, value: textValue(text), coerce: (raw) => parsePeopleCell(raw) };
}

const typeText = (row: DataEntityRow, ctx: DataModelCtx) =>
  row.kind === 'station'
    ? tr(ctx, 'dataWorkspace.rowType.station', 'Station')
    : tr(ctx, 'dataWorkspace.rowType.shot', 'Shot');

const sectionText = (row: DataEntityRow, ctx: DataModelCtx) => ctx.sectionNames.get(row.sectionId) ?? String(row.sectionId);

const nameText = (row: DataEntityRow, ctx: DataModelCtx) => {
  const s = asStation(row);
  return s ? stationDisplayName(ctx, s.g) : '';
};

/** "2 → 3", or the station names when the endpoints have them. */
export function fromToText(row: DataEntityRow, ctx: DataModelCtx): string {
  const s = asShot(row);
  if (!s) return '';
  const names = ctx.data.stationNames;
  const end = (index: number) => {
    const g = stationGlobalIndex(ctx.data).get(s.sectionId)?.get(index);
    const name = g === undefined ? '' : displayStationName(names.get(g));
    return name !== '' ? name : String(index);
  };
  return `${end(s.shotIndex)} → ${end(s.shotIndex + 1)}`;
}

const commentText = (row: DataEntityRow, ctx: DataModelCtx) =>
  (row.kind === 'shot' ? ctx.data.shotComments.get(row.g) : ctx.data.stationComments.get(row.g)) ?? '';

const kindText = (row: DataEntityRow, ctx: DataModelCtx) => {
  const s = asShot(row);
  return s ? shotKindGlyph(shotKindOf(ctx, s)) : '';
};

const LOGIC: Record<DataColumnKey, Logic> = {
  type: { text: typeText, value: textValue(typeText) },
  section: { text: sectionText, value: textValue(sectionText) },
  name: { text: nameText, value: textValue(nameText), coerce: (raw) => parseOptionalText(raw.trim()) },
  fromTo: { text: fromToText, value: textValue(fromToText) },
  length: shotNumber('shotLength', (c) => decimalsOf(c).length, false),
  azimuth: {
    value: (row, ctx) => {
      const s = asShot(row);
      if (!s) return null;
      const v = meanAzimuth(ctx, s);
      return Number.isFinite(v) ? v : null;
    },
    text: (row, ctx) => {
      const s = asShot(row);
      if (!s) return '';
      return fmt(ctx, meanAzimuth(ctx, s), decimalsOf(ctx).azimuth);
    },
    coerce: (raw, _row, ctx) => parseRequired(raw, ctx),
    frozenLocks: true,
  },
  azimuth_in: shotNumber('shotAzimuthIn', (c) => decimalsOf(c).azimuth, false),
  azimuth_out: shotNumber('shotAzimuthOut', (c) => decimalsOf(c).azimuth, false),
  inclination: {
    value: (row, ctx) => {
      const s = asShot(row);
      return s ? meanPitch(ctx, s) : null;
    },
    text: (row, ctx) => {
      const s = asShot(row);
      const v = s ? meanPitch(ctx, s) : null;
      return v === null ? '' : fmt(ctx, v, decimalsOf(ctx).inclination);
    },
    coerce: (raw, _row, ctx) => parseNullable(raw, ctx),
    frozenLocks: true,
  },
  pitch_in: shotNumber('shotPitchIn', (c) => decimalsOf(c).inclination, true),
  pitch_out: shotNumber('shotPitchOut', (c) => decimalsOf(c).inclination, true),
  depth_in: shotNumber('shotDepthIn', (c) => decimalsOf(c).depth, false),
  depth_out: shotNumber('shotDepthOut', (c) => decimalsOf(c).depth, false),
  depth: stationNumber((s, ctx) => stationCoord(ctx, s, 2), (c) => decimalsOf(c).depth),
  left: { ...shotNumber('shotLeft', (c) => decimalsOf(c).length, true), frozenLocks: false },
  right: { ...shotNumber('shotRight', (c) => decimalsOf(c).length, true), frozenLocks: false },
  up: { ...shotNumber('shotUp', (c) => decimalsOf(c).length, true), frozenLocks: false },
  down: { ...shotNumber('shotDown', (c) => decimalsOf(c).length, true), frozenLocks: false },
  kind: {
    text: kindText,
    value: (row, ctx) => {
      const s = asShot(row);
      return s ? shotKindOf(ctx, s) : null;
    },
    coerce: (raw) => parseShotKind(raw),
  },
  comment: { text: commentText, value: textValue(commentText), coerce: (raw) => parseOptionalText(raw) },
  x: stationNumber((s, ctx) => stationCoord(ctx, s, 0), (c) => decimalsOf(c).coordinates),
  y: stationNumber((s, ctx) => stationCoord(ctx, s, 1), (c) => decimalsOf(c).coordinates),
  lat: stationNumber((s, ctx) => stationGps(ctx, s)?.latitude ?? null, (c) => decimalsOf(c).gps),
  long: stationNumber((s, ctx) => stationGps(ctx, s)?.longitude ?? null, (c) => decimalsOf(c).gps),
  temperature: { ...shotNumber('shotTemperature', () => 1, true), frozenLocks: false },
  surveyors: peopleLogic('surveyors'),
  explorers: peopleLogic('explorers'),
  date: {
    text: (row, ctx) => {
      const s = asShot(row);
      return s ? (shotCredit(ctx, s)?.date ?? '') : '';
    },
    value: (row, ctx) => {
      const s = asShot(row);
      return s ? (shotCredit(ctx, s)?.date ?? null) : null;
    },
    coerce: (raw) => parseDateCell(raw),
  },
};

function appliesTo(meta: DataColumnMeta, row: DataEntityRow): boolean {
  return meta.appliesTo === 'both' || meta.appliesTo === row.kind;
}

/** Why a cell can't take an edit right now (see DataLockReason), or null. */
function lockReasonFor(meta: DataColumnMeta, logic: Logic, row: DataEntityRow, ctx: DataModelCtx): DataLockReason | null {
  if (!appliesTo(meta, row)) return 'na';
  if (!meta.editable || !logic.coerce) return 'computed';
  if (ctx.editingBlocked) return 'blocked';
  if (logic.frozenLocks && FROZEN_LOCKED.has(meta.key) && row.kind === 'shot' && isShotFrozen(ctx, row)) return 'frozen';
  return null;
}

/** Coerce only (no range checks): the grid adapter pairs it with
 *  `validationError` so an interactive edit flashes the reason. */
export function coerceCell(key: DataColumnKey, raw: string, row: DataEntityRow, ctx: DataModelCtx): unknown {
  return LOGIC[key].coerce?.(raw, row, ctx);
}

export function buildColumnDefs(): DataColumnDef[] {
  return DATA_COLUMNS.map((meta): DataColumnDef => {
    const logic = LOGIC[meta.key];
    const def: DataColumnDef = {
      ...meta,
      value: (row, ctx) => (appliesTo(meta, row) ? logic.value(row, ctx) : null),
      text: (row, ctx) => (appliesTo(meta, row) ? logic.text(row, ctx) : ''),
      lockReason: (row, ctx) => lockReasonFor(meta, logic, row, ctx),
    };
    if (meta.editable && logic.coerce) {
      const coerce = logic.coerce;
      def.parse = (raw, row, ctx) => {
        const value = coerce(raw, row, ctx);
        if (value === undefined) return undefined;
        return validationError(meta.key, value, row, ctx) ? undefined : value;
      };
    }
    return def;
  });
}
