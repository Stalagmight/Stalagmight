/**
 * Fully synthetic Data-workspace model for the view / filter / search / replace
 * tests: a hand-built SurveyData, survey-order rows and a small column set with
 * the same contract as model/columns.ts (value / text / lockReason / parse).
 */
import { emptySurveyData, shotKindToFlags, type SurveyData } from '../../../../types/survey-data';
import type { Project } from '../../../../types/project';
import type { ProjectWarning } from '../../../../types/commands';
import type { ShotKind } from '../../../../types/ipc/ShotKind';
import { DATA_COLUMN_BY_KEY } from '../../columnsMeta';
import { parseLocaleNumber } from '../../../common/SpreadsheetGrid/gridClipboard';
import type {
  DataColumnDef,
  DataColumnKey,
  DataEntityRow,
  DataLockReason,
  DataModelCtx,
  DataRow,
} from '../../types';
import type { DataViewInput } from '../view';
import { EMPTY_FILTERS, EMPTY_SEARCH } from '../../store';

export interface ShotSeed {
  length: number;
  azimuth?: number;
  comment?: string | undefined;
  kind?: ShotKind;
  date?: string;
}

export interface SectionSeed {
  id: number;
  name: string;
  shots: ShotSeed[];
  /** z per station (n+1 values); default 0, -1, -2, ... */
  z?: number[];
  stationComments?: Record<number, string>;
  stationNames?: Record<number, string>;
}

export function makeData(sections: SectionSeed[]): SurveyData {
  const data = emptySurveyData();
  const shotTotal = sections.reduce((n, s) => n + s.shots.length, 0);
  const stationTotal = sections.reduce((n, s) => n + s.shots.length + 1, 0);
  data.shotCount = shotTotal;
  data.shotIds = new Uint32Array(shotTotal);
  data.shotSectionIds = new Uint32Array(shotTotal);
  data.shotLength = new Float32Array(shotTotal);
  data.shotAzimuthIn = new Float32Array(shotTotal);
  data.shotAzimuthOut = new Float32Array(shotTotal);
  data.shotFlags = new Uint8Array(shotTotal);
  data.stationCount = stationTotal;
  data.stationSectionIds = new Uint32Array(stationTotal);
  data.stationIndices = new Uint32Array(stationTotal);
  data.stationCoords = new Float64Array(stationTotal * 3);
  let g = 0;
  let sg = 0;
  for (const sec of sections) {
    data.sections.push({
      id: sec.id,
      name: sec.name,
      color: `#${(sec.id * 1234567).toString(16).padStart(6, '0').slice(0, 6)}`,
      direction: 'in',
      quality: 0,
      shotOffset: g,
      shotCount: sec.shots.length,
    } as SurveyData['sections'][number]);
    sec.shots.forEach((shot, i) => {
      data.shotIds[g] = sec.id * 100_000 + i + 1;
      data.shotSectionIds[g] = sec.id;
      data.shotLength[g] = shot.length;
      data.shotAzimuthIn[g] = shot.azimuth ?? 0;
      data.shotAzimuthOut[g] = shot.azimuth ?? 0;
      data.shotFlags[g] = shotKindToFlags(shot.kind ?? 'Normal');
      if (shot.comment) data.shotComments.set(g, shot.comment);
      g++;
    });
    for (let i = 0; i <= sec.shots.length; i++) {
      data.stationSectionIds[sg] = sec.id;
      data.stationIndices[sg] = i;
      data.stationCoords[sg * 3 + 2] = sec.z?.[i] ?? -i;
      const c = sec.stationComments?.[i];
      if (c) data.stationComments.set(sg, c);
      const nm = sec.stationNames?.[i];
      if (nm) data.stationNames.set(sg, nm);
      sg++;
    }
  }
  return data;
}

/** Survey-order rows: optional header, station 0, shot 1, station 1, ... */
export function makeRows(data: SurveyData, opts: { headers?: boolean } = {}): DataRow[] {
  const rows: DataRow[] = [];
  let sg = 0;
  data.sections.forEach((sec, sectionOrder) => {
    const sectionId = sec.id;
    if (opts.headers) rows.push({ kind: 'section', key: `sec:${sectionId}`, sectionId, sectionOrder, order: rows.length });
    for (let i = 0; i <= sec.shotCount; i++) {
      rows.push({
        kind: 'station',
        key: `st:${sectionId}:${i}`,
        sectionId,
        sectionOrder,
        order: rows.length,
        stationIndex: i,
        g: sg++,
      });
      if (i < sec.shotCount) {
        const g = sec.shotOffset + i;
        const shotId = data.shotIds[g]!;
        rows.push({
          kind: 'shot',
          key: `sh:${sectionId}:${shotId}`,
          sectionId,
          sectionOrder,
          order: rows.length,
          shotIndex: i,
          position: i + 1,
          shotId,
          g,
        });
      }
    }
  });
  return rows;
}

export interface CtxOpts {
  warnings?: ProjectWarning[];
  frozen?: string[];
  editingBlocked?: boolean;
  precision?: number;
  locale?: string;
}

export function makeCtx(data: SurveyData, opts: CtxOpts = {}): DataModelCtx {
  const warningsByShot = new Map<string, ProjectWarning[]>();
  for (const w of opts.warnings ?? []) {
    const key = `${w.sectionId}:${w.shotId}`;
    warningsByShot.set(key, [...(warningsByShot.get(key) ?? []), w]);
  }
  return {
    data,
    project: {} as Project,
    sectionsById: new Map(data.sections.map((s) => [s.id, s])),
    sectionLabels: new Map(data.sections.map((s) => [s.id, s.name ?? `S${s.id}`])),
    sectionNames: new Map(data.sections.map((s) => [s.id, s.name ?? `Section ${s.id}`])),
    warningsByShot,
    frozenShots: new Set(opts.frozen ?? []),
    precision: opts.precision ?? 1,
    locale: opts.locale ?? 'en-US',
    editingBlocked: opts.editingBlocked ?? false,
  };
}

export function warning(sectionId: number, shotId: number, isResolved = false): ProjectWarning {
  return { sectionId, shotId, shotIndex: 0, code: 'X', message: 'x', severity: 'warning', isResolved } as ProjectWarning;
}

const fmt = (v: number | null, ctx: DataModelCtx): string =>
  v === null || !Number.isFinite(v) ? '' : v.toFixed(ctx.precision);

function col(
  key: DataColumnKey,
  value: (row: DataEntityRow, ctx: DataModelCtx) => number | string | null,
  opts: {
    text?: (row: DataEntityRow, ctx: DataModelCtx) => string;
    lock?: (row: DataEntityRow, ctx: DataModelCtx) => DataLockReason | null;
    parse?: DataColumnDef['parse'];
  } = {},
): DataColumnDef {
  const meta = DATA_COLUMN_BY_KEY.get(key)!;
  const applies = (row: DataEntityRow) => meta.appliesTo === 'both' || meta.appliesTo === row.kind;
  const def: DataColumnDef = {
    ...meta,
    value: (row, ctx) => (applies(row) ? value(row, ctx) : null),
    text: (row, ctx) => {
      if (!applies(row)) return '';
      if (opts.text) return opts.text(row, ctx);
      const v = value(row, ctx);
      return typeof v === 'number' ? fmt(v, ctx) : (v ?? '');
    },
    lockReason: (row, ctx) => {
      if (!applies(row)) return 'na';
      if (ctx.editingBlocked) return 'blocked';
      if (opts.lock) return opts.lock(row, ctx);
      return meta.editable ? null : 'computed';
    },
  };
  if (opts.parse) def.parse = opts.parse;
  return def;
}

const numParse: DataColumnDef['parse'] = (raw, _row, ctx) => {
  const n = parseLocaleNumber(raw, ctx.locale);
  return n === undefined || n < 0 ? undefined : n;
};

/** A representative column set (shot, station and shared columns). */
export function makeColumns(): DataColumnDef[] {
  return [
    col('type', (row) => (row.kind === 'station' ? 'Station' : 'Shot')),
    col('section', (row, ctx) => ctx.sectionNames.get(row.sectionId) ?? ''),
    col('name', (row, ctx) => (row.kind === 'station' ? (ctx.data.stationNames.get(row.g) ?? null) : null), {
      parse: (raw) => raw.trim() || null,
    }),
    col('length', (row, ctx) => (row.kind === 'shot' ? ctx.data.shotLength[row.g]! : null), {
      parse: numParse,
      lock: (row, ctx) => (ctx.frozenShots.has(`${row.sectionId}:${(row as { shotIndex: number }).shotIndex}`) ? 'frozen' : null),
    }),
    col('azimuth', (row, ctx) => (row.kind === 'shot' ? ctx.data.shotAzimuthIn[row.g]! : null), { parse: numParse }),
    col('depth', (row, ctx) => (row.kind === 'station' ? -ctx.data.stationCoords[row.g * 3 + 2]! : null)),
    col(
      'kind',
      (row, ctx) =>
        row.kind === 'shot' ? (['Normal', 'Jump', 'Reference'][ctx.data.shotFlags[row.g]! & 3] ?? null) : null,
    ),
    col(
      'comment',
      (row, ctx) =>
        (row.kind === 'shot' ? ctx.data.shotComments.get(row.g) : ctx.data.stationComments.get(row.g)) ?? null,
      { parse: (raw) => raw },
    ),
    col('x', (row, ctx) => (row.kind === 'station' ? ctx.data.stationCoords[row.g * 3]! : null)),
  ];
}

export function viewState(patch: Partial<DataViewInput> = {}): DataViewInput {
  return {
    filters: EMPTY_FILTERS,
    sort: null,
    collapsedSections: [],
    search: EMPTY_SEARCH,
    hiddenColumns: [],
    columnOrder: [],
    revealRowKeys: [],
    ...patch,
  };
}

export const keys = (rows: readonly DataRow[]): string[] => rows.map((r) => r.key);
