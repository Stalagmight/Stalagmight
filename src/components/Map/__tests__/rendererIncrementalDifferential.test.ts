/**
 * Renderer differential: incremental vs fresh build (#576 FZ-ORACLE).
 *
 * A seeded random sequence of survey edits and display changes is applied to ONE
 * real SurveyLayer the way MapCanvas applies them (store update -> the
 * useSurveyLayerSync layout effect's `update()` -> the passive setters ->
 * useAnalyticalOverlays' setters -> the coalesced flush), so the geometry
 * controller picks its incremental / color-only / full tier itself and the dim /
 * highlight / label side channels run as in the app. After every step a SECOND,
 * fresh SurveyLayer is mounted on the same final state, and the two must hold
 * identical CPU-side render state: every typed array of the geometry and label
 * buffers, the controllers' cached products, the dim / highlight / visibility
 * state, and the GL layers' draw flags (rendererDiffState.ts). Each instance's GL
 * is a shadow context, so every buffer the GPU would hold must also equal the CPU
 * array it came from (the incremental path's sub-upload ranges included).
 *
 * The REAL controllers and GL layers run over the fake map (surveyLayerFakeMap.ts)
 * with a shadow WebGL2 context handed to each custom layer's onAdd; only the
 * label font is replaced (a deterministic per-character rasterizer instead of
 * TinySDF + FontFace, which node lacks).
 *
 * One documented approximation is honoured, not hidden: a kind / reference toggle
 * re-runs the label reveal pass over the labels the last build EMITTED
 * (labelController.rerunReveal), so until the next label build the emitted label
 * set may differ from a fresh build's. Only while that is the case are the
 * emitted-label entries skipped (counted in the tally); everything else is
 * compared on every step.
 *
 * Not reached: drag previews (setDragHide, the drag ghost), the measured-path
 * layer, camera moves (the camera is fixed per seed, so no viewport re-scope),
 * an overlay field recompute still in flight (the "hold"), config size changes,
 * WebGL context loss / restore, DPR changes, glyph-atlas growth, build-time
 * exclusions, and the interact-overlay / loop-closure GeoJSON sources.
 *
 * Knobs: RENDER_DIFF_SEED=<seed> replays one seed; RENDER_DIFF_SEEDS=<count>,
 * RENDER_DIFF_FIRST=<first seed>, RENDER_DIFF_STEPS=<steps per seed>. The
 * default is the CI set (CI_SEEDS from seed 1, plus REGRESSION_SEEDS).
 */
import { beforeEach, afterEach, describe, expect, it, vi } from 'vitest';

vi.mock('maplibre-gl', () => ({ default: {} }));
// Dev-build timing logs (one per build, thousands per run) go through console.log,
// which carries the run's tally line.
vi.mock('../../../utils/debugLog', () => ({ debugLog: () => {} }));
vi.mock('../surveyLayer/customRender/glyphAtlas', async (importOriginal) => {
  const real = await importOriginal<typeof import('../surveyLayer/customRender/glyphAtlas')>();
  // TinySDF needs a canvas and the bundled font a FontFace; node has neither. A
  // deterministic stand-in with per-character widths keeps shaping text-dependent.
  const raster = (ch: string): import('../surveyLayer/customRender/glyphAtlas').RasterizedGlyph => {
    const code = ch.codePointAt(0) ?? 0;
    const empty = ch === ' ';
    const w = empty ? 0 : 8 + (code % 7);
    const h = empty ? 0 : 18;
    return {
      data: new Uint8Array(w * h).fill(code & 0xff),
      width: w,
      height: h,
      glyphWidth: Math.max(0, w - 2 * real.SDF_BUFFER),
      glyphHeight: Math.max(0, h - 2 * real.SDF_BUFFER),
      glyphTop: 15,
      glyphLeft: 1,
      glyphAdvance: empty ? 6 : 10 + (code % 5),
    };
  };
  return {
    ...real,
    ensureLabelFont: () => Promise.resolve(true),
    canRasterize: () => true,
    createTinySdfRasterizer: () => raster,
  };
});

import SurveyLayer from '../SurveyLayer';
import { makeFakeSurveyMap } from './surveyLayerFakeMap';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { selectActiveStations, selectActiveSurveyData } from '../../../store/geometrySelectors';
import {
  getEffectiveShowJumps,
  getEffectiveShowReferenceShots,
  getEffectiveShowShotComments,
  getEffectiveShowShotData,
  getEffectiveShowStationComments,
} from '../../../store/displaySelectors';
import { getMapThemeColors, type MapThemeColors } from '../../../utils/themeColors';
import { buildDistanceFeatureState, overlayApplyAction } from '../../../utils/distanceGradient';
import { buildGasReachFeatureState } from '../../../utils/gasReach';
import { metersToMapCoords } from '../../../utils/coordinates';
import type { GpsOrigin } from '../../../types/ipc/GpsOrigin';
import type { Station } from '../../../types/ipc/Station';
import { Rng, initialModel, materialize, step, type Display, type StoreState } from './rendererDiffGen';
import {
  FakeWebGLBuffer,
  ShadowGl,
  captureRenderState,
  controllersOf,
  firstDifference,
  fingerprint,
  gpuIncoherence,
  makeShadowGl,
  type EntryGroup,
} from './rendererDiffState';

// ─── Knobs ────────────────────────────────────────────────────────────────────

/** CI default: seeds 1..CI_SEEDS, CI_STEPS steps each (a few seconds). */
const CI_SEEDS = 32;
const CI_STEPS = 50;
/** Every seed that ever failed, kept as a permanent regression seed.
 *  12: FZ-ORACLE-R-1 (fixed: a theme change committed with a gps-origin or
 *  depth-colour change dropped the latter; see the two theme-commit tests).
 *  102, 125, 167: driver-model gaps found while building this (a cleared gas
 *  origin flags the field failed; a non-interactive tool drops the hover; the
 *  app never passes an undefined precision) - kept so the model stays right. */
const REGRESSION_SEEDS: readonly number[] = [12, 102, 125, 167];


const envNum = (k: string): number | undefined => {
  const v = (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env?.[k];
  return v === undefined || v === '' ? undefined : Number(v);
};
const ONE_SEED = envNum('RENDER_DIFF_SEED');
const FIRST = envNum('RENDER_DIFF_FIRST') ?? 1;
const COUNT = envNum('RENDER_DIFF_SEEDS') ?? CI_SEEDS;
const STEPS = envNum('RENDER_DIFF_STEPS') ?? CI_STEPS;
const SEEDS: number[] =
  ONE_SEED !== undefined
    ? [ONE_SEED]
    : [...new Set([...Array.from({ length: COUNT }, (_, i) => FIRST + i), ...REGRESSION_SEEDS])];

// ─── Camera + themes ──────────────────────────────────────────────────────────

/** A fixed camera per seed. `viewport: false` = no bounds (labels unscoped, no
 *  reveal pass); else a box around (cx, cy) that follows the GPS origin, so a
 *  datum change does not just pan the survey out of view. Zooms avoid the
 *  13..13.5 label hysteresis band (history-dependent by design). */
interface Camera {
  zoom: number;
  viewport: boolean;
  cx: number;
  cy: number;
  half: number;
  gps: GpsOrigin | null;
}

function cameraFor(r: Rng): Camera {
  return { zoom: r.pick([11, 12.5, 14, 16, 18, 18]), viewport: r.bool(0.6), cx: r.coord(150), cy: r.coord(150), half: r.range(60, 600), gps: null };
}

function boundsOf(cam: Camera): unknown {
  if (!cam.viewport) return undefined;
  const [w, s] = metersToMapCoords(cam.cx - cam.half, cam.cy - cam.half, cam.gps);
  const [e, n] = metersToMapCoords(cam.cx + cam.half, cam.cy + cam.half, cam.gps);
  return { getWest: () => w, getEast: () => e, getNorth: () => n, getSouth: () => s };
}

const BASE_THEME = getMapThemeColors();
const PALETTES: readonly MapThemeColors[] = [
  BASE_THEME,
  {
    ...BASE_THEME,
    stationDefault: '#d0d0d0',
    zeroDatum: '#ff00aa',
    strokeColor: '#101010',
    depthShallow: '#33ccff',
    depthDeep: '#cc2200',
    labelColor: '#202020',
    textHalo: '#f8f8f8',
    textHaloAlt: '#e0e0e0',
    grayLine: '#999999',
  },
];

// ─── One commit, as MapCanvas makes it ────────────────────────────────────────

/** What one render commit hands SurveyLayer: the store slices + display state,
 *  with the memoized values (active stations, the theme object) the hooks hold. */
interface Snap {
  store: StoreState;
  display: Display;
  active: Station[];
  theme: MapThemeColors;
}

function snapOf(store: StoreState, display: Display, prev: Snap | null): Snap {
  const active = selectActiveStations(store, display.geometryMode);
  const theme =
    prev && prev.display.themeEpoch === display.themeEpoch ? prev.theme : { ...PALETTES[display.theme]! };
  return { store, display: { ...display }, active, theme };
}

function setStore(s: Snap): void {
  const { project, stations, surveyData, stationsAdjusted, surveyDataAdjusted } = s.store;
  useProjectStore.setState({ project, stations, surveyData, stationsAdjusted, surveyDataAdjusted });
  useUIStore.setState({ geometryMode: s.display.geometryMode });
}

/** Drain the microtask queue: the coalesced update() flush and the font promise. */
async function settle(): Promise<void> {
  for (let i = 0; i < 4; i++) await Promise.resolve();
}

const effDepth = (x: Snap): boolean => x.display.depthOn && !x.display.severityOn;

/** The display-valued args of SurveyLayer.update() changed between two commits
 *  (what applyThemeColors' re-update used to overwrite, FZ-ORACLE-R-1). */
function updateArgsChanged(a: Snap, b: Snap): boolean {
  const x = a.display;
  const y = b.display;
  return x.gpsOrigin !== y.gpsOrigin || effDepth(a) !== effDepth(b) || x.depthRange !== y.depthRange ||
    x.precision !== y.precision || x.severityOn !== y.severityOn || x.warnings !== y.warnings;
}

/** Run the effects a commit from `prev` to `next` triggers (all of them on a
 *  mount, `prev === null`), in hook order. Only effects whose deps changed run,
 *  as React does: the layout effect's update() first, then the passive setters. */
function runEffects(layer: SurveyLayer, prev: Snap | null, next: Snap): void {
  const d = next.display;
  const s = next.store;
  const changed = (f: (x: Snap) => unknown): boolean => prev === null || f(prev) !== f(next);

  // useSurveyLayerSync: the layout effect (survey publish).
  if (
    prev === null || updateArgsChanged(prev, next) || changed((x) => x.store.project) || changed((x) => x.active) ||
    changed((x) => x.display.geometryMode) || changed((x) => x.store.surveyDataAdjusted)
  ) {
    layer.update(s.project.sections, next.active, s.project.loop_closures, d.gpsOrigin, effDepth(next),
      d.depthRange, d.precision, d.severityOn, d.warnings);
  }
  // MapCanvas: the active tool and pointer / panel hover.
  if (changed((x) => x.display.tool)) layer.setActiveTool(d.tool);
  if (changed((x) => x.display.hoverSection)) layer.setHoveredSection(d.hoverSection);
  if (changed((x) => x.display.hoverSegment)) layer.setHoveredSegment(d.hoverSegment);
  // useSurveyLayerSync: passive effects, in declaration order.
  if (changed((x) => x.theme)) layer.applyThemeColors(next.theme);
  if (changed((x) => x.display.selectSection)) {
    layer.setSelectedStation(null);
    layer.setSelectedShot(null, null);
    layer.setSelectedSection(d.selectSection);
  }
  const shotData = (x: Snap): boolean => getEffectiveShowShotData(x.display.showShots, x.display.showShotData);
  if (changed(shotData)) layer.setShowShotLabels(shotData(next));
  if (changed((x) => x.display.showSectionNames)) layer.setShowSectionNames(d.showSectionNames);
  if (changed((x) => x.display.showStationLabels)) layer.setShowStationLabels(d.showStationLabels);
  const stComments = (x: Snap): boolean => getEffectiveShowStationComments(x.display.showStations, x.display.showStationComments);
  if (changed(stComments)) layer.setShowStationComments(stComments(next));
  const shComments = (x: Snap): boolean => getEffectiveShowShotComments(x.display.showShots, x.display.showShotComments);
  if (changed(shComments)) layer.setShowShotComments(shComments(next));
  if (changed((x) => x.display.hiddenComment)) layer.setHiddenComment(d.hiddenComment);
  if (changed((x) => x.display.showStations)) layer.setShowStations(d.showStations);
  if (changed((x) => x.display.showShots)) layer.setShowShots(d.showShots);
  const refs = (x: Snap): boolean => getEffectiveShowReferenceShots(x.display.showShots, x.display.showReferenceShots);
  if (changed(refs)) layer.setShowReferenceShots(refs(next));
  const jumps = (x: Snap): boolean => getEffectiveShowJumps(x.display.showShots, x.display.showJumps);
  if (changed(jumps)) layer.setShowJumps(jumps(next));
  if (changed((x) => x.display.anchored)) layer.setAnchoredSections(d.anchored);
  if (changed((x) => x.display.immovableStations) || changed((x) => x.display.immovableShots)) {
    layer.setImmovableFeatures(d.immovableStations, d.immovableShots);
  }
  // useAnalyticalOverlays: distance, gas reach, plan idle tint (measured SoA). A
  // commit here is SETTLED: a null field with the mode on means the fetch failed
  // or was cleared (no origin), which the hook flags as `failed` - the transient
  // "recompute in flight" hold is not a settled state (see the coverage notes).
  const distOn = d.distanceOn && !d.severityOn;
  if (changed((x) => x.display.distanceOn) || changed((x) => x.display.severityOn) ||
    changed((x) => x.display.distanceField) || changed((x) => x.store.surveyData)) {
    if (distOn && d.distanceField) layer.setDistanceGradient(true, buildDistanceFeatureState(s.surveyData, d.distanceField));
    else if (overlayApplyAction(distOn, d.distanceField != null, true, d.distanceField == null) === 'off') {
      layer.setDistanceGradient(false, null);
    }
  }
  if (changed((x) => x.display.gasPlan) || changed((x) => x.display.gasField) || changed((x) => x.store.surveyData)) {
    if (d.gasPlan && d.gasField) layer.setGasReach(true, buildGasReachFeatureState(s.surveyData, d.gasField));
    else if (overlayApplyAction(d.gasPlan, d.gasField != null, true, d.gasField == null) === 'off') layer.setGasReach(false, null);
  }
  if (changed((x) => x.display.gasPlan) || changed((x) => x.display.gasOriginChosen)) {
    layer.setPlanIdleTint(d.gasPlan && !d.gasOriginChosen);
  }
}

// ─── Instances ────────────────────────────────────────────────────────────────

interface Instance {
  layer: SurveyLayer;
  shadow: ShadowGl;
}

interface CustomLayerLike {
  type?: string;
  onAdd?: (map: unknown, gl: unknown) => void;
}

/** A SurveyLayer on a fresh fake map whose custom layers get a shadow WebGL2
 *  context, the seed's camera, and the commit `snap` mounted and flushed. */
async function mount(cam: Camera, snap: Snap): Promise<Instance> {
  const fake = makeFakeSurveyMap();
  const { gl, shadow } = makeShadowGl();
  const map = fake.map as unknown as Record<string, unknown>;
  const add = map.addLayer as (layer: CustomLayerLike, beforeId?: string) => void;
  map.addLayer = (layer: CustomLayerLike, beforeId?: string): void => {
    const onAdd = layer.onAdd;
    if (layer.type !== 'custom' || !onAdd) return add(layer, beforeId);
    const own = Object.prototype.hasOwnProperty.call(layer, 'onAdd');
    layer.onAdd = (m: unknown) => onAdd.call(layer, m, gl);
    try {
      add(layer, beforeId);
    } finally {
      if (own) layer.onAdd = onAdd;
      else delete layer.onAdd;
    }
  };
  map.getZoom = () => cam.zoom;
  map.getBounds = () => boundsOf(cam);
  const layer = new SurveyLayer(fake.map);
  await settle(); // the label font promise
  runEffects(layer, null, snap);
  await settle();
  return { layer, shadow };
}

interface Tally {
  steps: number;
  incremental: number;
  colorOnly: number;
  full: number;
  dim: number;
  highlight: number;
  labelBuild: number;
  labelFlags: number;
  labelReveal: number;
  revealSkips: number;
}

const newTally = (): Tally => ({
  steps: 0, incremental: 0, colorOnly: 0, full: 0, dim: 0, highlight: 0,
  labelBuild: 0, labelFlags: 0, labelReveal: 0, revealSkips: 0,
});

/** Count the tiers / side channels the incremental instance takes (wrapping its
 *  controllers' own methods; behaviour unchanged). Returns the reveal-dirty probe:
 *  true while a reveal-only rerun ran after the last label build. */
function instrument(layer: SurveyLayer, t: Tally): () => boolean {
  const { geo, labels } = controllersOf(layer);
  let revealDirty = false;
  const wrap = (obj: object, name: string, after: (result: unknown) => void): void => {
    const o = obj as Record<string, (...a: unknown[]) => unknown>;
    const orig = o[name]!;
    o[name] = function (this: unknown, ...a: unknown[]) {
      const r = orig.apply(this, a);
      after(r);
      return r;
    };
  };
  wrap(geo, 'tryIncremental', (r) => void (r === true && t.incremental++));
  wrap(geo, 'tryColorOnly', (r) => void (r === true && t.colorOnly++));
  wrap(geo, 'build', () => void t.full++);
  wrap(geo, 'setDimming', () => void t.dim++);
  wrap(geo, 'refreshHighlight', () => void t.highlight++);
  wrap(labels, 'build', () => {
    t.labelBuild++;
    revealDirty = false;
  });
  wrap(labels, 'refreshFlags', () => void t.labelFlags++);
  const lab = labels as unknown as { rerunReveal: () => void };
  const rerun = lab.rerunReveal.bind(labels);
  lab.rerunReveal = () => {
    const before = labels.lastData?.buffers.revealZoom;
    rerun();
    if (labels.lastData?.buffers.revealZoom !== before) {
      t.labelReveal++;
      revealDirty = true;
    }
  };
  return () => revealDirty;
}

const tierTag = (a: Tally, b: Tally): string =>
  [
    b.incremental > a.incremental ? 'inc' : '',
    b.colorOnly > a.colorOnly ? 'color' : '',
    b.full > a.full ? 'full' : '',
  ].filter(Boolean).join('+') || '-';

// ─── One seed ─────────────────────────────────────────────────────────────────

interface SeedFailure {
  seed: number;
  report: string;
}

async function runSeed(seed: number, steps: number, total: Tally): Promise<SeedFailure | null> {
  const r = new Rng(seed);
  const cam = cameraFor(r);
  const model = initialModel(r);
  const trace: string[] = [
    `mount sections=${model.sections.length} camera=${cam.viewport ? `box(${cam.cx},${cam.cy},+-${cam.half})` : 'none'} zoom=${cam.zoom}`,
  ];
  let snap = snapOf(materialize(model), model.display, null);
  setStore(snap);
  const inc = await mount(cam, snap);
  const t = newTally();
  const revealDirty = instrument(inc.layer, t);
  const { geo, labels } = controllersOf(inc.layer);
  const fail = (at: number, what: string): SeedFailure => ({
    seed,
    report: [
      `renderer differential FAILED: seed=${seed} at step ${at} of ${steps}`,
      what,
      `replay: RENDER_DIFF_SEED=${seed} RENDER_DIFF_STEPS=${steps} pnpm exec vitest run src/components/Map/__tests__/rendererIncrementalDifferential.test.ts`,
      'trace (step [tier taken] change):',
      ...trace.map((line, i) => `  ${String(i).padStart(3)} ${line}`),
    ].join('\n'),
  });
  if (!geo.active || !labels.active) return fail(0, `controllers not active after mount (geo=${geo.active} labels=${labels.active})`);
  try {
    for (let i = 1; i <= steps; i++) {
      const before = { ...t };
      const res = step(r, model);
      const store = res.store ? materialize(model) : snap.store;
      const next = snapOf(store, model.display, snap);
      cam.gps = model.display.gpsOrigin;
      setStore(next);
      runEffects(inc.layer, snap, next);
      await settle();
      snap = next;
      t.steps++;
      trace.push(`[${tierTag(before, t)}] ${res.desc}`);

      const active = selectActiveSurveyData(useProjectStore.getState(), useUIStore.getState().geometryMode);
      const gotInc = captureRenderState(inc.layer, active);
      const fresh = await mount(cam, snap);
      const gotFresh = captureRenderState(fresh.layer, active);
      const skip = new Set<EntryGroup>(revealDirty() ? ['labelEmitted'] : []);
      if (skip.size > 0) t.revealSkips++;
      const diff = firstDifference(gotInc, gotFresh, skip);
      const gpu = gpuIncoherence(inc.layer, inc.shadow) ?? gpuIncoherence(fresh.layer, fresh.shadow);
      fresh.layer.release();
      if (diff) {
        return fail(i, [
          `first difference: ${diff.key}[${diff.index}] incremental=${diff.incremental} fresh=${diff.fresh}`,
          `fingerprints: incremental ${fingerprint(gotInc)} | fresh ${fingerprint(gotFresh)}`,
        ].join('\n'));
      }
      if (gpu) return fail(i, `GPU shadow incoherent with the CPU arrays: ${gpu}`);
    }
    return null;
  } finally {
    inc.layer.release();
    for (const k of Object.keys(total) as Array<keyof Tally>) total[k] += t[k];
  }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

beforeEach(() => {
  vi.stubGlobal('WebGL2RenderingContext', ShadowGl);
  vi.stubGlobal('WebGLBuffer', FakeWebGLBuffer);
  for (const k of ['info', 'warn', 'debug', 'error'] as const) vi.spyOn(console, k).mockImplementation(() => {});
  useUIStore.setState({ geometryMode: 'measured' });
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe('custom renderer: incremental updates equal a fresh build (#576 FZ-ORACLE)', () => {
  it(
    ONE_SEED !== undefined
      ? `seed ${ONE_SEED} x ${STEPS} steps`
      : `${SEEDS.length} seeds (${FIRST}..${FIRST + COUNT - 1} + regression seeds) x ${STEPS} steps`,
    async () => {
      const total = newTally();
      const failures: SeedFailure[] = [];
      for (const seed of SEEDS) {
        const f = await runSeed(seed, STEPS, total);
        if (f) failures.push(f);
        if (failures.length >= 10) break;
      }
      // The run's one-line tally: shows the fast paths were actually exercised.
      console.log(
        `[render-diff] seeds=${SEEDS.length} steps=${total.steps} tiers: incremental=${total.incremental} ` +
          `colorOnly=${total.colorOnly} full=${total.full} | side: dim=${total.dim} highlight=${total.highlight} ` +
          `labelBuild=${total.labelBuild} labelFlags=${total.labelFlags} labelReveal=${total.labelReveal} ` +
          `revealSkips=${total.revealSkips} | failing seeds: ${failures.map((f) => f.seed).join(',') || 'none'}`,
      );
      if (failures.length > 0) throw new Error(`${failures.length} failing seed(s): ${failures.map((f) => f.seed).join(', ')}\n\n${failures[0]!.report}`);
      if (ONE_SEED === undefined && envNum('RENDER_DIFF_SEEDS') === undefined) {
        // The CI set must actually exercise every tier and side channel.
        expect(total.incremental, 'incremental tier taken').toBeGreaterThan(0);
        expect(total.colorOnly, 'color-only tier taken').toBeGreaterThan(0);
        expect(total.full, 'full tier taken').toBeGreaterThan(0);
        expect(total.dim, 'dim side channel').toBeGreaterThan(0);
        expect(total.highlight, 'highlight side channel').toBeGreaterThan(0);
        expect(total.labelFlags, 'label flags side channel').toBeGreaterThan(0);
        expect(total.labelReveal, 'label reveal rerun').toBeGreaterThan(0);
      }
    },
    Math.max(60_000, SEEDS.length * STEPS * 60),
  );

  /** One commit changing the theme AND `change`, then the differential. */
  async function themeCommit(change: (d: Display) => void): Promise<void> {
    const model = initialModel(new Rng(12));
    const cam: Camera = { zoom: 18, viewport: false, cx: 0, cy: 0, half: 100, gps: null };
    const first = snapOf(materialize(model), model.display, null);
    setStore(first);
    const inc = await mount(cam, first);
    change(model.display);
    model.display.theme = 1;
    model.display.themeEpoch++;
    const next = snapOf(first.store, model.display, first);
    cam.gps = model.display.gpsOrigin;
    runEffects(inc.layer, first, next);
    await settle();
    const active = selectActiveSurveyData(useProjectStore.getState(), useUIStore.getState().geometryMode);
    const fresh = await mount(cam, next);
    const diff = firstDifference(captureRenderState(inc.layer, active), captureRenderState(fresh.layer, active));
    inc.layer.release();
    fresh.layer.release();
    expect(diff, JSON.stringify(diff)).toBeNull();
  }

  // FZ-ORACLE-R-1: applyThemeColors re-sent the previous flush's args over this
  // commit's pending update() (last call wins), dropping its change until the next one.
  it('FZ-ORACLE-R-1: a gps-origin change committed with a theme change is drawn', async () => {
    await themeCommit((d) => {
      d.gpsOrigin = { latitude: -35.1, longitude: 53.1 };
    });
  });

  it('FZ-ORACLE-R-1: a depth-gradient toggle committed with a theme change is drawn', async () => {
    await themeCommit((d) => {
      d.depthOn = true;
    });
  });
});
