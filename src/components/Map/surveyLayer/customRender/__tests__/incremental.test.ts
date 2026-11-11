import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap } from 'maplibre-gl';
import {
  emptySurveyData,
  shotKindToFlags,
  STATION_FLAG_IS_ZERO_DATUM,
  type SurveyData,
} from '../../../../../types/survey-data';
import type { DisplayColorContext } from '../../displayColor';
import { computeMercatorAnchor } from '../mercator';
import {
  sameGpsOrigin,
  sameSurveyLayout,
  sameColorGlobals,
  planIncrementalUpdate,
  coalesceVertexRuns,
} from '../incremental';

/**
 * Incremental-edit fast path (#customRender, roadmap task #5). Two layers of
 * coverage:
 *  - Pure diff/gate helpers (no mocks).
 *  - The controller's incremental apply, driven with a fake GL layer, proving
 *    the mutated buffers are BYTE-IDENTICAL to a full rebuild of the same
 *    geometry, and that structural / global-color changes fall back to a rebuild.
 */

// ─── Fixture builders ──────────────────────────────────────────────────────

const THEME = {
  stationDefault: '#ffffff',
  zeroDatum: '#00ffff',
  strokeColor: '#000000',
  depthShallow: '#0000ff',
  depthDeep: '#ff0000',
  severityError: '#ff0000',
  severityWarning: '#ffff00',
  severityClear: '#00000000',
};

function ctx(over: Partial<DisplayColorContext> = {}): DisplayColorContext {
  return {
    showDepthGradient: false,
    showIssueSeverity: false,
    depthRange: { min: 0, max: 50 },
    themeColors: THEME as unknown as DisplayColorContext['themeColors'],
    ...over,
  } as DisplayColorContext;
}

interface SecSpec {
  id: number;
  color: string;
  stations: Array<[number, number, number]>;
  refShots?: Set<number>;
  zeroDatum?: Set<number>; // station indices flagged zero-datum
}

function buildSurvey(specs: SecSpec[]): SurveyData {
  const d = emptySurveyData();
  const shotIds: number[] = [];
  const shotSectionIds: number[] = [];
  const shotFlags: number[] = [];
  const shotDepthIn: number[] = [];
  const shotDepthOut: number[] = [];
  const stSectionIds: number[] = [];
  const stIndices: number[] = [];
  const stCoords: number[] = [];
  const stFlags: number[] = [];
  let shotOffset = 0;
  for (const s of specs) {
    const shotCount = Math.max(0, s.stations.length - 1);
    d.sections.push({
      id: s.id, name: null, surveyor: null, date: null, color: s.color,
      direction: 'in' as never, quality: 0, next_shot_id: shotCount + 1, shotOffset, shotCount,
    });
    for (let i = 0; i < shotCount; i++) {
      shotIds.push(i);
      shotSectionIds.push(s.id);
      shotFlags.push(s.refShots?.has(i) ? shotKindToFlags('Reference') : 0);
      shotDepthIn.push(-i);
      shotDepthOut.push(-(i + 1));
    }
    shotOffset += shotCount;
    s.stations.forEach((c, idx) => {
      stSectionIds.push(s.id);
      stIndices.push(idx);
      stCoords.push(c[0], c[1], c[2]);
      stFlags.push(s.zeroDatum?.has(idx) ? STATION_FLAG_IS_ZERO_DATUM : 0);
    });
  }
  d.shotCount = shotIds.length;
  d.shotIds = Uint32Array.from(shotIds);
  d.shotSectionIds = Uint32Array.from(shotSectionIds);
  d.shotFlags = Uint8Array.from(shotFlags);
  d.shotDepthIn = Float32Array.from(shotDepthIn);
  d.shotDepthOut = Float32Array.from(shotDepthOut);
  d.stationCount = stSectionIds.length;
  d.stationSectionIds = Uint32Array.from(stSectionIds);
  d.stationIndices = Uint32Array.from(stIndices);
  d.stationCoords = Float64Array.from(stCoords);
  d.stationFlags = Uint8Array.from(stFlags);
  return d;
}

function cloneSurvey(d: SurveyData): SurveyData {
  return {
    ...d,
    sections: d.sections.map((s) => ({ ...s })),
    stationCoords: Float64Array.from(d.stationCoords),
    stationFlags: Uint8Array.from(d.stationFlags),
    shotFlags: Uint8Array.from(d.shotFlags),
    shotDepthIn: Float32Array.from(d.shotDepthIn),
    shotDepthOut: Float32Array.from(d.shotDepthOut),
  };
}

/** A survey with pinned bbox corners (never edited) so interior edits do not
 *  shift the mercator anchor; several editable interior sections. */
function pinnedSurvey(): SurveyData {
  return buildSurvey([
    // Section 0: two pins at the extreme corners; keeps the bbox fixed.
    { id: 0, color: '#111111', stations: [[-1000, -1000, -1], [1000, 1000, -1]] },
    { id: 1, color: '#ff0000', stations: [[0, 0, -1], [50, 10, -2], [80, 40, -3]], zeroDatum: new Set([0]) },
    { id: 2, color: '#00ff00', stations: [[-100, 20, -1], [-60, -30, -2], [-20, 5, -3], [10, -10, -4]] },
    { id: 3, color: '#0000ff', stations: [[200, -200, -1], [230, -150, -2]], refShots: new Set([0]) },
  ]);
}

// ─── Pure diff / gate helpers ──────────────────────────────────────────────

describe('sameGpsOrigin', () => {
  it('treats both-null and equal values as same, differing values as not', () => {
    expect(sameGpsOrigin(null, null)).toBe(true);
    const a = { latitude: 1, longitude: 2 } as never;
    expect(sameGpsOrigin(a, a)).toBe(true);
    expect(sameGpsOrigin({ latitude: 1, longitude: 2 } as never, { latitude: 1, longitude: 2 } as never)).toBe(true);
    expect(sameGpsOrigin(null, a)).toBe(false);
    expect(sameGpsOrigin({ latitude: 1, longitude: 2 } as never, { latitude: 1, longitude: 3 } as never)).toBe(false);
  });
});

describe('sameSurveyLayout', () => {
  it('true for a pure position edit, false for structural changes', () => {
    const a = pinnedSurvey();
    const moved = cloneSurvey(a);
    moved.stationCoords[3 * 3] = 999; // move some station's x
    expect(sameSurveyLayout(a, moved)).toBe(true);

    const dropped = cloneSurvey(a);
    dropped.sections = dropped.sections.slice(0, 3);
    expect(sameSurveyLayout(a, dropped)).toBe(false);

    const remapped = cloneSurvey(a);
    remapped.stationSectionIds = Uint32Array.from(remapped.stationSectionIds);
    remapped.stationSectionIds[remapped.stationCount - 1] = 99;
    expect(sameSurveyLayout(a, remapped)).toBe(false);

    // A shotId that changed slot under an otherwise-stable layout must force a
    // full rebuild: the incremental path skips the dim/hide/highlight buffers,
    // which the immovable-feature predicates key on shotIds.
    const shotRekeyed = cloneSurvey(a);
    shotRekeyed.shotIds = Uint32Array.from(shotRekeyed.shotIds);
    shotRekeyed.shotIds[shotRekeyed.shotCount - 1] = 987654;
    expect(sameSurveyLayout(a, shotRekeyed)).toBe(false);
  });
});

describe('sameColorGlobals', () => {
  it('false when the theme ref, a mode, or the depth range changes', () => {
    expect(sameColorGlobals(ctx(), ctx())).toBe(true);
    expect(sameColorGlobals(ctx(), ctx({ themeColors: { ...THEME } as never }))).toBe(false);
    expect(sameColorGlobals(ctx(), ctx({ showDepthGradient: true }))).toBe(false);
    expect(sameColorGlobals(ctx(), ctx({ depthRange: { min: 0, max: 60 } }))).toBe(false);
    // Severity on: the arrays are rebuilt per flush (new ref) -> never same.
    const sev = { shotSeverity: new Uint8Array(0), stationSeverity: new Uint8Array(0) };
    expect(sameColorGlobals(
      ctx({ showIssueSeverity: true, issueSeverity: sev }),
      ctx({ showIssueSeverity: true, issueSeverity: { ...sev } }),
    )).toBe(false);
  });

  it('treats analytical-overlay changes as color-global (forces a rebuild)', () => {
    const shots = new Map([[1, 2]]);
    const stations = new Map([[3, 4]]);
    const ov = { mode: 'distance' as const, shots, stations, max: 100 };
    // Enter / exit: null <-> overlay.
    expect(sameColorGlobals(ctx(), ctx({ overlay: ov }))).toBe(false);
    expect(sameColorGlobals(ctx({ overlay: ov }), ctx())).toBe(false);
    // Same map refs (a geometry flush that left the overlay untouched) -> same.
    expect(sameColorGlobals(ctx({ overlay: ov }), ctx({ overlay: { ...ov } }))).toBe(true);
    // Recompute (fresh map ref) / mode / scale change -> not same.
    expect(sameColorGlobals(
      ctx({ overlay: ov }),
      ctx({ overlay: { ...ov, shots: new Map([[1, 2]]) } }),
    )).toBe(false);
    expect(sameColorGlobals(
      ctx({ overlay: ov }),
      ctx({ overlay: { ...ov, mode: 'gasReach' } }),
    )).toBe(false);
    expect(sameColorGlobals(ctx({ overlay: ov }), ctx({ overlay: { ...ov, max: 101 } }))).toBe(false);
  });

  it('forces a rebuild whenever the move-comment tint is active (enter/exit/while-on)', () => {
    // Enter / exit the tool.
    expect(sameColorGlobals(ctx(), ctx({ commentTint: true }))).toBe(false);
    expect(sameColorGlobals(ctx({ commentTint: true }), ctx())).toBe(false);
    // While on: a flush can relocate a comment (recolor a feature) without any
    // coord/flag/section-color change the diff tracks, so stay conservative.
    expect(sameColorGlobals(ctx({ commentTint: true }), ctx({ commentTint: true }))).toBe(false);
  });
});

describe('coalesceVertexRuns', () => {
  it('collapses contiguous slots into runs, splits gaps', () => {
    expect(coalesceVertexRuns([2, 3, 4, 7, 8], 4)).toEqual([[8, 12], [28, 8]]);
    expect(coalesceVertexRuns([5], 4)).toEqual([[20, 4]]);
    expect(coalesceVertexRuns([], 4)).toEqual([]);
  });
});

describe('planIncrementalUpdate', () => {
  // The controller feeds the cached anchor; recompute it here the same way.
  const anchorOf = (d: SurveyData): [number, number] => computeMercatorAnchor(d, null);

  it('plans a single-station move: its section dirty, the station reprojected', () => {
    const prev = pinnedSurvey();
    const [ax, ay] = anchorOf(prev);
    const next = cloneSurvey(prev);
    // Move section 2's second station (global index 2 pins + 3 in sec1 = 5).
    const g = 5;
    next.stationCoords[g * 3] = next.stationCoords[g * 3]! + 5;
    const plan = planIncrementalUpdate(prev, next, ctx(), ctx(), null, null, ax, ay);
    expect(plan).not.toBeNull();
    expect(plan!.reprojectStationGlobals).toEqual([g]);
    expect(plan!.dirtySectionIdxs).toEqual([2]); // section id 2 is index 2
  });

  it('plans a recolor: section dirty, no reprojection', () => {
    const prev = pinnedSurvey();
    const [ax, ay] = anchorOf(prev);
    const next = cloneSurvey(prev);
    next.sections[1]!.color = '#abcdef';
    const plan = planIncrementalUpdate(prev, next, ctx(), ctx(), null, null, ax, ay);
    expect(plan).not.toBeNull();
    expect(plan!.reprojectStationGlobals).toEqual([]);
    expect(plan!.dirtySectionIdxs).toEqual([1]);
  });

  it('falls back (null) on structural change, anchor shift, depthRange shift, gps + theme change', () => {
    const prev = pinnedSurvey();
    const [ax, ay] = anchorOf(prev);

    const structural = cloneSurvey(prev);
    structural.sections = structural.sections.slice(0, 3);
    expect(planIncrementalUpdate(prev, structural, ctx(), ctx(), null, null, ax, ay)).toBeNull();

    // Move a PIN corner -> the bbox (and anchor) shifts -> full rebuild.
    const anchorShift = cloneSurvey(prev);
    anchorShift.stationCoords[0] = -5000;
    expect(planIncrementalUpdate(prev, anchorShift, ctx(), ctx(), null, null, ax, ay)).toBeNull();

    const move = cloneSurvey(prev);
    move.stationCoords[5 * 3] = move.stationCoords[5 * 3]! + 5;
    // depthRange shift recolors every depth section -> full rebuild.
    expect(planIncrementalUpdate(
      prev, move, ctx({ showDepthGradient: true }), ctx({ showDepthGradient: true, depthRange: { min: 0, max: 99 } }), null, null, ax, ay,
    )).toBeNull();
    // gps origin change reprojects everything.
    expect(planIncrementalUpdate(prev, move, ctx(), ctx(), null, { latitude: 1, longitude: 1 } as never, ax, ay)).toBeNull();
    // theme change recolors everything.
    expect(planIncrementalUpdate(prev, move, ctx(), ctx({ themeColors: { ...THEME } as never }), null, null, ax, ay)).toBeNull();
  });
});

// ─── Controller incremental apply (byte-identity + fallbacks) ──────────────

const h = vi.hoisted(() => {
  const state = { lineUploads: 0, circleUploads: 0, circleIndexUploads: 0, lineIndexUploads: 0, fullUpdates: 0 };
  class FakeLayer {
    readonly id: string;
    data: unknown;
    constructor(id: string, data: unknown) {
      this.id = id;
      this.data = data;
    }
    onAdd(): void {}
    isDisabled(): boolean {
      return false;
    }
    getDisabledReason(): string | null {
      return null;
    }
    update(data: unknown): void {
      this.data = data;
      state.fullUpdates++;
    }
    setShotsVisible(): void {}
    setStationsVisible(): void {}
    setReferenceShotsVisible(): void {}
    setJumpShotsVisible(): void {}
    updateDim(): void {}
    updateHide(): void {}
    updateHighlight(): void {}
    uploadLineVertexRange(): void {
      state.lineUploads++;
    }
    uploadCircleVertexRange(): void {
      state.circleUploads++;
    }
    uploadCircleIndices(): void {
      state.circleIndexUploads++;
    }
    uploadLineIndices(): void {
      state.lineIndexUploads++;
    }
  }
  return { state, FakeLayer };
});

vi.mock('../surveyCustomLayer', () => ({ SurveyCustomLayer: h.FakeLayer }));

// vi.mock is hoisted, so this static import already binds to the fake layer.
import { SurveyCustomController, type CustomSyncParams } from '../controller';
import type { CustomRenderData } from '../surveyCustomLayer';
type Ctrl = InstanceType<typeof SurveyCustomController>;

function makeFakeMap(): MaplibreMap {
  const existing = new Set<string>(['survey-shots', 'survey-shot-gradients', 'survey-stations']);
  return {
    getStyle: () => ({}),
    getLayer: (id: string) => (existing.has(id) ? { id } : undefined),
    addLayer: (layer: { id: string; onAdd?: (m: unknown, gl: unknown) => void }) => {
      existing.add(layer.id);
      layer.onAdd?.(undefined, undefined);
    },
    removeLayer: (id: string) => existing.delete(id),
    setLayoutProperty: () => {},
    triggerRepaint: () => {},
  } as unknown as MaplibreMap;
}

function params(surveyData: SurveyData, colorCtx: DisplayColorContext = ctx()): CustomSyncParams {
  return { surveyData, gpsOrigin: null, colorCtx, shotWidthPx: 2, stationRadiusPx: 3, stationStrokePx: 1 };
}

const lineArrays = ['posA', 'posB', 'endpoint', 'side', 'colorFrom', 'colorTo', 'reference', 'dim', 'hide', 'highlight', 'indices', 'sectionRanges'] as const;
const circleArrays = ['center', 'corner', 'fill', 'stroke', 'sizeMul', 'dim', 'hide', 'highlight', 'allReference', 'indices'] as const;

function expectDataEqual(a: CustomRenderData, b: CustomRenderData): void {
  expect(a.anchorX).toBe(b.anchorX);
  expect(a.anchorY).toBe(b.anchorY);
  for (const k of lineArrays) {
    expect(Array.from(a.line[k] as ArrayLike<number>)).toEqual(Array.from(b.line[k] as ArrayLike<number>));
  }
  for (const k of circleArrays) {
    expect(Array.from(a.circle[k] as ArrayLike<number>)).toEqual(Array.from(b.circle[k] as ArrayLike<number>));
  }
}

/** Deterministic PRNG (mulberry32). */
function rng(seed: number): () => number {
  let a = seed >>> 0;
  return () => {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

describe('SurveyCustomController incremental apply', () => {
  beforeEach(() => {
    h.state.lineUploads = 0;
    h.state.circleUploads = 0;
    h.state.circleIndexUploads = 0;
    h.state.lineIndexUploads = 0;
    h.state.fullUpdates = 0;
    vi.spyOn(console, 'info').mockImplementation(() => {});
    vi.spyOn(console, 'warn').mockImplementation(() => {});
  });
  afterEach(() => vi.restoreAllMocks());

  const dataOf = (c: Ctrl): CustomRenderData => (c as unknown as { lastData: CustomRenderData }).lastData;

  it('position edits stay byte-identical to a full rebuild (property test)', () => {
    const rand = rng(0xC0FFEE);
    let incrementalRuns = 0;
    for (let iter = 0; iter < 40; iter++) {
      const prev = pinnedSurvey();
      const c = new SurveyCustomController(makeFakeMap());
      c.sync(params(prev)); // full build (enable)

      const next = cloneSurvey(prev);
      // Move 1-3 interior stations (never the two pins at globals 0,1).
      const moves = 1 + Math.floor(rand() * 3);
      for (let m = 0; m < moves; m++) {
        const g = 2 + Math.floor(rand() * (next.stationCount - 2));
        next.stationCoords[g * 3] = next.stationCoords[g * 3]! + (rand() - 0.5) * 40;
        next.stationCoords[g * 3 + 1] = next.stationCoords[g * 3 + 1]! + (rand() - 0.5) * 40;
      }

      // enable() constructs (no update()); an incremental sync sub-uploads and
      // never calls update(), so a zero fullUpdates delta + a line sub-upload
      // marks the fast path.
      const fuBefore = h.state.fullUpdates;
      const luBefore = h.state.lineUploads;
      c.sync(params(next));
      if (h.state.fullUpdates === fuBefore && h.state.lineUploads > luBefore) incrementalRuns++;

      // Reference: a fresh controller full-builds `next` from scratch.
      const ref = new SurveyCustomController(makeFakeMap());
      ref.sync(params(next));
      expectDataEqual(dataOf(c), dataOf(ref));
      c.destroy();
      ref.destroy();
    }
    // Pin the exact fast-path count for this seed (37/40), not `> 0`: a
    // regression that silently fell back to full-rebuild would still pass `> 0`
    // while the byte-identity oracle held on the rebuild path, leaving the
    // incremental path effectively dead. The 3 non-incremental iterations are the
    // legitimate `dirty.size > sections/2` fallback (a 3-move iteration dirtying
    // all three interior sections of the 4-section fixture).
    expect(incrementalRuns).toBe(37);
  });

  // Kind edits are what move a station between draw ranks (real-line markers on
  // top), so the circle index order must follow them too. Random kind flips on
  // the interior sections, mixed with a position edit, against a fresh rebuild.
  it('shot-kind edits stay byte-identical to a full rebuild, both draw orders included (property test)', () => {
    const rand = rng(0x5EED);
    let incrementalRuns = 0;
    let reorders = 0;
    let lineReorders = 0;
    for (let iter = 0; iter < 40; iter++) {
      let cur = pinnedSurvey();
      const c = new SurveyCustomController(makeFakeMap());
      c.sync(params(cur));
      // A few edits in sequence, so a later edit starts from reordered indices.
      for (let step = 0; step < 3; step++) {
        const next = cloneSurvey(cur);
        // Interior shots are globals 1..6 (section 0's pin shot is global 0).
        const flips = 1 + Math.floor(rand() * 2);
        for (let f = 0; f < flips; f++) {
          const gi = 1 + Math.floor(rand() * 6);
          next.shotFlags[gi] = Math.floor(rand() * 3); // Normal / Jump / Reference
        }
        if (rand() < 0.5) {
          const g = 2 + Math.floor(rand() * (next.stationCount - 2));
          next.stationCoords[g * 3] = next.stationCoords[g * 3]! + (rand() - 0.5) * 20;
        }
        const fu = h.state.fullUpdates;
        const iu = h.state.circleIndexUploads;
        const lu = h.state.lineIndexUploads;
        c.sync(params(next));
        if (h.state.fullUpdates === fu) incrementalRuns++;
        if (h.state.circleIndexUploads > iu) reorders++;
        if (h.state.lineIndexUploads > lu) lineReorders++;
        const ref = new SurveyCustomController(makeFakeMap());
        ref.sync(params(next));
        expectDataEqual(dataOf(c), dataOf(ref));
        ref.destroy();
        cur = next;
      }
      c.destroy();
    }
    // Pinned for this seed, like the position property test: 117 of the 120
    // edits take the incremental path (3 dirty most sections and rebuild); 82 of
    // them rewrite the circle draw order and 91 the line draw order. A
    // regression that fell back to rebuilds, or never reordered, would still
    // pass the byte-identity check alone.
    expect(incrementalRuns).toBe(117);
    expect(reorders).toBe(82);
    expect(lineReorders).toBe(91);
  });

  it('depth-mode recolor via a z-move stays byte-identical', () => {
    const prev = pinnedSurvey();
    const dctx = ctx({ showDepthGradient: true, depthRange: { min: 0, max: 4 } });
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev, dctx));
    const next = cloneSurvey(prev);
    next.stationCoords[5 * 3 + 2] = -2.5; // z change within the depth range
    c.sync(params(next, dctx));
    const ref = new SurveyCustomController(makeFakeMap());
    ref.sync(params(next, dctx));
    expectDataEqual(dataOf(c), dataOf(ref));
  });

  // The raised section (the selection's) stays on top through a kind edit
  // elsewhere: the incremental reorder must honour it as the rebuild does.
  it('a kind edit keeps the raised section on top, as a rebuild does', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.setRaisedSection(3); // section 3's lone shot is a Reference
    c.sync(params(prev));
    const fu = h.state.fullUpdates;
    const iu = h.state.circleIndexUploads;
    const lu = h.state.lineIndexUploads;
    const jump = cloneSurvey(prev);
    jump.shotFlags[3] = shotKindToFlags('Jump'); // section 2's first shot
    c.sync(params(jump));
    expect(h.state.fullUpdates).toBe(fu); // incremental, not a rebuild
    expect(h.state.circleIndexUploads).toBe(iu + 1);
    expect(h.state.lineIndexUploads).toBe(lu + 1);
    const ref = new SurveyCustomController(makeFakeMap());
    ref.setRaisedSection(3);
    ref.sync(params(jump));
    expectDataEqual(dataOf(c), dataOf(ref));
    // The raise is what keeps section 3 last: unraised, its reference draws first.
    const plain = new SurveyCustomController(makeFakeMap());
    plain.sync(params(jump));
    expect(Array.from(dataOf(c).line.indices)).not.toEqual(Array.from(dataOf(plain).line.indices));
    expect(Array.from(dataOf(c).circle.indices)).not.toEqual(Array.from(dataOf(plain).circle.indices));
    for (const x of [c, ref, plain]) x.destroy();
  });

  // The circle index buffer draws stations by rank (real-line markers on top).
  // A kind edit can move a station between ranks: the incremental path must
  // redo the draw order (and re-upload it) to stay byte-identical to a rebuild,
  // and leave it alone when no rank moved.
  it('a kind edit redoes the line draw order and, when a station rank moved, the circle one', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev));
    const fu = h.state.fullUpdates;

    // Section 2's first shot (global 3) Normal -> Jump: station (2,0) touches
    // nothing else, so it drops from real to jump rank.
    const jump = cloneSurvey(prev);
    jump.shotFlags[3] = shotKindToFlags('Jump');
    c.sync(params(jump));
    expect(h.state.fullUpdates).toBe(fu); // incremental, not a rebuild
    expect(h.state.circleIndexUploads).toBe(1);
    expect(h.state.lineIndexUploads).toBe(1); // the jump shot now draws under the real ones
    const ref = new SurveyCustomController(makeFakeMap());
    ref.sync(params(jump));
    expectDataEqual(dataOf(c), dataOf(ref));

    // A position edit moves no rank: no index re-upload.
    const moved = cloneSurvey(jump);
    moved.stationCoords[5 * 3] = moved.stationCoords[5 * 3]! + 3;
    c.sync(params(moved));
    expect(h.state.fullUpdates).toBe(fu);
    expect(h.state.circleIndexUploads).toBe(1);
    expect(h.state.lineIndexUploads).toBe(1);

    // And back to Normal: real again, drawn last.
    const back = cloneSurvey(moved);
    back.shotFlags[3] = 0;
    c.sync(params(back));
    expect(h.state.circleIndexUploads).toBe(2);
    expect(h.state.lineIndexUploads).toBe(2);
    const ref2 = new SurveyCustomController(makeFakeMap());
    ref2.sync(params(back));
    expectDataEqual(dataOf(c), dataOf(ref2));
    for (const x of [c, ref, ref2]) x.destroy();
  });

  it('a structural change falls back to a full rebuild', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev));
    expect(h.state.fullUpdates).toBe(0); // enable() constructs, does not update()
    const next = cloneSurvey(prev);
    next.sections = next.sections.slice(0, 3); // drop a section (structural)
    // Rebuild the SoA so counts match the trimmed sections.
    const trimmed = buildSurvey([
      { id: 0, color: '#111111', stations: [[-1000, -1000, -1], [1000, 1000, -1]] },
      { id: 1, color: '#ff0000', stations: [[0, 0, -1], [50, 10, -2], [80, 40, -3]], zeroDatum: new Set([0]) },
      { id: 2, color: '#00ff00', stations: [[-100, 20, -1], [-60, -30, -2], [-20, 5, -3], [10, -10, -4]] },
    ]);
    c.sync(params(trimmed));
    expect(h.state.fullUpdates).toBe(1); // took the full-rebuild path
    expect(h.state.lineUploads).toBe(0);
    const ref = new SurveyCustomController(makeFakeMap());
    ref.sync(params(trimmed));
    expectDataEqual(dataOf(c), dataOf(ref));
  });

  it('keeps the drag-hide flag consistent across an incremental edit', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev));
    // Drag begins: hide section id 2.
    c.setDragHide({ sectionIds: new Set([2]) });
    // Commit-time flush moves a station in the hidden section.
    const next = cloneSurvey(prev);
    next.stationCoords[6 * 3] = next.stationCoords[6 * 3]! + 3;
    c.sync(params(next));
    // A fresh full build of `next` WITH the same hide state is the oracle.
    const ref = new SurveyCustomController(makeFakeMap());
    ref.sync(params(next));
    ref.setDragHide({ sectionIds: new Set([2]) });
    ref.sync(params(next));
    expectDataEqual(dataOf(c), dataOf(ref));
    // Section id 2 (index 2) stations are all hidden in the incremental result.
    const data = dataOf(c);
    expect(Array.from(data.circle.hide).some((v) => v === 1)).toBe(true);
  });

  it('applies the segment highlight on the cached buffers and matches a full build', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev));
    // No emphasis yet.
    expect(Array.from(dataOf(c).circle.highlight).every((v) => v === 0)).toBe(true);
    expect(Array.from(dataOf(c).line.highlight).every((v) => v === 0)).toBe(true);
    // Move-Segment hover emphasizes the connected section id 2 at HOVER level (2).
    c.setHighlight([2]);
    expect(Array.from(dataOf(c).circle.highlight).some((v) => v === 2)).toBe(true);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 2)).toBe(true);
    // Setting the highlight BEFORE sync makes the full build reproduce it: the
    // oracle for the cheap path must be byte-identical.
    const ref = new SurveyCustomController(makeFakeMap());
    ref.setHighlight([2]);
    ref.sync(params(prev));
    expectDataEqual(dataOf(c), dataOf(ref));
    // Hover leaves geometry (or drag begins): clearing zeroes every highlight flag.
    c.setHighlight([]);
    expect(Array.from(dataOf(c).circle.highlight).every((v) => v === 0)).toBe(true);
    expect(Array.from(dataOf(c).line.highlight).every((v) => v === 0)).toBe(true);
  });

  it('section hover (level 2) and selection (level 1) merge with hover winning', () => {
    const prev = pinnedSurvey();
    const c = new SurveyCustomController(makeFakeMap());
    c.sync(params(prev));
    // Selection alone: connected section id 2 emphasized at SELECTION level (1).
    c.setSectionSelect([2]);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 1)).toBe(true);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 2)).toBe(false);
    // Hovering the SAME section promotes it to hover level (2); hover wins.
    c.setSectionHover([2]);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 2)).toBe(true);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 1)).toBe(false);
    // Full build with both sources set reproduces the merged levels byte-for-byte.
    const ref = new SurveyCustomController(makeFakeMap());
    ref.setSectionSelect([2]);
    ref.setSectionHover([2]);
    ref.sync(params(prev));
    expectDataEqual(dataOf(c), dataOf(ref));
    // Leaving the hover drops back to selection level (1); clearHighlights zeroes all.
    c.setSectionHover([]);
    expect(Array.from(dataOf(c).line.highlight).some((v) => v === 1)).toBe(true);
    c.clearHighlights();
    expect(Array.from(dataOf(c).line.highlight).every((v) => v === 0)).toBe(true);
    expect(Array.from(dataOf(c).circle.highlight).every((v) => v === 0)).toBe(true);
  });
});
