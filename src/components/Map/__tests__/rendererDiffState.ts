/**
 * Render-state capture, compare and a GPU shadow for the renderer
 * incremental-vs-fresh differential (#576 FZ-ORACLE,
 * rendererIncrementalDifferential.test.ts). NOT a test suite - helpers it imports.
 *
 * - {@link captureRenderState}: every CPU-side array and scalar the geometry and
 *   label controllers (and their GL layers) hold, as ordered named entries.
 * - {@link firstDifference}: the first entry + element index two captures differ
 *   at (whole arrays, byte for byte - the verdict).
 * - {@link fingerprint}: the FNV-1a `hv` digest the W14-FE proof spec used, for a
 *   one-line summary of a capture in a failure report (never the verdict).
 * - {@link ShadowGl}: a FakeGl that keeps a byte copy of every buffer it is given
 *   (`bufferData` / `bufferSubData`), so {@link gpuIncoherence} can check that
 *   what the GPU would hold equals the CPU arrays it was uploaded from.
 */
import { FakeGl, FakeWebGLBuffer } from '../surveyLayer/customRender/__tests__/fakeGl';

// ─── FNV-1a fingerprint (the W14-FE `hv`) ─────────────────────────────────────

export const FNV_OFFSET = 2166136261;
const te = new TextEncoder();

function fnv(h: number, b: Uint8Array): number {
  for (let i = 0; i < b.length; i++) {
    h ^= b[i]!;
    h = Math.imul(h, 16777619) >>> 0;
  }
  return h;
}

function bytesOf(v: ArrayBufferView): Uint8Array {
  return new Uint8Array(v.buffer, v.byteOffset, v.byteLength);
}

/** FNV-1a over every typed array / number / string / Map / Set / array / plain
 *  object reachable from `v` (depth-capped, cycle-safe, class instances by name). */
export function hv(v: unknown, h = FNV_OFFSET, d = 0, seen = new WeakSet<object>()): number {
  if (v === null || v === undefined) return fnv(h, te.encode(String(v)));
  if (ArrayBuffer.isView(v)) return fnv(fnv(h, te.encode(`${v.constructor.name}:${v.byteLength}`)), bytesOf(v));
  if (typeof v !== 'object') return typeof v === 'function' ? h : fnv(h, te.encode(`${typeof v}:${String(v)}`));
  if (seen.has(v) || d > 5) return h;
  seen.add(v);
  if (v instanceof Map) {
    for (const [k, x] of v) h = hv(x, hv(k, h, d + 1, seen), d + 1, seen);
    return h;
  }
  if (v instanceof Set) {
    for (const x of v) h = hv(x, h, d + 1, seen);
    return h;
  }
  if (Array.isArray(v)) {
    h = fnv(h, te.encode(`[${v.length}`));
    for (const x of v) h = hv(x, h, d + 1, seen);
    return h;
  }
  const proto = Object.getPrototypeOf(v) as { constructor?: { name?: string } } | null;
  if (proto !== Object.prototype && proto !== null) return fnv(h, te.encode(`<${proto.constructor?.name}>`));
  for (const k of Object.keys(v).sort()) h = hv((v as Record<string, unknown>)[k], fnv(h, te.encode(k)), d + 1, seen);
  return h;
}

// ─── GPU shadow ───────────────────────────────────────────────────────────────

/** FakeGl that mirrors buffer contents: what a real context would hold after the
 *  layer's `bufferData` / `bufferSubData` calls. `faults` records calls a real
 *  context would reject (no buffer bound, a sub-upload past the buffer end). */
export class ShadowGl extends FakeGl {
  readonly shadow = new Map<unknown, Uint8Array>();
  readonly faults: string[] = [];
  private readonly bound = new Map<number, unknown>();

  override bindBuffer(target: number, buffer: unknown): void {
    super.bindBuffer(target, buffer);
    this.bound.set(target, buffer);
  }

  override bufferData(target: number, data: unknown, usage: number): void {
    super.bufferData(target, data, usage);
    const buf = this.bound.get(target);
    if (!buf || !ArrayBuffer.isView(data)) {
      this.faults.push(`bufferData without a bound buffer / typed source (target ${target})`);
      return;
    }
    this.shadow.set(buf, bytesOf(data).slice());
  }

  /** WebGL2 `bufferSubData(target, dstByteOffset, src, srcOffset?, length?)`:
   *  `srcOffset` / `length` count ELEMENTS of `src`; `length` 0 / absent = to the end. */
  bufferSubData(target: number, dstByteOffset: number, src: ArrayBufferView, srcOffset = 0, length = 0): void {
    this.calls.bufferSubData = this.count('bufferSubData') + 1;
    const buf = this.bound.get(target);
    const dst = buf === undefined ? undefined : this.shadow.get(buf);
    if (!dst) {
      this.faults.push(`bufferSubData into an unallocated buffer (target ${target})`);
      return;
    }
    const bpe = (src as unknown as { BYTES_PER_ELEMENT?: number }).BYTES_PER_ELEMENT ?? 1;
    const all = bytesOf(src);
    const from = srcOffset * bpe;
    const to = length > 0 ? from + length * bpe : all.length;
    if (to > all.length || dstByteOffset + (to - from) > dst.length) {
      this.faults.push(`bufferSubData out of range: dst ${dstByteOffset}+${to - from} > ${dst.length}`);
      return;
    }
    dst.set(all.subarray(from, to), dstByteOffset);
  }

  override deleteBuffer(buf: unknown): void {
    super.deleteBuffer(buf);
    this.shadow.delete(buf);
  }
}

/** A ShadowGl behind a Proxy that turns every GL entry point FakeGl lacks
 *  (textures, uniforms, VAOs...) into a no-op, so the REAL custom layers' onAdd /
 *  update paths run. `instanceof` still sees the ShadowGl prototype. */
export function makeShadowGl(): { gl: WebGL2RenderingContext; shadow: ShadowGl } {
  const shadow = new ShadowGl();
  const noop = (): object => ({});
  const gl = new Proxy(shadow, {
    get(t, p) {
      if (p in t) {
        const v = Reflect.get(t, p, t) as unknown;
        return typeof v === 'function' ? (v as (...a: unknown[]) => unknown).bind(t) : v;
      }
      return noop;
    },
  });
  return { gl: gl as unknown as WebGL2RenderingContext, shadow };
}

export { FakeWebGLBuffer };

// ─── Private shapes read off the controllers (no product types widened) ───────

interface GeoLayerPriv {
  data: unknown;
  lineGpu: Record<string, unknown> | null;
  circleGpu: Record<string, unknown> | null;
  drawShots: boolean;
  drawStations: boolean;
  hideReference: boolean;
  hideJump: boolean;
  disabled: boolean;
}

interface GeoCtlPriv {
  active: boolean;
  fatal: boolean;
  layer: GeoLayerPriv | null;
  lastData: { anchorX: number; anchorY: number; line: Record<string, unknown>; circle: Record<string, unknown> } & Record<string, unknown> | null;
  lastStationRel: Float32Array | null;
  lastAllReference: Float32Array | null;
  lastSurveyData: unknown;
  dimState: Record<string, unknown>;
  hideState: Record<string, unknown>;
  segmentHoverState: ReadonlySet<number>;
  sectionHoverState: ReadonlySet<number>;
  sectionSelectState: ReadonlySet<number>;
  shotsVisible: boolean;
  stationsVisible: boolean;
  referenceShotsVisible: boolean;
  jumpShotsVisible: boolean;
}

interface GlyphInfoLike {
  ax: number;
  ay: number;
}

interface LabelLayerPriv {
  data: unknown;
  gpu: Record<string, unknown> | null;
  style: unknown;
  disabled: boolean;
}

interface LabelCtlPriv {
  active: boolean;
  fatal: boolean;
  layer: LabelLayerPriv | null;
  atlas: { glyphs: Map<string, GlyphInfoLike> } | null;
  lastData: { anchorX: number; anchorY: number; buffers: Record<string, unknown> } | null;
  lastDescriptors: unknown[] | null;
  lastExtents: unknown[] | null;
  lastBuildZoom: number | undefined;
  lastWorldPixelScale: number | undefined;
  lastEmitSlack: number;
  lastEmitBounded: boolean;
  builtViewport: unknown;
  lastAnchorX: number;
  lastAnchorY: number;
  lastBuiltWantFull: boolean;
  fullLabelsBuilt: boolean;
  kindVisible: boolean[];
  referenceShotsVisible: boolean;
  jumpShotsVisible: boolean;
  dimState: Record<string, unknown>;
  hideState: Record<string, unknown>;
  hiddenComment: unknown;
  commentOverlapExempt: boolean;
  lastThemeColors: unknown;
}

/** The two controllers off a SurveyLayer (private fields, read only). */
export function controllersOf(layer: unknown): { geo: GeoCtlPriv; labels: LabelCtlPriv } {
  const l = layer as { customRender: GeoCtlPriv; customLabels: LabelCtlPriv };
  return { geo: l.customRender, labels: l.customLabels };
}

// ─── Capture ──────────────────────────────────────────────────────────────────

/** `labelEmitted` entries depend on the emitted label set, which a reveal-only
 *  rerun (kind / reference toggle) legitimately leaves stale until the next build
 *  (labelController.rerunReveal's documented approximation). */
export type EntryGroup = 'geo' | 'label' | 'labelEmitted';
export interface Entry {
  key: string;
  group: EntryGroup;
  value: unknown;
}

/** Sets -> sorted arrays, Maps -> sorted entry arrays, everything else as is, so
 *  JSON comparison is order-independent where the product's is. */
function plain(v: unknown): unknown {
  if (v instanceof Set) return [...v].sort((a, b) => (a < b ? -1 : a > b ? 1 : 0));
  if (v instanceof Map) return [...v.entries()].sort((a, b) => (a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0));
  if (Array.isArray(v)) return v.map(plain);
  if (v && typeof v === 'object' && !ArrayBuffer.isView(v)) {
    const out: Record<string, unknown> = {};
    for (const k of Object.keys(v).sort()) out[k] = plain((v as Record<string, unknown>)[k]);
    return out;
  }
  return v;
}

function pushRecord(out: Entry[], group: EntryGroup, prefix: string, rec: Record<string, unknown> | null): void {
  if (!rec) {
    out.push({ key: prefix, group, value: null });
    return;
  }
  for (const k of Object.keys(rec).sort()) {
    const v = rec[k];
    if (typeof v === 'function') continue;
    out.push({ key: `${prefix}.${k}`, group, value: ArrayBuffer.isView(v) ? v : plain(v) });
  }
}

/** Label `uv` holds atlas pixel corners, and atlas placement depends on the order
 *  glyphs were first seen (history) - so replace each glyph's uv by the glyph it
 *  addresses plus its corners relative to the glyph origin. Everything else in
 *  the label buffers is history-free and compared raw. */
function semanticUv(uv: Float32Array, atlas: LabelCtlPriv['atlas']): { glyphs: string[]; rel: Float32Array } {
  const byOrigin = new Map<string, string>();
  for (const [ch, g] of atlas?.glyphs ?? []) byOrigin.set(`${g.ax},${g.ay}`, ch);
  const n = uv.length / 8;
  const glyphs: string[] = new Array<string>(n);
  const rel = new Float32Array(uv.length);
  for (let i = 0; i < n; i++) {
    const u0 = uv[i * 8]!;
    const v0 = uv[i * 8 + 1]!;
    glyphs[i] = byOrigin.get(`${u0},${v0}`) ?? `?@${u0},${v0}`;
    for (let c = 0; c < 4; c++) {
      rel[i * 8 + c * 2] = uv[i * 8 + c * 2]! - u0;
      rel[i * 8 + c * 2 + 1] = uv[i * 8 + c * 2 + 1]! - v0;
    }
  }
  return { glyphs, rel };
}

/** Everything the two controllers + their GL layers hold that reaches the GPU or
 *  decides what a later cheap path writes. `activeSurveyData` is the store's
 *  current active geometry (the controller must have synced to exactly it). */
export function captureRenderState(layer: unknown, activeSurveyData: unknown): Entry[] {
  const { geo, labels } = controllersOf(layer);
  const out: Entry[] = [];
  const g = (key: string, value: unknown): void => void out.push({ key, group: 'geo', value });
  g('geo.active', geo.active);
  g('geo.fatal', geo.fatal);
  g('geo.lastSurveyDataIsActive', geo.lastSurveyData === activeSurveyData);
  g('geo.layerDataIsLastData', geo.layer ? geo.layer.data === geo.lastData : null);
  const ld = geo.lastData;
  if (ld) {
    for (const k of Object.keys(ld).sort()) {
      if (k === 'line' || k === 'circle') continue;
      g(`geo.data.${k}`, ld[k]);
    }
  }
  pushRecord(out, 'geo', 'geo.line', ld?.line ?? null);
  pushRecord(out, 'geo', 'geo.circle', ld?.circle ?? null);
  g('geo.stationRel', geo.lastStationRel);
  g('geo.allReference', geo.lastAllReference);
  g('geo.dimState', plain(geo.dimState));
  g('geo.hideState', plain(geo.hideState));
  g('geo.highlight', plain([geo.segmentHoverState, geo.sectionHoverState, geo.sectionSelectState]));
  g('geo.visible', [geo.shotsVisible, geo.stationsVisible, geo.referenceShotsVisible, geo.jumpShotsVisible]);
  const gl = geo.layer;
  g('geo.layer.flags', gl ? [gl.disabled, gl.drawShots, gl.drawStations, gl.hideReference, gl.hideJump] : null);

  const l = (key: string, value: unknown, group: EntryGroup = 'label'): void => void out.push({ key, group, value });
  l('labels.active', labels.active);
  l('labels.fatal', labels.fatal);
  l('labels.layerDataIsLastData', labels.layer ? labels.layer.data === labels.lastData : null);
  l('labels.layer.style', labels.layer ? plain(labels.layer.style) : null);
  l('labels.state', plain({
    kindVisible: labels.kindVisible,
    referenceShotsVisible: labels.referenceShotsVisible,
    jumpShotsVisible: labels.jumpShotsVisible,
    dimState: labels.dimState,
    hideState: labels.hideState,
    hiddenComment: labels.hiddenComment,
    commentOverlapExempt: labels.commentOverlapExempt,
    lastThemeColors: labels.lastThemeColors,
    lastBuildZoom: labels.lastBuildZoom,
    lastWorldPixelScale: labels.lastWorldPixelScale,
    lastEmitSlack: labels.lastEmitSlack,
    lastEmitBounded: labels.lastEmitBounded,
    builtViewport: labels.builtViewport,
    lastAnchor: [labels.lastAnchorX, labels.lastAnchorY],
    wantFull: [labels.lastBuiltWantFull, labels.fullLabelsBuilt],
  }));
  const lb = labels.lastData;
  l('labels.anchor', lb ? [lb.anchorX, lb.anchorY] : null);
  l('labels.descriptors', labels.lastDescriptors?.map((d) => JSON.stringify(plain(d))) ?? null, 'labelEmitted');
  l('labels.extents', labels.lastExtents?.map((e) => JSON.stringify(plain(e))) ?? null, 'labelEmitted');
  if (lb) {
    for (const k of Object.keys(lb.buffers).sort()) {
      const v = lb.buffers[k];
      if (k === 'uv' && v instanceof Float32Array) {
        const s = semanticUv(v, labels.atlas);
        l('labels.buffers.uvGlyph', s.glyphs, 'labelEmitted');
        l('labels.buffers.uvRel', s.rel, 'labelEmitted');
      } else {
        l(`labels.buffers.${k}`, v, 'labelEmitted');
      }
    }
  } else {
    l('labels.buffers', null, 'labelEmitted');
  }
  return out;
}

/** One-line FNV-1a digest of a capture (per group), for failure reports. */
export function fingerprint(entries: Entry[]): string {
  const by = new Map<EntryGroup, number>();
  for (const e of entries) by.set(e.group, hv(e.value, fnv(by.get(e.group) ?? FNV_OFFSET, te.encode(e.key))));
  return [...by].map(([k, h]) => `${k}=${h.toString(16)}`).join(' ');
}

// ─── Compare ──────────────────────────────────────────────────────────────────

export interface Difference {
  key: string;
  /** Element index of the first differing element (-1: shape / type differs). */
  index: number;
  incremental: string;
  fresh: string;
}

function fmt(v: unknown): string {
  const s = ArrayBuffer.isView(v) ? `${v.constructor.name}(${(v as unknown as { length: number }).length})` : JSON.stringify(v);
  return s === undefined ? 'undefined' : s.length > 300 ? `${s.slice(0, 300)}...` : s;
}

function diffValue(a: unknown, b: unknown): Omit<Difference, 'key'> | null {
  if (ArrayBuffer.isView(a) || ArrayBuffer.isView(b)) {
    if (!ArrayBuffer.isView(a) || !ArrayBuffer.isView(b) || a.constructor !== b.constructor || a.byteLength !== b.byteLength) {
      return { index: -1, incremental: fmt(a), fresh: fmt(b) };
    }
    const ba = bytesOf(a);
    const bb = bytesOf(b);
    for (let i = 0; i < ba.length; i++) {
      if (ba[i] !== bb[i]) {
        const bpe = (a as unknown as { BYTES_PER_ELEMENT: number }).BYTES_PER_ELEMENT;
        const at = Math.floor(i / bpe);
        const ea = (a as unknown as ArrayLike<number>)[at];
        const eb = (b as unknown as ArrayLike<number>)[at];
        return { index: at, incremental: String(ea), fresh: String(eb) };
      }
    }
    return null;
  }
  if (Array.isArray(a) && Array.isArray(b)) {
    if (a.length !== b.length) {
      for (let i = 0; i < Math.min(a.length, b.length); i++) {
        if (JSON.stringify(a[i]) !== JSON.stringify(b[i])) return { index: i, incremental: fmt(a[i]), fresh: fmt(b[i]) };
      }
      return { index: -1, incremental: `length ${a.length}`, fresh: `length ${b.length}` };
    }
    for (let i = 0; i < a.length; i++) {
      if (JSON.stringify(a[i]) !== JSON.stringify(b[i])) return { index: i, incremental: fmt(a[i]), fresh: fmt(b[i]) };
    }
    return null;
  }
  return JSON.stringify(a) === JSON.stringify(b) ? null : { index: -1, incremental: fmt(a), fresh: fmt(b) };
}

/** The first entry (in capture order) where `inc` and `fresh` differ, skipping
 *  the `skip` groups. Keys missing on one side count as a difference. */
export function firstDifference(inc: Entry[], fresh: Entry[], skip: ReadonlySet<EntryGroup> = new Set()): Difference | null {
  const fm = new Map(fresh.map((e) => [e.key, e]));
  for (const e of inc) {
    if (skip.has(e.group)) continue;
    const f = fm.get(e.key);
    if (!f) return { key: e.key, index: -1, incremental: fmt(e.value), fresh: '<missing>' };
    const d = diffValue(e.value, f.value);
    if (d) return { key: e.key, ...d };
  }
  const ik = new Set(inc.map((e) => e.key));
  for (const f of fresh) {
    if (!skip.has(f.group) && !ik.has(f.key)) return { key: f.key, index: -1, incremental: '<missing>', fresh: fmt(f.value) };
  }
  return null;
}

// ─── GPU coherence ────────────────────────────────────────────────────────────

/** GPU buffer field -> the CPU array field it is uploaded from (default: same name). */
const GPU_SOURCE: Record<string, string> = { index: 'indices' };

function gpuPairs(prefix: string, gpu: Record<string, unknown> | null, cpu: Record<string, unknown> | undefined): Array<[string, unknown, unknown]> {
  if (!gpu || !cpu) return [];
  const pairs: Array<[string, unknown, unknown]> = [];
  for (const k of Object.keys(gpu)) {
    if (k === 'count') continue;
    pairs.push([`${prefix}.${k}`, gpu[k], cpu[GPU_SOURCE[k] ?? k]]);
  }
  return pairs;
}

/** First place the shadowed GPU buffers of one SurveyLayer disagree with the CPU
 *  arrays their layer holds (or a GL call a real context would reject), or null. */
export function gpuIncoherence(layer: unknown, shadow: ShadowGl): string | null {
  if (shadow.faults.length > 0) return `gl fault: ${shadow.faults[0]}`;
  const { geo, labels } = controllersOf(layer);
  const gd = geo.layer?.data as { line?: Record<string, unknown>; circle?: Record<string, unknown> } | null | undefined;
  const ld = labels.layer?.data as { buffers?: Record<string, unknown> } | null | undefined;
  const pairs = [
    ...gpuPairs('geo.lineGpu', geo.layer?.lineGpu ?? null, gd?.line),
    ...gpuPairs('geo.circleGpu', geo.layer?.circleGpu ?? null, gd?.circle),
    ...gpuPairs('labels.gpu', labels.layer?.gpu ?? null, ld?.buffers),
  ];
  for (const [name, buf, cpu] of pairs) {
    const gpuBytes = shadow.shadow.get(buf);
    if (!gpuBytes) return `${name}: no shadow (never uploaded)`;
    if (!ArrayBuffer.isView(cpu)) return `${name}: no CPU source array`;
    const c = bytesOf(cpu);
    if (c.length !== gpuBytes.length) return `${name}: gpu ${gpuBytes.length} bytes, cpu ${c.length} bytes`;
    for (let i = 0; i < c.length; i++) {
      if (c[i] !== gpuBytes[i]) return `${name}: gpu != cpu at byte ${i}`;
    }
  }
  return null;
}
