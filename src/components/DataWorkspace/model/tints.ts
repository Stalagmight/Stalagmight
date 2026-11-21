/**
 * Row tints for the Data workspace, mirroring the map's Display colour modes:
 * section colour, Survey Issues severity, depth ramp, along-cave distance, and
 * a quiet station / shot distinction. Pure: the colour functions (the map's
 * own `issueScoreColor` / `depthColorFor` / distance scale, bound to the theme
 * and the user's palette + Levels) are injected by `useDataTint`, so this file
 * is a cheap per-row lookup over precomputed arrays.
 *
 * A tint is a CSS colour (a hex or a `var(--...)`), drawn at full strength in
 * the gutter stripe and washed out under the row by the grid.
 */
import type { DistanceField } from '../../../types/project';
import type { DataModelCtx, DataRow, TintMode } from '../types';

/** Station distance by `sectionId:stationIndex` (null = unreachable). */
export type DistanceIndex = Map<string, number | null>;

export function buildDistanceIndex(field: DistanceField): DistanceIndex {
  const m: DistanceIndex = new Map();
  for (let i = 0; i < field.station_refs.length; i++) {
    const r = field.station_refs[i]!;
    m.set(`${r.section_id}:${r.station_index}`, field.distances[i] ?? null);
  }
  return m;
}

export interface TintInputs {
  mode: TintMode;
  ctx: Pick<DataModelCtx, 'data' | 'sectionsById'>;
  /** Survey Issues scores per GLOBAL shot / station index (`computeIssueScores`). */
  issueScores?: { shotScore: Float32Array; stationScore: Float32Array } | null;
  /** Score (0..3) -> colour, the map's ramp with the user's palette. */
  issueColor?: (score: number) => string;
  /** Depth value -> colour (the map's ramp / palette + Levels), or null. */
  depthColor?: (depth: number) => string | null;
  distance?: { index: DistanceIndex; color: (d: number) => string | null } | null;
}

/** The colour token the 'type' mode uses for station / shot rows. */
export const TYPE_TINT = {
  station: 'var(--sh-station)',
  shot: 'var(--sh-shot)',
} as const;

function shotDistance(inputs: TintInputs, sectionId: number, shotIndex: number): number | null {
  const index = inputs.distance?.index;
  if (!index) return null;
  const a = index.get(`${sectionId}:${shotIndex}`);
  const b = index.get(`${sectionId}:${shotIndex + 1}`);
  return a == null || b == null ? null : (a + b) / 2;
}

/** A row's tint colour, or null for no tint. */
export function rowTint(row: DataRow, inputs: TintInputs): string | null {
  const { mode, ctx } = inputs;
  if (mode === 'none') return null;
  const sectionColor = ctx.sectionsById.get(row.sectionId)?.color ?? null;
  if (row.kind === 'section' || mode === 'section') return sectionColor;
  switch (mode) {
    case 'type':
      return TYPE_TINT[row.kind];
    case 'issues': {
      const scores = inputs.issueScores;
      if (!scores || !inputs.issueColor) return null;
      const score = row.kind === 'shot' ? scores.shotScore[row.g] : scores.stationScore[row.g];
      // A clean row stays untinted so the problems stand out.
      return score && score > 0 ? inputs.issueColor(score) : null;
    }
    case 'depth': {
      if (!inputs.depthColor) return null;
      const d = ctx.data;
      const value =
        row.kind === 'shot' ? (d.shotDepthIn[row.g]! + d.shotDepthOut[row.g]!) / 2 : d.stationCoords[row.g * 3 + 2]!;
      return Number.isFinite(value) ? inputs.depthColor(value) : null;
    }
    case 'distance': {
      if (!inputs.distance) return null;
      const value =
        row.kind === 'shot'
          ? shotDistance(inputs, row.sectionId, row.shotIndex)
          : (inputs.distance.index.get(`${row.sectionId}:${row.stationIndex}`) ?? null);
      return value === null || value < 0 ? null : inputs.distance.color(value);
    }
    default:
      return null;
  }
}

/** The data extent the depth ramp spans: station z and every shot depth,
 *  exactly as the map computes it (falls back to 0..50 like the map). */
export function depthExtent(ctx: Pick<DataModelCtx, 'data'>): { min: number; max: number } {
  const d = ctx.data;
  let min = Infinity;
  let max = -Infinity;
  for (let i = 2; i < d.stationCoords.length; i += 3) {
    const z = d.stationCoords[i]!;
    if (z < min) min = z;
    if (z > max) max = z;
  }
  for (let i = 0; i < d.shotCount; i++) {
    const a = d.shotDepthIn[i]!;
    const b = d.shotDepthOut[i]!;
    if (a < min) min = a;
    if (a > max) max = a;
    if (b < min) min = b;
    if (b > max) max = b;
  }
  if (!Number.isFinite(min) || !Number.isFinite(max)) return { min: 0, max: 50 };
  return { min, max };
}

// ─── Legend ──────────────────────────────────────────────────────────────

export interface TintLegend {
  mode: TintMode;
  /** 'gradient': a ramp from `min` to `max`; 'swatches': named colours. */
  kind: 'gradient' | 'swatches';
  min?: string;
  max?: string;
  /** Ramp stops, offsets 0..1 ascending. */
  stops?: { offset: number; color: string }[];
  swatches?: { label: string; color: string }[];
  /** Extra line (e.g. "Unreachable rows stay untinted"). */
  note?: string;
}

export interface LegendFormat {
  t: (key: string, opts?: Record<string, unknown>) => string;
  /** Format a length / depth in metres for the min / max labels. */
  metres: (value: number) => string;
  depthRange: { min: number; max: number } | null;
  distanceMax: number | null;
}

function sampleStops(color: (v: number) => string | null, min: number, max: number, count = 9) {
  const stops: { offset: number; color: string }[] = [];
  for (let k = 0; k < count; k++) {
    const offset = k / (count - 1);
    const c = color(min + (max - min) * offset);
    if (c) stops.push({ offset, color: c });
  }
  return stops;
}

/**
 * Legend for the toolbar's tint menu: a gradient with min / max labels for the
 * ramps, swatches for the type mode, null for 'none' and 'section' (every
 * section keeps its own colour; the sections picker shows them).
 */
export function tintLegend(mode: TintMode, inputs: TintInputs, f: LegendFormat): TintLegend | null {
  switch (mode) {
    case 'type':
      return {
        mode,
        kind: 'swatches',
        swatches: [
          { label: f.t('dataWorkspace.rowType.station'), color: TYPE_TINT.station },
          { label: f.t('dataWorkspace.rowType.shot'), color: TYPE_TINT.shot },
        ],
      };
    case 'issues':
      if (!inputs.issueColor) return null;
      return {
        mode,
        kind: 'gradient',
        min: f.t('dataWorkspace.tint.legend.clear'),
        max: f.t('dataWorkspace.tint.legend.error'),
        stops: sampleStops(inputs.issueColor, 0, 3),
        note: f.t('dataWorkspace.tint.legend.issuesNote'),
      };
    case 'depth': {
      if (!inputs.depthColor || !f.depthRange) return null;
      const { min, max } = f.depthRange;
      return { mode, kind: 'gradient', min: f.metres(min), max: f.metres(max), stops: sampleStops(inputs.depthColor, min, max) };
    }
    case 'distance': {
      if (!inputs.distance || f.distanceMax === null) return null;
      return {
        mode,
        kind: 'gradient',
        min: f.metres(0),
        max: f.metres(f.distanceMax),
        stops: sampleStops(inputs.distance.color, 0, f.distanceMax),
        note: f.t('dataWorkspace.tint.legend.distanceNote'),
      };
    }
    default:
      return null;
  }
}
