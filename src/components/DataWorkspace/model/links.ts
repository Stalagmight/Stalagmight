/**
 * Station topology for the Data workspace - merges, closures, zero datums,
 * branch starts - and the link RAIL drawn in the row-name gutter.
 *
 * `stationLinks` is O(group size) per station off the SoA sparse maps
 * (`stationMergedWith` is the transitive merge group, `stationClosuresWith`
 * the closures touching the station) plus a per-project closure index for the
 * misclosure figures. `buildRail` is one O(rows) pass over the DISPLAYED rows.
 */
import type { LoopClosure, StationRef } from '../../../types/project';
import type { Project } from '../../../types/project';
import { STATION_FLAG_IS_ZERO_DATUM } from '../../../types/survey-data';
import { shotKindFromFlags } from '../../../types/survey-data';
import type { DataEntityRow, DataModelCtx, DataRow } from '../types';
import { sectionOrderOf } from './ctx';
import { sectionRunPosition, shotRowKey, stationGlobalIndex, stationRowKey } from './rows';

export interface ClosureLink {
  closureId: string;
  /** The station at the other end of the closure. */
  partner: StationRef;
  /** The station the closure is attached through (a merge-group member), if any. */
  via: StationRef | null;
  transitive: boolean;
  /** Misclosure distance in metres, or null when the closure is unknown. */
  misclosure: number | null;
  /** Error in percent of the loop length, or null. */
  errorPct: number | null;
}

export interface StationLinks {
  /** The other members of the station's merge group. */
  merges: StationRef[];
  closures: ClosureLink[];
  zeroDatum: boolean;
  /** Merged with at least one other station. */
  junction: boolean;
  /** Station 0 merged with another section's station: the section continues
   *  from there (the parent's station). */
  branchFrom: StationRef | null;
  /** The section's last station merged with another section's station. */
  branchTo: StationRef | null;
}

const NO_LINKS: StationLinks = {
  merges: [],
  closures: [],
  zeroDatum: false,
  junction: false,
  branchFrom: null,
  branchTo: null,
};

const closureIndexCache = new WeakMap<Project, Map<string, LoopClosure>>();
function closureIndex(project: Project): Map<string, LoopClosure> {
  let m = closureIndexCache.get(project);
  if (!m) {
    m = new Map((project.loop_closures ?? []).map((c) => [String(c.id), c]));
    closureIndexCache.set(project, m);
  }
  return m;
}

const sameRef = (a: StationRef, sectionId: number, stationIndex: number) =>
  a.section_id === sectionId && a.station_index === stationIndex;

/** The parent a branch section continues from: an earlier-declared section
 *  first, then the lowest section order, then the lowest station index. */
function pickParent(ctx: DataModelCtx, own: number, refs: readonly StationRef[]): StationRef | null {
  const order = sectionOrderOf(ctx.data);
  const ownOrder = order.get(own) ?? 0;
  let best: StationRef | null = null;
  let bestKey: [number, number, number] | null = null;
  for (const r of refs) {
    if (r.section_id === own) continue;
    const o = order.get(r.section_id) ?? Number.MAX_SAFE_INTEGER;
    const key: [number, number, number] = [o < ownOrder ? 0 : 1, o, r.station_index];
    if (!bestKey || key[0] < bestKey[0] || (key[0] === bestKey[0] && (key[1] < bestKey[1] || (key[1] === bestKey[1] && key[2] < bestKey[2])))) {
      best = r;
      bestKey = key;
    }
  }
  return best;
}

export function stationLinks(ctx: DataModelCtx, sectionId: number, stationIndex: number, g: number): StationLinks {
  const data = ctx.data;
  const zeroDatum = ((data.stationFlags[g] ?? 0) & STATION_FLAG_IS_ZERO_DATUM) !== 0;
  const mergedRaw = data.stationMergedWith.get(g);
  const closuresRaw = data.stationClosuresWith.get(g);
  if (!zeroDatum && !mergedRaw && !closuresRaw) return NO_LINKS;
  const merges = (mergedRaw ?? []).filter((r) => !sameRef(r, sectionId, stationIndex));
  const index = closuresRaw ? closureIndex(ctx.project) : null;
  const closures: ClosureLink[] = (closuresRaw ?? []).map((c) => {
    const closure = index?.get(String(c.closure_id));
    return {
      closureId: String(c.closure_id),
      partner: c.station_ref,
      via: c.via_station,
      transitive: c.is_transitive,
      misclosure: closure ? closure.misclosure_distance : null,
      errorPct: closure ? closure.error_percentage : null,
    };
  });
  const lastIndex = ctx.sectionsById.get(sectionId)?.shotCount ?? -1;
  return {
    merges,
    closures,
    zeroDatum,
    junction: merges.length > 0,
    branchFrom: stationIndex === 0 ? pickParent(ctx, sectionId, merges) : null,
    branchTo: stationIndex === lastIndex && stationIndex > 0 ? pickParent(ctx, sectionId, merges) : null,
  };
}

/** Links of the station a row is on (shots have none). */
export function rowLinks(ctx: DataModelCtx, row: DataEntityRow): StationLinks {
  return row.kind === 'station' ? stationLinks(ctx, row.sectionId, row.stationIndex, row.g) : NO_LINKS;
}

/** Row key of a station reference (for navigation / highlighting). */
export function stationRefKey(ref: StationRef): string {
  return stationRowKey(ref.section_id, ref.station_index);
}

/** The rows a station is linked to: merge partners, then closure partners. */
export function linkedRowKeys(links: StationLinks): string[] {
  const keys = links.merges.map(stationRefKey);
  for (const c of links.closures) keys.push(stationRefKey(c.partner));
  return [...new Set(keys)];
}

/** The station-0 row key of a section continuing from a parent station. */
export function branchFromOfSection(ctx: DataModelCtx, sectionId: number): StationRef | null {
  const g = stationGlobalIndex(ctx.data).get(sectionId)?.get(0);
  return g === undefined ? null : stationLinks(ctx, sectionId, 0, g).branchFrom;
}

/** Shot row key from a shot's section + 0-based index (the SoA order). */
export function shotKeyAt(ctx: DataModelCtx, sectionId: number, shotIndex: number): string | null {
  const sec = ctx.sectionsById.get(sectionId);
  if (!sec || shotIndex < 0 || shotIndex >= sec.shotCount) return null;
  return shotRowKey(sectionId, ctx.data.shotIds[sec.shotOffset + shotIndex]!);
}

// ─── Rail ──────────────────────────────────────────────────────────────────

export type RailLine = 'solid' | 'dashed' | 'dotted';

/** How a line meets the row's top / bottom edge:
 *  - 'edge': runs to the edge (the neighbouring row continues it);
 *  - 'gap':  runs to the top edge with break marks - rows of this section
 *            were omitted between this row and the displayed one above;
 *  - 'cut':  stops short with break marks - this section's run continues,
 *            but no row of it is displayed on that side. */
export type RailEdge = 'edge' | 'gap' | 'cut';

export interface RailSegment {
  style: RailLine;
  edge: RailEdge;
}

export interface RailCell {
  /** The section's colour. */
  color: string;
  kind: 'header' | 'station' | 'shot';
  /** Station node: filled dot, or a hollow ring for a junction (merged station). */
  node: 'dot' | 'ring' | null;
  zeroDatum: boolean;
  closure: boolean;
  above: RailSegment | null;
  below: RailSegment | null;
  /** Branch elbow from the parent station (its section colour). On the header
   *  row it turns down into the lane (`down`); on station 0 it is a short
   *  connector into the node. */
  elbow: { parentColor: string; down: boolean } | null;
}

export function shotLineStyle(flags: number): RailLine {
  const kind = shotKindFromFlags(flags);
  return kind === 'Jump' ? 'dashed' : kind === 'Reference' ? 'dotted' : 'solid';
}

/**
 * Rail cells for the displayed rows, aligned with them; null when the view is
 * not in survey order (sorted - the rail would lie). Rows of one section are
 * contiguous in survey order, so "the previous / next displayed row of the
 * same section" is just the neighbour in the list.
 */
export function buildRail(rows: readonly DataRow[], ctx: DataModelCtx, surveyOrder: boolean): (RailCell | null)[] | null {
  if (!surveyOrder) return null;
  const data = ctx.data;
  const out: (RailCell | null)[] = new Array(rows.length).fill(null);
  const colorOf = (id: number) => ctx.sectionsById.get(id)?.color ?? 'var(--cc-text-secondary)';

  for (let i = 0; i < rows.length; i++) {
    const row = rows[i]!;
    const prev = rows[i - 1];
    const next = rows[i + 1];
    const sec = ctx.sectionsById.get(row.sectionId);
    if (!sec) continue;
    const color = colorOf(row.sectionId);

    if (row.kind === 'section') {
      const parent = branchFromOfSection(ctx, row.sectionId);
      const startsBelow = next?.kind === 'station' && next.sectionId === row.sectionId && next.stationIndex === 0;
      out[i] = {
        color,
        kind: 'header',
        node: null,
        zeroDatum: false,
        closure: false,
        above: null,
        below: null,
        elbow: parent ? { parentColor: colorOf(parent.section_id), down: startsBelow } : null,
      };
      continue;
    }

    const pos = sectionRunPosition(row);
    const maxPos = sec.shotCount * 2;
    const prevSame = prev && prev.kind !== 'section' && prev.sectionId === row.sectionId ? prev : null;
    const nextSame = next && next.kind !== 'section' && next.sectionId === row.sectionId ? next : null;

    // Shot i spans the row; station i sits between shot i-1 (above) and shot i (below).
    let styleAbove: RailLine | null = null;
    let styleBelow: RailLine | null = null;
    if (row.kind === 'shot') {
      styleAbove = styleBelow = shotLineStyle(data.shotFlags[row.g] ?? 0);
    } else {
      if (row.stationIndex > 0) styleAbove = shotLineStyle(data.shotFlags[sec.shotOffset + row.stationIndex - 1] ?? 0);
      if (row.stationIndex < sec.shotCount) styleBelow = shotLineStyle(data.shotFlags[sec.shotOffset + row.stationIndex] ?? 0);
    }

    let above: RailSegment | null = null;
    if (pos > 0 && styleAbove) {
      const adjacent = prevSame !== null && sectionRunPosition(prevSame) === pos - 1;
      above = { style: styleAbove, edge: adjacent ? 'edge' : prevSame ? 'gap' : 'cut' };
    }
    let below: RailSegment | null = null;
    if (pos < maxPos && styleBelow) below = { style: styleBelow, edge: nextSame ? 'edge' : 'cut' };

    let elbow: RailCell['elbow'] = null;
    let node: RailCell['node'] = null;
    let zeroDatum = false;
    let closure = false;
    if (row.kind === 'station') {
      const links = stationLinks(ctx, row.sectionId, row.stationIndex, row.g);
      node = links.junction ? 'ring' : 'dot';
      zeroDatum = links.zeroDatum;
      closure = links.closures.length > 0;
      if (links.branchFrom) {
        const headerAbove = prev?.kind === 'section' && prev.sectionId === row.sectionId;
        if (headerAbove) above = { style: 'solid', edge: 'edge' };
        else elbow = { parentColor: colorOf(links.branchFrom.section_id), down: false };
      }
    }
    out[i] = { color, kind: row.kind, node, zeroDatum, closure, above, below, elbow };
  }
  return out;
}
