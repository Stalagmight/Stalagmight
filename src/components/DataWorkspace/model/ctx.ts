/**
 * Small cached lookups over a `DataModelCtx` shared by the column, link and
 * edit logic: per-category precision, row names, the section objects (for the
 * per-shot survey credit), the GPS origin, and a translator with English
 * fallbacks so the pure model works without i18n.
 *
 * Every cache is a WeakMap keyed by an object the store replaces on change
 * (SurveyData, Project), so nothing here can go stale or leak.
 */
import type { DecimalPrecision } from '../../../types/config';
import type { Section, SurveyCredit } from '../../../types/ipc.generated';
import type { Project } from '../../../types/project';
import type { SurveyData } from '../../../types/survey-data';
import { lazySectionHasNoCredits } from '../../../utils/projectPeople';
import { formatFixed } from '../../../i18n/format';
import type { DataModelCtx, DataRow, ShotRow } from '../types';

export function decimalsOf(ctx: DataModelCtx): DecimalPrecision {
  if (ctx.decimals) return ctx.decimals;
  const p = ctx.precision;
  return { length: p, coordinates: p, depth: p, azimuth: p, inclination: p, gps: 6 };
}

/** `formatFixed` in the ctx locale; '' for a non-finite value. */
export function fmt(ctx: DataModelCtx, value: number, places: number): string {
  return Number.isFinite(value) ? formatFixed(value, ctx.locale, places) : '';
}

/** `t(key, opts)` when the ctx has a translator, else the English fallback with
 *  `{{name}}` placeholders filled from `opts`. */
export function tr(ctx: Pick<DataModelCtx, 't'>, key: string, fallback: string, opts?: Record<string, unknown>): string {
  if (ctx.t) return String(ctx.t(key, opts ?? {}));
  if (!opts) return fallback;
  return fallback.replace(/\{\{(\w+)\}\}/g, (_, k: string) => String(opts[k] ?? ''));
}

export function sectionLabelOf(ctx: DataModelCtx, sectionId: number): string {
  return ctx.sectionLabels.get(sectionId) ?? String(sectionId);
}

/** "<label>:<index>" for a station. */
export function stationName(ctx: DataModelCtx, sectionId: number, stationIndex: number): string {
  return `${sectionLabelOf(ctx, sectionId)}:${stationIndex}`;
}

/** "<label>:<position>" for a shot (1-based position). */
export function shotName(ctx: DataModelCtx, sectionId: number, position: number): string {
  return `${sectionLabelOf(ctx, sectionId)}:${position}`;
}

/** The row name the gutter shows ("Main:3"); a header row gets its full name. */
export function rowName(ctx: DataModelCtx, row: DataRow): string {
  if (row.kind === 'station') return stationName(ctx, row.sectionId, row.stationIndex);
  if (row.kind === 'shot') return shotName(ctx, row.sectionId, row.position);
  return ctx.sectionNames.get(row.sectionId) ?? sectionLabelOf(ctx, row.sectionId);
}

const sectionOrderCache = new WeakMap<SurveyData, Map<number, number>>();
/** Section id -> declaration order. */
export function sectionOrderOf(data: SurveyData): Map<number, number> {
  let m = sectionOrderCache.get(data);
  if (!m) {
    m = new Map(data.sections.map((s, i) => [s.id, i]));
    sectionOrderCache.set(data, m);
  }
  return m;
}

const sectionObjectCache = new WeakMap<Project, Map<number, Section>>();
function sectionObject(project: Project, sectionId: number): Section | undefined {
  let m = sectionObjectCache.get(project);
  if (!m) {
    m = new Map(project.sections.map((s) => [s.id, s]));
    sectionObjectCache.set(project, m);
  }
  return m.get(sectionId);
}

const creditFreeCache = new WeakMap<object, boolean>();
/**
 * A shot's survey credit. Credit lives only on materialized `Shot` objects, so
 * this reads `section.shots` for THAT section (once, then cached by the
 * loader) - and skips even that for a lazy section that credits nobody. Only
 * visible credit cells, or a search / filter over a credit column, call it.
 */
export function shotCredit(ctx: DataModelCtx, row: ShotRow): SurveyCredit | undefined {
  const section = sectionObject(ctx.project, row.sectionId);
  if (!section) return undefined;
  let free = creditFreeCache.get(section);
  if (free === undefined) {
    free = lazySectionHasNoCredits(section);
    creditFreeCache.set(section, free);
  }
  if (free) return undefined;
  const shot = section.shots[row.shotIndex];
  return shot && shot.id === row.shotId ? shot.credit : section.shots.find((s) => s.id === row.shotId)?.credit;
}

const gpsOriginCache = new WeakMap<Project, { latitude: number; longitude: number } | null>();
/** The survey's GPS anchor (first zero datum), as the map and DataTable use it. */
export function gpsOriginOf(project: Project): { latitude: number; longitude: number } | null {
  if (gpsOriginCache.has(project)) return gpsOriginCache.get(project)!;
  const gps = project.zero_datums?.[0]?.gps;
  const origin = gps ? { latitude: gps.latitude, longitude: gps.longitude } : null;
  gpsOriginCache.set(project, origin);
  return origin;
}
