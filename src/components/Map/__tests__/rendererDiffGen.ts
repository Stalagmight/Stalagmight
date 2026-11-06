/**
 * Seeded generator for the renderer incremental-vs-fresh differential (#576
 * FZ-ORACLE, rendererIncrementalDifferential.test.ts). NOT a test suite.
 *
 * A {@link Model} is a small synthetic survey (1-10 sections of 1-12 shots, every
 * station coordinate explicit) plus the display state MapCanvas feeds SurveyLayer.
 * {@link step} applies one weighted random change (an edit, a display toggle, a
 * no-op re-send, or several at once) and returns a one-line description for the
 * trace; {@link materialize} turns the model into the store slices the app holds
 * (legacy sections/stations -> `surveyDataFromLegacy`, the closure-adjusted pair).
 * Synthetic data only, all built in code from the seed.
 */
import type { Project } from '../../../types/ipc/Project';
import type { Station } from '../../../types/ipc/Station';
import type { StationRef } from '../../../types/ipc/StationRef';
import type { ClosureRef } from '../../../types/ipc/ClosureRef';
import type { ProjectWarning } from '../../../types/ipc/ProjectWarning';
import type { DistanceField } from '../../../types/ipc/DistanceField';
import type { GasReachField } from '../../../types/ipc/GasReachField';
import type { GpsOrigin } from '../../../types/ipc/GpsOrigin';
import type { DecimalPrecision } from '../../../types/ipc/DecimalPrecision';
import type { ShotKind } from '../../../types/ipc/ShotKind';
import type { ToolType } from '../../../store/uiStore';
import { surveyDataFromLegacy, surveyDataWithAdjustedStations, type SurveyData } from '../../../types/survey-data';
import { makeProject, makeSection, makeShot, makeStation } from '../../../test/factories/survey';
import { defaultConfig } from '../../../types/config';
import { FEATURE_ID_STRIDE } from '../../../utils/ids';
import { isNonInteractiveTool } from '../../../utils/toolUtils';

// ─── PRNG ─────────────────────────────────────────────────────────────────────

const M64 = (1n << 64n) - 1n;

/** splitmix64 (Steele, Lea, Flood 2014): the whole run is a pure function of the seed. */
export class Rng {
  private s: bigint;
  constructor(seed: number) {
    this.s = BigInt(seed) & M64;
  }
  private next(): bigint {
    this.s = (this.s + 0x9e3779b97f4a7c15n) & M64;
    let z = this.s;
    z = ((z ^ (z >> 30n)) * 0xbf58476d1ce4e5b9n) & M64;
    z = ((z ^ (z >> 27n)) * 0x94d049bb133111ebn) & M64;
    return z ^ (z >> 31n);
  }
  /** [0, 1) with 53 random bits. */
  float(): number {
    return Number(this.next() >> 11n) / 2 ** 53;
  }
  /** Integer in [0, n). */
  int(n: number): number {
    return Math.floor(this.float() * n);
  }
  /** Integer in [lo, hi]. */
  range(lo: number, hi: number): number {
    return lo + this.int(hi - lo + 1);
  }
  bool(p = 0.5): boolean {
    return this.float() < p;
  }
  pick<T>(xs: readonly T[]): T {
    return xs[this.int(xs.length)]!;
  }
  /** Rounded to 1/16 so coordinates stay exact in float32 and float64 alike. */
  coord(span: number): number {
    return Math.round((this.float() * 2 - 1) * span * 16) / 16;
  }
  weighted<K extends string>(w: Readonly<Record<K, number>>): K {
    const keys = Object.keys(w) as K[];
    let t = this.float() * keys.reduce((s, k) => s + w[k], 0);
    for (const k of keys) {
      t -= w[k];
      if (t < 0) return k;
    }
    return keys[keys.length - 1]!;
  }
}

// ─── Model ────────────────────────────────────────────────────────────────────

export interface ShotM {
  id: number;
  length: number;
  azimuth: number;
  depthIn: number;
  depthOut: number;
  kind: ShotKind;
  comment: string | null;
}

export interface StationM {
  x: number;
  y: number;
  z: number;
  zeroDatum: boolean;
  comment: string | null;
}

export interface SectionM {
  id: number;
  name: string | null;
  color: string;
  shots: ShotM[];
  /** `shots.length + 1` stations, index i = station_index i. */
  stations: StationM[];
  nextShotId: number;
  /** False: the section has shots but no computed stations (not drawn). */
  placed: boolean;
}

/** (section id, station index). */
export interface Ref {
  s: number;
  i: number;
}

export interface Display {
  geometryMode: 'measured' | 'closureAdjusted';
  /** Null: no adjusted geometry in the store; else the perturbation seed. */
  adjustSeed: number | null;
  gpsOrigin: GpsOrigin | null;
  depthOn: boolean;
  depthRange: { min: number; max: number };
  severityOn: boolean;
  warnings: ProjectWarning[];
  /** Never undefined: MapCanvas's useDecimalPrecision falls back to the default. */
  precision: DecimalPrecision;
  /** Palette index; `themeEpoch` bumps on every apply (a fresh theme object). */
  theme: number;
  themeEpoch: number;
  distanceOn: boolean;
  distanceField: DistanceField | null;
  gasPlan: boolean;
  gasOriginChosen: boolean;
  gasField: GasReachField | null;
  anchored: ReadonlySet<number>;
  immovableStations: ReadonlySet<number>;
  immovableShots: ReadonlySet<number>;
  hoverSection: number | null;
  hoverSegment: number[];
  selectSection: number | null;
  showShots: boolean;
  showStations: boolean;
  showShotData: boolean;
  showStationLabels: boolean;
  showSectionNames: boolean;
  showStationComments: boolean;
  showShotComments: boolean;
  showReferenceShots: boolean;
  showJumps: boolean;
  tool: ToolType;
  hiddenComment: { type: 'station' | 'shot'; sectionId: number; id: number } | null;
}

export interface Model {
  sections: SectionM[];
  nextSectionId: number;
  merges: Array<[Ref, Ref]>;
  closures: Array<{ id: number; a: Ref; b: Ref }>;
  nextClosureId: number;
  /** Bumped on every store-changing step (project.modified_at). */
  version: number;
  /** Per-seed: the store lists stations in section order, or reversed. */
  reverseStations: boolean;
  display: Display;
}

export interface StoreState {
  project: Project;
  stations: Station[];
  surveyData: SurveyData;
  stationsAdjusted: Station[] | null;
  surveyDataAdjusted: SurveyData | null;
}

const COLORS = ['#3388ff', '#ff5500', '#22aa44', '#aa22cc', '#ddcc00', '#00bbbb', '#ff3388', '#888888'];
const NAME_CHARS = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789 _-.';
const KINDS: readonly ShotKind[] = ['Normal', 'Normal', 'Normal', 'Jump', 'Reference'];
const TOOLS: readonly ToolType[] = ['select', 'select', 'pan', 'comment', 'move_comment', 'move_segment', 'move_station', 'measure'];
const PRECISIONS: readonly DecimalPrecision[] = [
  { length: 2, coordinates: 2, depth: 1, azimuth: 1, inclination: 1, gps: 6 },
  { length: 1, coordinates: 1, depth: 0, azimuth: 0, inclination: 0, gps: 5 },
  { length: 3, coordinates: 3, depth: 2, azimuth: 2, inclination: 2, gps: 7 },
];

function text(r: Rng, min: number, max: number): string {
  let s = '';
  const n = r.range(min, max);
  for (let i = 0; i < n; i++) s += r.pick([...NAME_CHARS]);
  return s.trim() === '' ? 'X' : s;
}

function newShot(r: Rng, sec: SectionM): ShotM {
  const depthIn = r.range(0, 40);
  return {
    id: sec.nextShotId++,
    length: r.range(2, 15) + r.range(0, 7) / 8,
    azimuth: r.range(0, 359),
    depthIn,
    depthOut: Math.max(0, depthIn + r.range(-3, 3)),
    kind: r.pick(KINDS),
    comment: r.bool(0.15) ? text(r, 1, 12) : null,
  };
}

function walk(r: Rng, from: StationM, shot: ShotM): StationM {
  const a = (shot.azimuth * Math.PI) / 180;
  return {
    x: Math.round((from.x + shot.length * Math.sin(a)) * 16) / 16,
    y: Math.round((from.y + shot.length * Math.cos(a)) * 16) / 16,
    z: -shot.depthOut,
    zeroDatum: false,
    comment: r.bool(0.1) ? text(r, 1, 10) : null,
  };
}

/** A new section starting at `start` (or a random spot), 1-12 shots. */
function newSection(r: Rng, m: Model, start: StationM | null): SectionM {
  const sec: SectionM = {
    id: m.nextSectionId++,
    name: r.bool(0.85) ? `Sec${text(r, 1, 6)}` : null,
    color: r.pick(COLORS),
    shots: [],
    stations: [],
    nextShotId: 1,
    placed: r.bool(0.95),
  };
  const s0: StationM = start
    ? { ...start, zeroDatum: false, comment: null }
    : { x: r.coord(300), y: r.coord(300), z: -r.range(0, 30), zeroDatum: r.bool(0.3), comment: null };
  sec.stations.push(s0);
  const n = r.range(1, 12);
  for (let i = 0; i < n; i++) {
    const shot = newShot(r, sec);
    sec.shots.push(shot);
    sec.stations.push(walk(r, sec.stations[i]!, shot));
  }
  return sec;
}

const defaultDisplay = (): Display => ({
  geometryMode: 'measured',
  adjustSeed: null,
  gpsOrigin: null,
  depthOn: false,
  depthRange: { min: 0, max: 50 },
  severityOn: false,
  warnings: [],
  precision: defaultConfig.units.decimal_precision,
  theme: 0,
  themeEpoch: 0,
  distanceOn: false,
  distanceField: null,
  gasPlan: false,
  gasOriginChosen: false,
  gasField: null,
  anchored: new Set(),
  immovableStations: new Set(),
  immovableShots: new Set(),
  hoverSection: null,
  hoverSegment: [],
  selectSection: null,
  showShots: true,
  showStations: true,
  showShotData: false,
  showStationLabels: false,
  showSectionNames: true,
  showStationComments: false,
  showShotComments: false,
  showReferenceShots: true,
  showJumps: true,
  tool: 'select',
  hiddenComment: null,
});

/** The seed's starting survey (1-6 sections, mostly chained) + default display. */
export function initialModel(r: Rng): Model {
  const m: Model = {
    sections: [],
    nextSectionId: 1,
    merges: [],
    closures: [],
    nextClosureId: 1,
    version: 0,
    reverseStations: r.bool(0.25),
    display: defaultDisplay(),
  };
  const n = r.range(1, 6);
  for (let i = 0; i < n; i++) addSection(r, m);
  return m;
}

// ─── Materialize ──────────────────────────────────────────────────────────────

const refKey = (r: Ref): string => `${r.s}:${r.i}`;
const toRef = (r: Ref): StationRef => ({ section_id: r.s, station_index: r.i });

/** Deterministic small adjustment for (seed, section, index): about half the
 *  stations move by up to 2 m, the rest stay put. */
function adjustOf(seed: number, s: number, i: number): [number, number, number] {
  let h = Math.imul(seed ^ 0x9e3779b9, 0x85ebca6b) ^ Math.imul(s + 1, 0xc2b2ae35) ^ Math.imul(i + 7, 0x27d4eb2f);
  h = Math.imul(h ^ (h >>> 15), 0x2c1b3c6d) >>> 0;
  if (h % 2 === 0) return [0, 0, 0];
  return [((h >>> 4) % 33) / 16 - 1, ((h >>> 12) % 33) / 16 - 1, ((h >>> 20) % 17) / 16 - 0.5];
}

/** The store slices the app holds for this model: fresh objects every call. */
export function materialize(m: Model): StoreState {
  const merged = new Map<string, StationRef[]>();
  const link = (a: Ref, b: Ref): void => {
    merged.set(refKey(a), [...(merged.get(refKey(a)) ?? []), toRef(b)]);
  };
  for (const [a, b] of m.merges) {
    link(a, b);
    link(b, a);
  }
  const closures = new Map<string, ClosureRef[]>();
  for (const c of m.closures) {
    for (const [x, y] of [[c.a, c.b], [c.b, c.a]] as const) {
      const ref: ClosureRef = { closure_id: c.id, station_ref: toRef(y), via_station: null, is_transitive: false };
      closures.set(refKey(x), [...(closures.get(refKey(x)) ?? []), ref]);
    }
  }
  const sections = m.sections.map((sec) =>
    makeSection({
      id: sec.id,
      name: sec.name,
      color: sec.color,
      next_shot_id: sec.nextShotId,
      shots: sec.shots.map((sh) =>
        makeShot({
          id: sh.id,
          azimuth_in: sh.azimuth,
          azimuth_out: (sh.azimuth + 180) % 360,
          length: sh.length,
          depth_in: sh.depthIn,
          depth_out: sh.depthOut,
          kind: sh.kind,
          comment: sh.comment,
        }),
      ),
    }),
  );
  const order = m.reverseStations ? [...m.sections].reverse() : m.sections;
  const stations: Station[] = [];
  for (const sec of order) {
    if (!sec.placed) continue;
    sec.stations.forEach((st, i) => {
      const key = `${sec.id}:${i}`;
      const cw = closures.get(key);
      stations.push(
        makeStation({
          station_ref: { section_id: sec.id, station_index: i },
          coordinates: { x: st.x, y: st.y, z: st.z },
          is_zero_datum: st.zeroDatum,
          merged_with: merged.get(key) ?? [],
          ...(cw ? { closures_with: cw } : {}),
          ...(st.comment !== null ? { comment: st.comment } : {}),
        }),
      );
    });
  }
  const project = makeProject({ sections, modified_at: `2026-01-01T00:00:00.${String(m.version).padStart(6, '0')}Z` });
  const surveyData = surveyDataFromLegacy(sections, stations);
  const seed = m.display.adjustSeed;
  const stationsAdjusted =
    seed === null
      ? null
      : stations.map((st) => {
          const [dx, dy, dz] = adjustOf(seed, st.station_ref.section_id, st.station_ref.station_index);
          const c = st.coordinates;
          return { ...st, coordinates: { x: c.x + dx, y: c.y + dy, z: c.z + dz } };
        });
  return {
    project,
    stations,
    surveyData,
    stationsAdjusted,
    surveyDataAdjusted: stationsAdjusted ? surveyDataWithAdjustedStations(surveyData, stationsAdjusted) : null,
  };
}

// ─── Steps ────────────────────────────────────────────────────────────────────

export interface StepResult {
  desc: string;
  /** The survey (store) changed: the driver re-materializes and sets the store. */
  store: boolean;
}

/** Station refs whose section is placed (drawn). */
function placedRefs(m: Model): Ref[] {
  const out: Ref[] = [];
  for (const sec of m.sections) if (sec.placed) sec.stations.forEach((_, i) => out.push({ s: sec.id, i }));
  return out;
}

function stationAt(m: Model, r: Ref): StationM | undefined {
  return m.sections.find((s) => s.id === r.s)?.stations[r.i];
}

/** Keep merges / closures pointing at the same stations after section `sid`
 *  gained a station at `at` (delta +1) or lost the one at `at` (delta -1). */
function remapRefs(m: Model, sid: number, at: number, delta: 1 | -1): void {
  const fix = (r: Ref): Ref | null => {
    if (r.s !== sid || r.i < at) return r;
    if (delta === -1 && r.i === at) return null;
    return { s: r.s, i: r.i + delta };
  };
  m.merges = m.merges.flatMap(([a, b]) => {
    const fa = fix(a);
    const fb = fix(b);
    return fa && fb ? [[fa, fb] as [Ref, Ref]] : [];
  });
  m.closures = m.closures.flatMap((c) => {
    const fa = fix(c.a);
    const fb = fix(c.b);
    return fa && fb ? [{ id: c.id, a: fa, b: fb }] : [];
  });
}

function addSection(r: Rng, m: Model): string {
  const refs = placedRefs(m);
  const connect = refs.length > 0 && r.bool(0.75);
  const from = connect ? r.pick(refs) : null;
  const sec = newSection(r, m, from ? stationAt(m, from)! : null);
  m.sections.splice(r.int(m.sections.length + 1), 0, sec);
  if (from) m.merges.push([from, { s: sec.id, i: 0 }]);
  return `addSection #${sec.id} shots=${sec.shots.length}${from ? ` at ${refKey(from)}` : ''}${sec.placed ? '' : ' unplaced'}`;
}

type EditKind =
  | 'moveStation' | 'moveDepth' | 'shotKind' | 'sectionColor' | 'shotDepth' | 'zeroDatum'
  | 'addShotEnd' | 'addShotMid' | 'delShotEnd' | 'delShotMid' | 'addSection' | 'delSection'
  | 'merge' | 'unmerge' | 'closure' | 'sectionOffset' | 'massEdit' | 'comment' | 'rename'
  | 'placeToggle' | 'noop';

type DisplayKind =
  | 'gps' | 'geometryMode' | 'adjusted' | 'depth' | 'depthRange' | 'severity' | 'warnings'
  | 'theme' | 'precision' | 'distance' | 'gas' | 'dim' | 'highlight' | 'labelToggle'
  | 'refJump' | 'tool' | 'hiddenComment';

const EDIT_WEIGHTS: Readonly<Record<EditKind, number>> = {
  moveStation: 14, moveDepth: 4, shotKind: 5, sectionColor: 5, shotDepth: 4, zeroDatum: 3,
  addShotEnd: 4, addShotMid: 3, delShotEnd: 3, delShotMid: 3, addSection: 3, delSection: 2,
  merge: 3, unmerge: 2, closure: 2, sectionOffset: 3, massEdit: 2, comment: 3, rename: 2,
  placeToggle: 1, noop: 4,
};

const DISPLAY_WEIGHTS: Readonly<Record<DisplayKind, number>> = {
  gps: 3, geometryMode: 3, adjusted: 2, depth: 3, depthRange: 2, severity: 2, warnings: 2,
  theme: 2, precision: 1, distance: 4, gas: 4, dim: 5, highlight: 5, labelToggle: 3,
  refJump: 3, tool: 2, hiddenComment: 2,
};

function pickSection(r: Rng, m: Model): SectionM | undefined {
  return m.sections.length > 0 ? r.pick(m.sections) : undefined;
}

function edit(r: Rng, m: Model, kind: EditKind): string {
  const sec = pickSection(r, m);
  const refs = placedRefs(m);
  switch (kind) {
    case 'moveStation': {
      if (refs.length === 0) return 'moveStation (none)';
      const n = r.range(1, 3);
      const moved: string[] = [];
      for (let k = 0; k < n; k++) {
        const ref = r.pick(refs);
        const st = stationAt(m, ref)!;
        st.x += r.coord(r.bool(0.7) ? 2 : 40);
        st.y += r.coord(r.bool(0.7) ? 2 : 40);
        if (r.bool(0.3)) st.z += r.coord(3);
        moved.push(refKey(ref));
      }
      return `moveStation ${moved.join(',')}`;
    }
    case 'moveDepth': {
      if (refs.length === 0) return 'moveDepth (none)';
      const ref = r.pick(refs);
      stationAt(m, ref)!.z += r.coord(5) || 1;
      return `moveDepth ${refKey(ref)}`;
    }
    case 'shotKind': {
      if (!sec || sec.shots.length === 0) return 'shotKind (none)';
      const shot = r.pick(sec.shots);
      shot.kind = r.pick((['Normal', 'Jump', 'Reference'] as const).filter((k) => k !== shot.kind));
      return `shotKind #${sec.id}/${shot.id}=${shot.kind}`;
    }
    case 'sectionColor':
      if (!sec) return 'sectionColor (none)';
      sec.color = r.pick(COLORS.filter((c) => c !== sec.color));
      return `sectionColor #${sec.id}=${sec.color}`;
    case 'shotDepth': {
      if (!sec || sec.shots.length === 0) return 'shotDepth (none)';
      const shot = r.pick(sec.shots);
      shot.depthIn = Math.max(0, shot.depthIn + r.range(-4, 4));
      shot.depthOut = Math.max(0, shot.depthOut + r.range(-4, 4));
      return `shotDepth #${sec.id}/${shot.id}`;
    }
    case 'zeroDatum': {
      if (refs.length === 0) return 'zeroDatum (none)';
      const ref = r.pick(refs);
      const st = stationAt(m, ref)!;
      st.zeroDatum = !st.zeroDatum;
      return `zeroDatum ${refKey(ref)}=${st.zeroDatum}`;
    }
    case 'addShotEnd':
    case 'addShotMid': {
      if (!sec) return `${kind} (none)`;
      const at = kind === 'addShotEnd' || sec.shots.length === 0 ? sec.shots.length : r.int(sec.shots.length);
      const shot = newShot(r, sec);
      sec.shots.splice(at, 0, shot);
      const base = sec.stations[at]!;
      remapRefs(m, sec.id, at + 1, 1);
      sec.stations.splice(at + 1, 0, walk(r, base, shot));
      return `${kind} #${sec.id} at=${at} id=${shot.id}`;
    }
    case 'delShotEnd':
    case 'delShotMid': {
      if (!sec || sec.shots.length <= 1) return `${kind} (none)`;
      const at = kind === 'delShotEnd' ? sec.shots.length - 1 : r.int(sec.shots.length);
      const [gone] = sec.shots.splice(at, 1);
      sec.stations.splice(at + 1, 1);
      remapRefs(m, sec.id, at + 1, -1);
      return `${kind} #${sec.id} at=${at} id=${gone!.id}`;
    }
    case 'addSection':
      if (m.sections.length >= 10) return 'addSection (full)';
      return addSection(r, m);
    case 'delSection': {
      if (!sec || m.sections.length <= 1) return 'delSection (none)';
      m.sections = m.sections.filter((s) => s !== sec);
      m.merges = m.merges.filter(([a, b]) => a.s !== sec.id && b.s !== sec.id);
      m.closures = m.closures.filter((c) => c.a.s !== sec.id && c.b.s !== sec.id);
      return `delSection #${sec.id}`;
    }
    case 'merge': {
      if (refs.length < 2) return 'merge (none)';
      const a = r.pick(refs);
      const b = r.pick(refs.filter((x) => x.s !== a.s));
      if (!b) return 'merge (one section)';
      const sa = stationAt(m, a)!;
      const sb = stationAt(m, b)!;
      sb.x = sa.x;
      sb.y = sa.y;
      sb.z = sa.z;
      m.merges.push([a, b]);
      return `merge ${refKey(a)}+${refKey(b)}`;
    }
    case 'unmerge': {
      if (m.merges.length === 0) return 'unmerge (none)';
      const k = r.int(m.merges.length);
      const [a, b] = m.merges[k]!;
      m.merges.splice(k, 1);
      return `unmerge ${refKey(a)}+${refKey(b)}`;
    }
    case 'closure': {
      if (refs.length < 2) return 'closure (none)';
      const a = r.pick(refs);
      const b = r.pick(refs);
      m.closures.push({ id: m.nextClosureId++, a, b });
      // The backend re-solves on a new closure: the adjusted geometry changes.
      if (m.display.adjustSeed !== null) m.display.adjustSeed = r.int(1 << 30);
      return `closure ${refKey(a)}~${refKey(b)}`;
    }
    case 'sectionOffset': {
      if (!sec) return 'sectionOffset (none)';
      const dx = r.coord(r.bool(0.5) ? 1 : 30);
      const dy = r.coord(r.bool(0.5) ? 1 : 30);
      const dz = r.bool(0.3) ? r.coord(2) : 0;
      for (const st of sec.stations) {
        st.x += dx;
        st.y += dy;
        st.z += dz;
      }
      return `sectionOffset #${sec.id} (${dx},${dy},${dz})`;
    }
    case 'massEdit': {
      // Touch more than half the sections: forces the incremental path's bail.
      const color = r.bool(0.5);
      for (const s of m.sections) {
        if (color) s.color = r.pick(COLORS.filter((c) => c !== s.color));
        else if (s.stations.length > 1) s.stations[r.range(1, s.stations.length - 1)]!.x += r.coord(1) || 0.5;
      }
      return `massEdit ${color ? 'colors' : 'nudge'} x${m.sections.length}`;
    }
    case 'comment': {
      if (!sec) return 'comment (none)';
      if (r.bool(0.5) && sec.shots.length > 0) {
        const shot = r.pick(sec.shots);
        shot.comment = shot.comment === null || r.bool(0.5) ? text(r, 1, 14) : null;
        return `shotComment #${sec.id}/${shot.id}=${shot.comment ?? 'null'}`;
      }
      const i = r.int(sec.stations.length);
      const st = sec.stations[i]!;
      st.comment = st.comment === null || r.bool(0.5) ? text(r, 1, 14) : null;
      return `stationComment #${sec.id}:${i}=${st.comment ?? 'null'}`;
    }
    case 'rename':
      if (!sec) return 'rename (none)';
      sec.name = r.bool(0.85) ? `Sec${text(r, 1, 8)}` : null;
      return `rename #${sec.id}=${sec.name ?? 'null'}`;
    case 'placeToggle':
      if (!sec) return 'placeToggle (none)';
      sec.placed = !sec.placed;
      return `placeToggle #${sec.id}=${sec.placed}`;
    case 'noop':
      return 'noop re-send';
  }
}

function randomSubset(r: Rng, xs: number[], p: number): Set<number> {
  return new Set(xs.filter(() => r.bool(p)));
}

function stationFeatureIds(m: Model): number[] {
  return placedRefs(m).map((x) => x.s * FEATURE_ID_STRIDE + x.i);
}

function shotFeatureIds(m: Model): number[] {
  return m.sections.flatMap((s) => s.shots.map((sh) => s.id * FEATURE_ID_STRIDE + sh.id));
}

/** A distance field over the current stations (some missing, some unreachable). */
function distanceField(r: Rng, m: Model): DistanceField {
  const refs = placedRefs(m).filter(() => r.bool(0.9));
  const distances = refs.map(() => (r.bool(0.1) ? null : r.range(0, 4000) / 8));
  return { station_refs: refs.map(toRef), distances, max_reachable: r.bool(0.1) ? 0 : 500 };
}

/** A gas reach field with a random reachable mask (only the fields the builder reads). */
function gasField(r: Rng, m: Model): GasReachField {
  const refs = placedRefs(m).filter(() => r.bool(0.9));
  return {
    station_refs: refs.map(toRef),
    gas_to_reach: refs.map(() => (r.bool(0.1) ? null : r.range(0, 3000) / 4)),
    reachable: refs.map(() => r.bool(0.7)),
    budget_litres: r.range(100, 900),
  } as unknown as GasReachField;
}

function warnings(r: Rng, m: Model): ProjectWarning[] {
  const shots = m.sections.flatMap((s) => s.shots.map((sh, i) => ({ s, sh, i })));
  const n = shots.length === 0 ? 0 : r.range(0, 4);
  return Array.from({ length: n }, () => {
    const { s, sh, i } = r.pick(shots);
    return {
      sectionId: s.id,
      shotId: sh.id,
      shotIndex: i,
      code: 'W',
      message: 'synthetic',
      severity: r.bool(0.4) ? 'error' : 'warning',
      isResolved: r.bool(0.15),
    };
  });
}

function display(r: Rng, m: Model, kind: DisplayKind): string {
  const d = m.display;
  const ids = m.sections.map((s) => s.id);
  switch (kind) {
    case 'gps':
      d.gpsOrigin = r.bool(0.3) ? null : { latitude: r.range(-600, 600) / 10, longitude: r.range(-1790, 1790) / 10 };
      return `gps ${d.gpsOrigin ? `${d.gpsOrigin.latitude},${d.gpsOrigin.longitude}` : 'null'}`;
    case 'geometryMode':
      d.geometryMode = d.geometryMode === 'measured' ? 'closureAdjusted' : 'measured';
      return `geometryMode ${d.geometryMode}`;
    case 'adjusted':
      d.adjustSeed = d.adjustSeed !== null && r.bool(0.4) ? null : r.int(1 << 30);
      return `adjusted ${d.adjustSeed ?? 'null'}`;
    case 'depth':
      d.depthOn = !d.depthOn;
      return `depth ${d.depthOn}`;
    case 'depthRange': {
      const min = r.range(0, 20);
      d.depthRange = { min, max: r.bool(0.1) ? min : min + r.range(1, 60) };
      return `depthRange ${d.depthRange.min}..${d.depthRange.max}`;
    }
    case 'severity':
      d.severityOn = !d.severityOn;
      return `severity ${d.severityOn}`;
    case 'warnings':
      d.warnings = warnings(r, m);
      return `warnings x${d.warnings.length}`;
    case 'theme':
      d.theme = r.int(2);
      d.themeEpoch++;
      return `theme ${d.theme}`;
    case 'precision':
      // A config reload hands a fresh object even for the same values.
      d.precision = { ...(r.bool(0.2) ? defaultConfig.units.decimal_precision : r.pick(PRECISIONS)) };
      return `precision length=${d.precision.length} depth=${d.precision.depth}`;
    case 'distance':
      if (!d.distanceOn || r.bool(0.3)) {
        d.distanceOn = !d.distanceOn || r.bool(0.5);
        d.distanceField = d.distanceOn && r.bool(0.85) ? distanceField(r, m) : null;
      } else {
        d.distanceField = distanceField(r, m);
      }
      return `distance ${d.distanceOn} field=${d.distanceField ? d.distanceField.station_refs.length : 'null'}`;
    case 'gas': {
      const c = r.int(3);
      if (c === 0) d.gasPlan = !d.gasPlan;
      else if (c === 1) d.gasOriginChosen = !d.gasOriginChosen;
      d.gasField = d.gasPlan && d.gasOriginChosen && r.bool(0.85) ? gasField(r, m) : null;
      return `gas plan=${d.gasPlan} origin=${d.gasOriginChosen} field=${d.gasField ? 'new' : 'null'}`;
    }
    case 'dim': {
      const c = r.int(4);
      if (c === 0) d.anchored = randomSubset(r, ids, 0.4);
      else if (c === 1) {
        d.immovableStations = randomSubset(r, stationFeatureIds(m), 0.2);
        d.immovableShots = randomSubset(r, shotFeatureIds(m), 0.2);
      } else if (c === 2) d.anchored = new Set();
      else {
        d.immovableStations = new Set();
        d.immovableShots = new Set();
      }
      return `dim anchored=${d.anchored.size} immovable=${d.immovableStations.size}/${d.immovableShots.size}`;
    }
    case 'highlight': {
      const c = r.int(4);
      const any = (): number | null => (ids.length > 0 && r.bool(0.8) ? r.pick(ids) : null);
      if (c === 0) d.hoverSection = any();
      else if (c === 1) d.hoverSegment = [...randomSubset(r, ids, 0.4)];
      else if (c === 2) d.selectSection = any();
      else {
        d.hoverSection = null;
        d.hoverSegment = [];
        d.selectSection = null;
      }
      return `highlight hover=${d.hoverSection} segment=[${d.hoverSegment.join(',')}] select=${d.selectSection}`;
    }
    case 'labelToggle': {
      const k = r.pick(['showShotData', 'showStationLabels', 'showSectionNames', 'showStationComments', 'showShotComments', 'showShots', 'showStations'] as const);
      d[k] = !d[k];
      return `labelToggle ${k}=${d[k]}`;
    }
    case 'refJump': {
      const k = r.pick(['showReferenceShots', 'showJumps'] as const);
      d[k] = !d[k];
      return `refJump ${k}=${d[k]}`;
    }
    case 'tool':
      d.tool = r.pick(TOOLS);
      // SurveyLayer.setActiveTool drops the hover on a non-interactive tool and
      // tells MapCanvas (onFeatureHover(false) ...), whose hover state follows.
      if (isNonInteractiveTool(d.tool)) d.hoverSection = null;
      return `tool ${d.tool}`;
    case 'hiddenComment': {
      const sec = pickSection(r, m);
      if (!sec || r.bool(0.3)) d.hiddenComment = null;
      else if (r.bool(0.5) && sec.shots.length > 0) d.hiddenComment = { type: 'shot', sectionId: sec.id, id: r.pick(sec.shots).id };
      else d.hiddenComment = { type: 'station', sectionId: sec.id, id: r.int(sec.stations.length) };
      return `hiddenComment ${d.hiddenComment ? `${d.hiddenComment.type}#${d.hiddenComment.sectionId}/${d.hiddenComment.id}` : 'null'}`;
    }
  }
}

/** Apply one random step (a single edit / display change, or 2-4 of them that
 *  land in ONE commit + flush). Every step bumps nothing but what it touches. */
export function step(r: Rng, m: Model): StepResult {
  const one = (): StepResult => {
    if (r.bool(0.55)) {
      const k = r.weighted(EDIT_WEIGHTS);
      return { desc: edit(r, m, k), store: true };
    }
    // Display steps that change the store-backed geometry (adjusted pair) say so.
    const k = r.weighted(DISPLAY_WEIGHTS);
    return { desc: display(r, m, k), store: k === 'adjusted' };
  };
  let res: StepResult;
  if (r.bool(0.12)) {
    const parts = Array.from({ length: r.range(2, 4) }, one);
    res = { desc: `multi[${parts.map((p) => p.desc).join(' | ')}]`, store: parts.some((p) => p.store) };
  } else {
    res = one();
  }
  if (res.store) m.version++;
  return res;
}
