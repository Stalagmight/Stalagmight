import { describe, expect, it } from 'vitest';
import {
  buildLabelDescriptors,
  buildLabelBuffers,
  buildLabelBuffersFromShaped,
  buildLabelRevealZooms,
  shapeDescriptors,
  type LabelBuildContext,
  type LabelDescriptor,
} from '../labelTessellate';
import { LABEL_KIND, LABEL_FLAG } from '../labelShaders';
import { GlyphAtlas, SDF_BUFFER, type GlyphRasterizer, type RasterizedGlyph } from '../glyphAtlas';
import { packCssColor } from '../colorPack';
import {
  emptySurveyData,
  shotKindToFlags,
  type SurveyData,
} from '../../../../../types/survey-data';
import { commentColorForKey } from '../../displayColor';

/**
 * Label descriptor extraction + per-glyph buffer emission (#customRender labels,
 * Phase 1). Locks: the 5-kind descriptor set (with comments), the ordinals +
 * flag bits (reference / all-reference / first-in-section), and the buffer
 * output shape (4 verts + 6 indices per glyph, shared per-label attributes,
 * atlas-px uv corners).
 */

/** Fixed-metric fake rasterizer (12px advance, 10x18 box) so any text shapes to
 *  one quad per non-space char. */
function fakeAtlas(): GlyphAtlas {
  const raster: GlyphRasterizer = (c: string): RasterizedGlyph => {
    const empty = c === ' ';
    const w = empty ? 0 : 10;
    const h = empty ? 0 : 18;
    return {
      data: new Uint8Array(Math.max(0, w * h)),
      width: w,
      height: h,
      glyphWidth: Math.max(0, w - 2 * SDF_BUFFER),
      glyphHeight: Math.max(0, h - 2 * SDF_BUFFER),
      glyphTop: 15,
      glyphLeft: 1,
      glyphAdvance: empty ? 6 : 12,
    };
  };
  return new GlyphAtlas({ rasterizer: raster });
}

/** One section, 2 shots (shot 1 is a reference shot), 3 stations; a comment on
 *  shot 0 and on station g2. */
function fixture(): SurveyData {
  const d = emptySurveyData();
  d.sections = [
    { id: 1, name: 'Alpha', surveyor: null, date: null, color: '#4ecdc4', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
  ];
  d.shotCount = 2;
  d.shotIds = new Uint32Array([0, 1]);
  d.shotSectionIds = new Uint32Array([1, 1]);
  d.shotFlags = new Uint8Array([0, shotKindToFlags('Reference')]);
  d.shotAzimuthIn = new Float32Array([312, 45]);
  d.shotLength = new Float32Array([4, 3]);
  d.shotDepthIn = new Float32Array([1, 2]);
  d.shotDepthOut = new Float32Array([2, 4]);
  d.stationCount = 3;
  d.stationSectionIds = new Uint32Array([1, 1, 1]);
  d.stationIndices = new Uint32Array([0, 1, 2]);
  d.stationCoords = new Float64Array([0, 0, 0, 10, 0, -1, 20, 0, -2]);
  d.stationFlags = new Uint8Array([0, 0, 0]);
  d.shotComments = new Map([[0, 'shot note']]);
  d.stationComments = new Map([[2, 'stn note']]);
  return d;
}

const CTX: LabelBuildContext = {
  colorCtx: {
    showDepthGradient: false,
    depthShallow: '#00bfff',
    depthDeep: '#ff4444',
    depthMin: -2,
    depthMax: 0,
    labelColor: '#e0e0e0',
  },
};

describe('buildLabelDescriptors', () => {
  it('emits the 5 kinds: shot/station labels + shot/station comments + section name', () => {
    const d = fixture();
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const byKind = (k: number): LabelDescriptor[] => descs.filter((x) => x.kind === k);
    expect(byKind(LABEL_KIND.SHOT_LABEL)).toHaveLength(2);
    expect(byKind(LABEL_KIND.SHOT_COMMENT)).toHaveLength(1);
    expect(byKind(LABEL_KIND.STATION_LABEL)).toHaveLength(3);
    expect(byKind(LABEL_KIND.STATION_COMMENT)).toHaveLength(1);
    expect(byKind(LABEL_KIND.SECTION_NAME)).toHaveLength(1);
  });

  it('station names mode: named stations only, labelled by name, everything else as for the ID', () => {
    const d = fixture();
    d.stationNames = new Map([[0, '7'], [2, ' \u202e9 ']]);
    const ids = buildLabelDescriptors(d, null, 0, 0, CTX).filter((x) => x.kind === LABEL_KIND.STATION_LABEL);
    const names = buildLabelDescriptors(d, null, 0, 0, { ...CTX, stationLabelText: 'name' }).filter(
      (x) => x.kind === LABEL_KIND.STATION_LABEL,
    );
    // g1 has no name, so it gets no label (no fallback to its index); g2's name is sanitized.
    expect(names.map((x) => [x.stationGlobal, x.text])).toEqual([
      [0, '7'],
      [2, '9'],
    ]);
    // Ordinal, flags, anchor, color and owner match the ID label for the same
    // station, so names declutter, decimate, dim and hide exactly like the IDs.
    for (const n of names) {
      const id = ids.find((x) => x.stationGlobal === n.stationGlobal)!;
      expect({ ...n, text: id.text }).toEqual(id);
    }
  });

  it('station names mode skips names that sanitize to nothing and leaves the other kinds alone', () => {
    const d = fixture();
    d.stationNames = new Map([[1, '\u2066\u2069'], [2, '   ']]);
    const ids = buildLabelDescriptors(d, null, 0, 0, CTX);
    const names = buildLabelDescriptors(d, null, 0, 0, { ...CTX, stationLabelText: 'name' });
    expect(names.filter((x) => x.kind === LABEL_KIND.STATION_LABEL)).toEqual([]);
    expect(names.filter((x) => x.kind !== LABEL_KIND.STATION_LABEL)).toEqual(
      ids.filter((x) => x.kind !== LABEL_KIND.STATION_LABEL),
    );
    // The explicit 'id' mode is the default.
    expect(buildLabelDescriptors(d, null, 0, 0, { ...CTX, stationLabelText: 'id' })).toEqual(ids);
  });

  it('assigns decimation ordinals (shot running index, station global index)', () => {
    const d = fixture();
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const shots = descs.filter((x) => x.kind === LABEL_KIND.SHOT_LABEL);
    expect(shots.map((s) => s.ordinal)).toEqual([0, 1]);
    const stations = descs.filter((x) => x.kind === LABEL_KIND.STATION_LABEL);
    expect(stations.map((s) => s.ordinal).sort((a, b) => a - b)).toEqual([0, 1, 2]);
    // Comments + section names never decimate.
    for (const k of [LABEL_KIND.SHOT_COMMENT, LABEL_KIND.STATION_COMMENT, LABEL_KIND.SECTION_NAME]) {
      for (const d2 of descs.filter((x) => x.kind === k)) expect(d2.ordinal).toBe(0);
    }
  });

  it('sets REFERENCE on the reference shot label only', () => {
    const d = fixture();
    const shots = buildLabelDescriptors(d, null, 0, 0, CTX).filter((x) => x.kind === LABEL_KIND.SHOT_LABEL);
    expect(shots[0]!.flags & LABEL_FLAG.REFERENCE).toBe(0);
    expect(shots[1]!.flags & LABEL_FLAG.REFERENCE).toBe(LABEL_FLAG.REFERENCE);
  });

  it('sets FIRST_IN_SECTION on the first station, ALL_REFERENCE on the all-reference station', () => {
    const d = fixture();
    const stations = buildLabelDescriptors(d, null, 0, 0, CTX).filter((x) => x.kind === LABEL_KIND.STATION_LABEL);
    const byOrdinal = new Map(stations.map((s) => [s.ordinal, s]));
    // Station g0 (index 0) is first-in-section; g2 only touches the reference
    // shot so it is all-reference; g0/g1 touch a non-reference shot.
    expect(byOrdinal.get(0)!.flags & LABEL_FLAG.FIRST_IN_SECTION).toBe(LABEL_FLAG.FIRST_IN_SECTION);
    expect(byOrdinal.get(1)!.flags & LABEL_FLAG.FIRST_IN_SECTION).toBe(0);
    expect(byOrdinal.get(0)!.flags & LABEL_FLAG.ALL_REFERENCE).toBe(0);
    expect(byOrdinal.get(2)!.flags & LABEL_FLAG.ALL_REFERENCE).toBe(LABEL_FLAG.ALL_REFERENCE);
  });

  it('sets REFERENCE on a reference-shot comment and ALL_REFERENCE on an all-reference station comment (hide-with-geometry parity)', () => {
    // Comment on BOTH shots (shot 1 is the reference shot) and on stations g0
    // (touches a non-reference shot) and g2 (all-reference).
    const d = fixture();
    d.shotComments = new Map([[0, 'non-ref note'], [1, 'ref note']]);
    d.stationComments = new Map([[0, 'g0 note'], [2, 'g2 note']]);
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);

    const shotComments = descs.filter((x) => x.kind === LABEL_KIND.SHOT_COMMENT);
    const byShot = new Map(shotComments.map((x) => [x.shotGlobal, x]));
    // Reference shot's comment carries REFERENCE so the hide-reference uniform
    // collapses it with its geometry; the non-reference shot's does not.
    expect(byShot.get(1)!.flags & LABEL_FLAG.REFERENCE).toBe(LABEL_FLAG.REFERENCE);
    expect(byShot.get(0)!.flags & LABEL_FLAG.REFERENCE).toBe(0);

    const stationComments = descs.filter((x) => x.kind === LABEL_KIND.STATION_COMMENT);
    const byStation = new Map(stationComments.map((x) => [x.stationGlobal, x]));
    // All-reference station's comment carries ALL_REFERENCE; the mixed station's
    // does not.
    expect(byStation.get(2)!.flags & LABEL_FLAG.ALL_REFERENCE).toBe(LABEL_FLAG.ALL_REFERENCE);
    expect(byStation.get(0)!.flags & LABEL_FLAG.ALL_REFERENCE).toBe(0);
  });

  it('colors comments with labelColor and section names with the section color', () => {
    const d = fixture();
    const descs = buildLabelDescriptors(d, null, 0, 0, CTX);
    const comment = descs.find((x) => x.kind === LABEL_KIND.SHOT_COMMENT)!;
    expect(comment.color).toEqual(packCssColor('#e0e0e0'));
    const section = descs.find((x) => x.kind === LABEL_KIND.SECTION_NAME)!;
    expect(section.color).toEqual(packCssColor('#4ecdc4'));
    expect(section.text).toBe('Alpha');
  });

  it('viewport filter (§4): drops anchors outside the padded rect, keeps ordinals global', () => {
    const d = fixture();
    // Stations sit at rel-mercator x ascending with station meters (0/10/20 m on
    // x). A rect that admits only the left half of the survey should keep fewer
    // labels than the unfiltered build, but shot ordinals stay 0,1 (global).
    const all = buildLabelDescriptors(d, null, 0, 0, CTX);
    const allShots = all.filter((x) => x.kind === LABEL_KIND.SHOT_LABEL);
    const maxX = Math.max(...allShots.map((s) => s.anchorX));
    const minX = Math.min(...allShots.map((s) => s.anchorX));
    // A rect covering everything is a no-op (same count).
    const full = buildLabelDescriptors(d, null, 0, 0, CTX, undefined, {
      minX: minX - 1,
      maxX: maxX + 1,
      minY: -1,
      maxY: 1,
    });
    expect(full.filter((x) => x.kind === LABEL_KIND.SHOT_LABEL)).toHaveLength(allShots.length);
    // A rect that only admits x <= the first shot's anchor drops the far shot but
    // the surviving shot keeps its GLOBAL ordinal (the loop still counts all).
    const clipped = buildLabelDescriptors(d, null, 0, 0, CTX, undefined, {
      minX: minX - 1e-9,
      maxX: minX + 1e-9,
      minY: -1,
      maxY: 1,
    });
    const clippedShots = clipped.filter((x) => x.kind === LABEL_KIND.SHOT_LABEL);
    expect(clippedShots.length).toBeLessThan(allShots.length);
    // Every surviving ordinal is one of the original global ordinals (0 or 1).
    for (const s of clippedShots) expect([0, 1]).toContain(s.ordinal);
  });

  it('routes comment colors through the injected resolver with owner keys (move_comment tint parity)', () => {
    const d = fixture();
    const keys: string[] = [];
    // The controller passes exactly this resolver when move_comment is active.
    const ctx: LabelBuildContext = {
      ...CTX,
      commentColor: (key: string) => {
        keys.push(key);
        return packCssColor(commentColorForKey(key));
      },
    };
    const descs = buildLabelDescriptors(d, null, 0, 0, ctx);
    // Shot comment on shot id 0 in section 1; station comment on station index 2.
    expect(keys.sort()).toEqual(['1:s0', '1:st2']);
    const shotComment = descs.find((x) => x.kind === LABEL_KIND.SHOT_COMMENT)!;
    expect(shotComment.color).toEqual(packCssColor(commentColorForKey('1:s0')));
    const stationComment = descs.find((x) => x.kind === LABEL_KIND.STATION_COMMENT)!;
    expect(stationComment.color).toEqual(packCssColor(commentColorForKey('1:st2')));
    // The palette tint is distinct from the flat labelColor (real parity, not a no-op).
    expect(shotComment.color).not.toEqual(packCssColor('#e0e0e0'));
  });
});

describe('buildLabelBuffers output shape', () => {
  it('emits 4 verts + 6 indices per glyph with the total glyph count', () => {
    const atlas = fakeAtlas();
    // Two single-glyph labels of different kinds.
    const descs: LabelDescriptor[] = [
      { kind: LABEL_KIND.STATION_LABEL, text: 'A', anchorX: 0.1, anchorY: 0.2, color: [10, 20, 30, 255], ordinal: 5, flags: LABEL_FLAG.FIRST_IN_SECTION },
      { kind: LABEL_KIND.SHOT_LABEL, text: 'BC', anchorX: 0.3, anchorY: 0.4, color: [40, 50, 60, 255], ordinal: 9, flags: 0 },
    ];
    const b = buildLabelBuffers(descs, atlas);
    expect(b.glyphCount).toBe(3); // 1 + 2
    expect(b.vertexCount).toBe(12);
    expect(b.indices).toHaveLength(18);
    expect(b.anchor).toHaveLength(24);
    expect(b.uv).toHaveLength(24);
    expect(b.color).toHaveLength(48);
    expect(b.kind).toHaveLength(12);
    expect(b.revealZoom).toHaveLength(12);
    expect(b.flags).toHaveLength(12);
    expect(b.byteLength).toBeGreaterThan(0);
  });

  it('shares anchor/color/kind/revealZoom/flags across a label glyphs; indices are per-glyph quads', () => {
    const atlas = fakeAtlas();
    const descs: LabelDescriptor[] = [
      { kind: LABEL_KIND.STATION_LABEL, text: 'A', anchorX: 0.1, anchorY: 0.2, color: [10, 20, 30, 200], ordinal: 5, flags: LABEL_FLAG.ALL_REFERENCE },
    ];
    const b = buildLabelBuffers(descs, atlas);
    // All 4 verts of the single glyph share the label attributes. buildLabelBuffers
    // takes no reveal resolver, so the reveal zoom defaults to 0 (visible from any
    // zoom - the whole-survey / no-projector fallback).
    for (let v = 0; v < 4; v++) {
      expect(b.anchor[v * 2]).toBeCloseTo(0.1, 6);
      expect(b.anchor[v * 2 + 1]).toBeCloseTo(0.2, 6);
      expect(Array.from(b.color.slice(v * 4, v * 4 + 4))).toEqual([10, 20, 30, 200]);
      expect(b.kind[v]).toBe(LABEL_KIND.STATION_LABEL);
      expect(b.revealZoom[v]).toBe(0);
      expect(b.flags[v]).toBe(LABEL_FLAG.ALL_REFERENCE);
    }
    // Two triangles, winding 0,1,2, 2,1,3 over the glyph's 4 verts.
    expect(Array.from(b.indices)).toEqual([0, 1, 2, 2, 1, 3]);
  });

  it('uv corners span the glyph atlas pixel rect (top-left..bottom-right)', () => {
    const atlas = fakeAtlas();
    const g = atlas.getGlyph('A'); // pack it so we know its atlas rect
    const descs: LabelDescriptor[] = [
      { kind: LABEL_KIND.STATION_LABEL, text: 'A', anchorX: 0, anchorY: 0, color: [0, 0, 0, 255], ordinal: 0, flags: 0 },
    ];
    const b = buildLabelBuffers(descs, atlas);
    // v0 top-left, v1 top-right, v2 bottom-left, v3 bottom-right (uv in atlas px).
    expect([b.uv[0], b.uv[1]]).toEqual([g.ax, g.ay]);
    expect([b.uv[2], b.uv[3]]).toEqual([g.ax + g.aw, g.ay]);
    expect([b.uv[4], b.uv[5]]).toEqual([g.ax, g.ay + g.ah]);
    expect([b.uv[6], b.uv[7]]).toEqual([g.ax + g.aw, g.ay + g.ah]);
  });

  it('an all-empty descriptor set yields an empty buffer', () => {
    const atlas = fakeAtlas();
    const b = buildLabelBuffers([], atlas);
    expect(b.glyphCount).toBe(0);
    expect(b.vertexCount).toBe(0);
    expect(b.indices).toHaveLength(0);
  });
});

describe('per-label reveal zoom emission', () => {
  const descs: LabelDescriptor[] = [
    { kind: LABEL_KIND.SHOT_LABEL, text: 'AB', anchorX: 0, anchorY: 0, color: [0, 0, 0, 255], ordinal: 0, flags: 0 }, // 2 glyphs
    { kind: LABEL_KIND.STATION_LABEL, text: 'C', anchorX: 0, anchorY: 0, color: [0, 0, 0, 255], ordinal: 1, flags: 0 }, // 1 glyph
  ];

  it('buildLabelBuffersFromShaped stamps revealFor(i) across every vertex of label i', () => {
    const atlas = fakeAtlas();
    const shaped = shapeDescriptors(descs, atlas);
    const reveal = [16.5, 18];
    const b = buildLabelBuffersFromShaped(descs, shaped, undefined, (i) => reveal[i]!);
    // Label 0 (2 glyphs → 8 verts) all read 16.5; label 1 (1 glyph → 4 verts) all read 18.
    for (let v = 0; v < 8; v++) expect(b.revealZoom[v]).toBe(16.5);
    for (let v = 8; v < 12; v++) expect(b.revealZoom[v]).toBe(18);
  });

  it('buildLabelRevealZooms produces the SAME per-vertex layout as the full emit (reveal-only refresh parity)', () => {
    const atlas = fakeAtlas();
    const shaped = shapeDescriptors(descs, atlas);
    const reveal = [14, 20];
    const full = buildLabelBuffersFromShaped(descs, shaped, undefined, (i) => reveal[i]!);
    const only = buildLabelRevealZooms(shaped, (i) => reveal[i]!);
    expect(only).toHaveLength(full.revealZoom.length);
    expect(Array.from(only)).toEqual(Array.from(full.revealZoom));
  });
});
