/**
 * Determinism-test kit (#576 fuzzers, FZ-DET-TS).
 *
 * The `*.determinism.test.ts` files assert two things about the frontend's PURE
 * layers (snapshot canonicalisation, the binary-envelope decode, the WebGL
 * tessellators, label layout / reveal, the gas-reach masks):
 *
 *   1. the same input twice gives byte-identical output - also with a different
 *      input decoded or built in between, which is what catches module-level
 *      state (a memo, a cache keyed too loosely);
 *   2. the same CONTENT handed over in a different insertion order - a shuffled
 *      `Map` / `Set`, an object whose keys were inserted in another order, the
 *      parallel arrays of a set-like wire field permuted together - gives the
 *      same output, wherever the module's contract says that input is a set.
 *
 * Input order that the contract makes semantic (shot order in a section, the
 * global station order a buffer is laid out in) is NOT shuffled; each test file
 * says which inputs it treats as sets and cites where the order is semantic.
 *
 * Seeds: every file runs a FIXED seed list in CI - its regression seeds first,
 * then `1..STALAGMIGHT_DETERMINISM_RUNS` (default {@link DEFAULT_DETERMINISM_RUNS}).
 * Raise the env knob for a long run, e.g.
 *
 *     STALAGMIGHT_DETERMINISM_RUNS=2000 pnpm exec vitest run determinism
 *
 * A failure message always starts with `seed=<n>` and names the first differing
 * path / byte, so it is a repro on its own. All data here is synthetic.
 */
import {
  emptySurveyData,
  shotKindToFlags,
  STATION_FLAG_IS_ZERO_DATUM,
  type SectionMetadata,
  type SurveyData,
} from '../types/survey-data';
import { GlyphAtlas, SDF_BUFFER, type RasterizedGlyph } from '../components/Map/surveyLayer/customRender/glyphAtlas';

/** Env knob: how many generated seeds each determinism file runs. */
export const DETERMINISM_RUNS_ENV = 'STALAGMIGHT_DETERMINISM_RUNS';
/** Seeds per file when the knob is unset - small enough for the CI vitest job. */
export const DEFAULT_DETERMINISM_RUNS = 8;

/** A seeded PRNG in [0, 1) (mulberry32). Same seed, same sequence, everywhere. */
export function mulberry32(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

export type Rng = () => number;

/** Integer in `[lo, hi]` (inclusive). */
export function randInt(rng: Rng, lo: number, hi: number): number {
  return lo + Math.floor(rng() * (hi - lo + 1));
}

/** One element of a non-empty list. */
export function pick<T>(rng: Rng, items: readonly T[]): T {
  return items[Math.floor(rng() * items.length)]!;
}

/** The seed count `STALAGMIGHT_DETERMINISM_RUNS` asks for (default 8). */
export function determinismRuns(): number {
  const env = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env;
  const raw = env?.[DETERMINISM_RUNS_ENV];
  const n = raw === undefined ? Number.NaN : Number.parseInt(raw, 10);
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_DETERMINISM_RUNS;
}

/** The fixed seed list: regression seeds first (a seed that once failed stays in
 *  the CI set), then `1..determinismRuns()`. */
export function determinismSeeds(regressionSeeds: readonly number[] = []): number[] {
  const out = [...regressionSeeds];
  const seen = new Set(out);
  const runs = determinismRuns();
  for (let s = 1; s <= runs; s++) if (!seen.has(s)) out.push(s);
  return out;
}

/** A Fisher-Yates shuffled COPY. */
export function shuffled<T>(items: readonly T[], rng: Rng): T[] {
  const out = [...items];
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(rng() * (i + 1));
    const tmp = out[i]!;
    out[i] = out[j]!;
    out[j] = tmp;
  }
  return out;
}

/** A random permutation of `0..n-1`. */
export function permutation(n: number, rng: Rng): number[] {
  return shuffled(Array.from({ length: n }, (_, i) => i), rng);
}

/** The same entries, inserted in a shuffled order. */
export function shuffledMap<K, V>(map: ReadonlyMap<K, V>, rng: Rng): Map<K, V> {
  return new Map(shuffled([...map.entries()], rng));
}

/** The same members, inserted in a shuffled order. */
export function shuffledSet<T>(set: ReadonlySet<T>, rng: Rng): Set<T> {
  return new Set(shuffled([...set], rng));
}

function isPlainObject(value: unknown): value is Record<string, unknown> {
  if (typeof value !== 'object' || value === null) return false;
  const proto: unknown = Object.getPrototypeOf(value);
  return proto === Object.prototype || proto === null;
}

/**
 * A deep copy whose plain objects have their keys INSERTED in a shuffled order
 * (and whose Maps / Sets are re-inserted shuffled). Arrays keep their order -
 * array order is content - and every leaf is shared, not cloned. Note that JS
 * itself always enumerates integer-like keys ascending, so only the other keys'
 * order actually changes; that is exactly the freedom a JSON producer has.
 */
export function reKeyed<T>(value: T, rng: Rng): T {
  const walk = (v: unknown): unknown => {
    if (Array.isArray(v)) return v.map(walk);
    if (v instanceof Map) return new Map(shuffled([...v.entries()], rng).map(([k, x]) => [k, walk(x)]));
    if (v instanceof Set) return new Set(shuffled([...v], rng));
    if (isPlainObject(v)) {
      const out: Record<string, unknown> = {};
      for (const key of shuffled(Object.keys(v), rng)) out[key] = walk(v[key]);
      return out;
    }
    return v;
  };
  return walk(value) as T;
}

function show(value: unknown): string {
  if (typeof value === 'number') return Object.is(value, -0) ? '-0' : String(value);
  if (typeof value === 'bigint') return `${value}n`;
  if (value === undefined) return '<undefined>';
  let text: string;
  try {
    text = JSON.stringify(value) ?? String(value);
  } catch {
    text = String(value);
  }
  return text.length > 120 ? `${text.slice(0, 120)}...` : text;
}

function kindOf(value: unknown): string {
  if (value === null) return 'null';
  if (Array.isArray(value)) return 'array';
  if (ArrayBuffer.isView(value)) return value.constructor.name;
  if (value instanceof Map) return 'Map';
  if (value instanceof Set) return 'Set';
  return typeof value;
}

/** Element of a typed array (or byte of a DataView) at `i`, for a message. */
function elementAt(view: ArrayBufferView, i: number): string {
  const indexed = view as unknown as ArrayLike<number | bigint>;
  return show(indexed[i]);
}

/**
 * First byte at which two typed arrays differ, as a message
 * (`element 9 (byte 36): 0.5 vs 0.25`), or null when byte-identical. Bytes, not
 * values: a -0 / +0 or NaN-payload flip is a difference too.
 */
export function firstByteDiff(a: ArrayBufferView, b: ArrayBufferView): string | null {
  if (a.constructor !== b.constructor) return `${a.constructor.name} vs ${b.constructor.name}`;
  if (a.byteLength !== b.byteLength) return `byteLength ${a.byteLength} vs ${b.byteLength}`;
  const ua = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
  const ub = new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
  const width = (a as unknown as { BYTES_PER_ELEMENT?: number }).BYTES_PER_ELEMENT ?? 1;
  for (let i = 0; i < ua.length; i++) {
    if (ua[i] !== ub[i]) {
      const e = Math.floor(i / width);
      return `element ${e} (byte ${i}): ${elementAt(a, e)} vs ${elementAt(b, e)}`;
    }
  }
  return null;
}

/** A total order over Map keys / Set members of mixed primitive types. */
function compareKeys(x: unknown, y: unknown): number {
  const tx = typeof x;
  const ty = typeof y;
  if (tx !== ty) return tx < ty ? -1 : 1;
  if (tx === 'number' || tx === 'bigint') return (x as number) < (y as number) ? -1 : (x as number) > (y as number) ? 1 : 0;
  const sx = tx === 'string' ? (x as string) : show(x);
  const sy = ty === 'string' ? (y as string) : show(y);
  return sx < sy ? -1 : sx > sy ? 1 : 0;
}

export interface DiffOptions {
  /** Also require Map / Set iteration order and plain-object key order to
   *  match (what "the same input twice" must give). Off = compare by content. */
  ordered?: boolean;
}

/**
 * The first difference between two values, as `<path>: <a> vs <b>`, or null when
 * they are identical. Typed arrays compare by bytes, numbers by `Object.is`
 * (so -0 vs 0 and NaN are seen), Maps / Sets by entries.
 */
export function firstValueDiff(a: unknown, b: unknown, options: DiffOptions = {}, path = '$'): string | null {
  if (Object.is(a, b)) return null;
  const ka = kindOf(a);
  const kb = kindOf(b);
  if (ka !== kb) return `${path}: ${ka} ${show(a)} vs ${kb} ${show(b)}`;
  if (ArrayBuffer.isView(a) && ArrayBuffer.isView(b)) {
    const d = firstByteDiff(a, b);
    return d === null ? null : `${path}: ${d}`;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) return `${path}.length: ${a.length} vs ${b.length}`;
    for (let i = 0; i < a.length; i++) {
      const d = firstValueDiff(a[i], b[i], options, `${path}[${i}]`);
      if (d !== null) return d;
    }
    return null;
  }
  if (a instanceof Map && b instanceof Map) {
    if (a.size !== b.size) return `${path}.size: ${a.size} vs ${b.size}`;
    const order = (m: Map<unknown, unknown>) =>
      options.ordered ? [...m.entries()] : [...m.entries()].sort((x, y) => compareKeys(x[0], y[0]));
    const ea = order(a);
    const eb = order(b);
    for (let i = 0; i < ea.length; i++) {
      const [keyA, valueA] = ea[i]!;
      const [keyB, valueB] = eb[i]!;
      if (!Object.is(keyA, keyB)) return `${path} entry #${i} key: ${show(keyA)} vs ${show(keyB)}`;
      const d = firstValueDiff(valueA, valueB, options, `${path}.get(${show(keyA)})`);
      if (d !== null) return d;
    }
    return null;
  }
  if (a instanceof Set && b instanceof Set) {
    if (a.size !== b.size) return `${path}.size: ${a.size} vs ${b.size}`;
    const order = (s: Set<unknown>) => (options.ordered ? [...s] : [...s].sort(compareKeys));
    const ma = order(a);
    const mb = order(b);
    for (let i = 0; i < ma.length; i++) {
      if (!Object.is(ma[i], mb[i])) return `${path} member #${i}: ${show(ma[i])} vs ${show(mb[i])}`;
    }
    return null;
  }
  if (typeof a === 'object' && a !== null && typeof b === 'object' && b !== null) {
    const ra = a as Record<string, unknown>;
    const rb = b as Record<string, unknown>;
    const keysA = Object.keys(ra);
    const keysB = Object.keys(rb);
    if (options.ordered && keysA.join('\u0000') !== keysB.join('\u0000')) {
      return `${path}: key order ${show(keysA)} vs ${show(keysB)}`;
    }
    const keys = [...new Set([...keysA, ...keysB])].sort();
    for (const key of keys) {
      const child = `${path}.${key}`;
      if (!(key in ra)) return `${child}: <absent> vs ${show(rb[key])}`;
      if (!(key in rb)) return `${child}: ${show(ra[key])} vs <absent>`;
      const d = firstValueDiff(ra[key], rb[key], options, child);
      if (d !== null) return d;
    }
    return null;
  }
  return `${path}: ${show(a)} vs ${show(b)}`;
}

/** `firstValueDiff` with the seed (and a label) prefixed, for `expect(...).toBeNull()`. */
export function seededDiff(
  seed: number,
  label: string,
  a: unknown,
  b: unknown,
  options: DiffOptions = { ordered: true },
): string | null {
  const d = firstValueDiff(a, b, options);
  return d === null ? null : `seed=${seed} ${label}: ${d}`;
}

/** Same as {@link seededDiff} for two strings that must be identical (a
 *  canonical JSON, a serialized result): reports the first differing offset. */
export function seededTextDiff(seed: number, label: string, a: string, b: string): string | null {
  if (a === b) return null;
  let i = 0;
  while (i < a.length && i < b.length && a[i] === b[i]) i++;
  const around = (s: string) => JSON.stringify(s.slice(Math.max(0, i - 40), i + 40));
  return `seed=${seed} ${label}: first difference at char ${i} (lengths ${a.length} vs ${b.length}): ${around(a)} vs ${around(b)}`;
}

/**
 * Block-wise equivariance: `b` is `a` with its fixed-size blocks permuted so
 * that block `k` of `b` is block `perm[k]` of `a` (e.g. a station's 4 vertices
 * after the stations were reordered). Byte comparison; null when it holds.
 */
export function permutedBlocksDiff(
  a: ArrayBufferView,
  b: ArrayBufferView,
  perm: readonly number[],
  elementsPerBlock: number,
): string | null {
  if (a.constructor !== b.constructor) return `${a.constructor.name} vs ${b.constructor.name}`;
  if (a.byteLength !== b.byteLength) return `byteLength ${a.byteLength} vs ${b.byteLength}`;
  const width = (a as unknown as { BYTES_PER_ELEMENT?: number }).BYTES_PER_ELEMENT ?? 1;
  const blockBytes = elementsPerBlock * width;
  const ua = new Uint8Array(a.buffer, a.byteOffset, a.byteLength);
  const ub = new Uint8Array(b.buffer, b.byteOffset, b.byteLength);
  for (let k = 0; k < perm.length; k++) {
    const from = perm[k]! * blockBytes;
    const to = k * blockBytes;
    for (let j = 0; j < blockBytes; j++) {
      if (ua[from + j] !== ub[to + j]) {
        return `block ${k} (from ${perm[k]}) byte ${j}: ${ua[from + j]} vs ${ub[to + j]}`;
      }
    }
  }
  return null;
}

// ─── Synthetic surveys ──────────────────────────────────────────────

/** Section colors the generator draws from (CSS hex, the wire format). */
const PALETTE = ['#e74c3c', '#3498db', '#2ecc71', '#f1c40f', '#9b59b6', '#1abc9c', '#e67e22'];

/** Comment texts: short ones and a long one that wraps (comment max width 12 em). */
const NOTE_WORDS = ['restriction', 'sump', 'silt', 'tie-in', 'line', 'arrow', 'cookie', 'halocline', 'breakdown'];

export interface SyntheticSurveyOptions {
  /** Upper bound on sections (at least 1 is generated). Default 4. */
  maxSections?: number;
  /** Upper bound on stations per section (at least 2). Default 8. */
  maxStations?: number;
  /** Probability that a shot / station carries a comment. Default 0.3. */
  commentRate?: number;
  /** Mean shot length in metres (stations are a random walk). Default 4. */
  stepMetres?: number;
}

function note(rng: Rng, owner: string): string {
  const words = Array.from({ length: randInt(rng, 1, 7) }, () => pick(rng, NOTE_WORDS));
  return `${owner} ${words.join(' ')}`;
}

/**
 * A random but well-formed `SurveyData`: CaveA.. sections with distinct,
 * unsorted ids, stations as a random walk (so labels crowd and collide), every
 * shot kind, the occasional dropped interior station (an invalid shot), zero
 * datums, and sparse shot / station comments. Stations are laid out in a random
 * interleaved GLOBAL order - the SoA does not promise per-section contiguity, and
 * nothing keyed by (section, index) may depend on it.
 */
export function syntheticSurvey(rng: Rng, options: SyntheticSurveyOptions = {}): SurveyData {
  const maxSections = options.maxSections ?? 4;
  const maxStations = Math.max(2, options.maxStations ?? 8);
  const commentRate = options.commentRate ?? 0.3;
  const step = options.stepMetres ?? 4;

  const sectionCount = randInt(rng, 1, maxSections);
  const ids = shuffled(Array.from({ length: 60 }, (_, i) => i + 1), rng).slice(0, sectionCount);

  interface StationRow {
    sid: number;
    idx: number;
    x: number;
    y: number;
    z: number;
    zeroDatum: boolean;
    comment: string | null;
  }
  const rows: StationRow[] = [];
  const sections: SectionMetadata[] = [];
  const shotIds: number[] = [];
  const shotSectionIds: number[] = [];
  const shotFlags: number[] = [];
  const shotAz: number[] = [];
  const shotLen: number[] = [];
  const depthIn: number[] = [];
  const depthOut: number[] = [];
  const shotComments = new Map<number, string>();

  let x = rng() * 20;
  let y = rng() * 20;
  let z = -rng() * 5;
  for (let s = 0; s < sectionCount; s++) {
    const sid = ids[s]!;
    const letter = String.fromCharCode(65 + (s % 26));
    const nStations = randInt(rng, 2, maxStations);
    const dropIdx = nStations > 2 && rng() < 0.25 ? randInt(rng, 1, nStations - 2) : -1;
    const shotOffset = shotIds.length;
    for (let i = 0; i < nStations; i++) {
      // A new section usually tees off near the previous walk position.
      x += (rng() - 0.5) * 2 * step;
      y += (rng() - 0.5) * 2 * step;
      z -= rng() * 2;
      if (i === dropIdx) continue;
      rows.push({
        sid,
        idx: i,
        x,
        y,
        z,
        zeroDatum: rng() < 0.1,
        comment: rng() < commentRate ? note(rng, `Station${letter}${i}`) : null,
      });
    }
    for (let j = 0; j < nStations - 1; j++) {
      const g = shotIds.length;
      shotIds.push(sid * 100_000 + j);
      shotSectionIds.push(sid);
      const r = rng();
      shotFlags.push(shotKindToFlags(r < 0.7 ? 'Normal' : r < 0.85 ? 'Jump' : 'Reference'));
      shotAz.push(Math.round(rng() * 3600) / 10);
      shotLen.push(Math.round(rng() * step * 200) / 100);
      depthIn.push(Math.round(rng() * 400) / 10);
      depthOut.push(Math.round(rng() * 400) / 10);
      if (rng() < commentRate) shotComments.set(g, note(rng, `Shot${letter}${j}`));
    }
    sections.push({
      id: sid,
      name: `Cave${letter}`,
      surveyor: null,
      date: null,
      color: pick(rng, PALETTE),
      direction: rng() < 0.5 ? 'in' : 'out',
      quality: 1,
      next_shot_id: sid * 100_000 + nStations - 1,
      azimuth_reference: null,
      shotOffset,
      shotCount: nStations - 1,
    });
  }

  const order = shuffled(rows, rng);
  const d = emptySurveyData();
  d.sections = sections;
  d.shotCount = shotIds.length;
  d.shotIds = new Uint32Array(shotIds);
  d.shotSectionIds = new Uint32Array(shotSectionIds);
  d.shotFlags = new Uint8Array(shotFlags);
  d.shotAzimuthIn = new Float32Array(shotAz);
  d.shotAzimuthOut = new Float32Array(shotAz);
  d.shotLength = new Float32Array(shotLen);
  d.shotDepthIn = new Float32Array(depthIn);
  d.shotDepthOut = new Float32Array(depthOut);
  const nanCol = () => new Float32Array(shotIds.length).fill(Number.NaN);
  d.shotPitchIn = nanCol();
  d.shotPitchOut = nanCol();
  d.shotLeft = nanCol();
  d.shotRight = nanCol();
  d.shotUp = nanCol();
  d.shotDown = nanCol();
  d.shotTemperature = nanCol();
  d.shotMarkerIndex = new Int32Array(shotIds.length).fill(-1);
  d.shotComments = shotComments;
  d.stationCount = order.length;
  d.stationSectionIds = new Uint32Array(order.map((r) => r.sid));
  d.stationIndices = new Uint32Array(order.map((r) => r.idx));
  d.stationCoords = new Float64Array(order.flatMap((r) => [r.x, r.y, r.z]));
  d.stationFlags = new Uint8Array(order.map((r) => (r.zeroDatum ? STATION_FLAG_IS_ZERO_DATUM : 0)));
  d.stationComments = new Map();
  order.forEach((r, g) => {
    if (r.comment !== null) d.stationComments.set(g, r.comment);
  });
  return d;
}

/** The same survey with its sparse Maps re-inserted in a shuffled order (the
 *  content - and every typed array - unchanged). */
export function withShuffledSparseMaps(data: SurveyData, rng: Rng): SurveyData {
  return {
    ...data,
    shotComments: shuffledMap(data.shotComments, rng),
    stationGps: shuffledMap(data.stationGps, rng),
    stationMergedWith: shuffledMap(data.stationMergedWith, rng),
    stationClosuresWith: shuffledMap(data.stationClosuresWith, rng),
    stationComments: shuffledMap(data.stationComments, rng),
  };
}

/**
 * The same survey with its stations re-laid-out in another GLOBAL order: new
 * station `k` is old station `perm[k]`, and every station-indexed column and
 * sparse map follows. Shots and sections are untouched. Whatever a builder emits
 * per station must come out block-permuted; whatever it looks up by
 * (section, station index) must not move at all.
 */
export function withPermutedStations(data: SurveyData, perm: readonly number[]): SurveyData {
  const inverse = new Map<number, number>();
  perm.forEach((oldG, newG) => inverse.set(oldG, newG));
  const remap = <V>(m: ReadonlyMap<number, V>): Map<number, V> => {
    const out = new Map<number, V>();
    for (const [oldG, v] of m) out.set(inverse.get(oldG)!, v);
    return out;
  };
  return {
    ...data,
    stationSectionIds: Uint32Array.from(perm, (g) => data.stationSectionIds[g]!),
    stationIndices: Uint32Array.from(perm, (g) => data.stationIndices[g]!),
    stationCoords: Float64Array.from(perm.flatMap((g) => [
      data.stationCoords[g * 3]!,
      data.stationCoords[g * 3 + 1]!,
      data.stationCoords[g * 3 + 2]!,
    ])),
    stationFlags: Uint8Array.from(perm, (g) => data.stationFlags[g]!),
    stationGps: remap(data.stationGps),
    stationMergedWith: remap(data.stationMergedWith),
    stationClosuresWith: remap(data.stationClosuresWith),
    stationComments: remap(data.stationComments),
  };
}

// ─── Glyph atlas ────────────────────────────────────────────────────

/**
 * A canvas-free glyph atlas with per-character metrics (advance and ink width
 * vary with the char code, a space has no ink), so shaping, wrapping and label
 * extents differ between texts the way a real font's do. The atlas packs glyphs
 * in FIRST-USE order (`glyphAtlas.ts` shelf packing), so two atlases only agree
 * on uv rects when they were fed the same glyph sequence.
 */
export function fakeGlyphAtlas(): GlyphAtlas {
  const rasterizer = (c: string): RasterizedGlyph => {
    const code = c.codePointAt(0) ?? 0;
    const ink = c.trim().length > 0;
    const w = ink ? 8 + (code % 7) : 0;
    const h = ink ? 14 + (code % 5) : 0;
    return {
      data: new Uint8Array(w * h),
      width: w,
      height: h,
      glyphWidth: Math.max(0, w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, h - 2 * SDF_BUFFER),
      glyphTop: 15 - SDF_BUFFER,
      glyphLeft: 1 + SDF_BUFFER,
      glyphAdvance: ink ? 9 + (code % 6) : 6,
    };
  };
  return new GlyphAtlas({ rasterizer });
}
