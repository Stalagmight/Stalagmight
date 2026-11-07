/**
 * Hit-test equivalence tests (#mem).
 *
 * The merged-feature sources can no longer resolve per-item hits from feature
 * properties, so `SurveyGeometryIndex` re-derives them from the SoA
 * `SurveyData`. These tests prove the new resolver picks the SAME station /
 * shot as the old per-feature path (`resolveNearestStation` /
 * `resolveNearestShot` over per-feature build output) for a grid of probe
 * points, with the same tolerances.
 */
import { describe, expect, it, vi } from 'vitest';
import type { Map as MaplibreMap, MapMouseEvent, MapGeoJSONFeature } from 'maplibre-gl';
import { resolveNearestShot, resolveNearestStation } from '../hitTest';
import { SurveyGeometryIndex } from '../geometryIndex';
import { padProbeBox } from '../hitProbe';
import { surveyDataFromLegacy } from '../../../../types/survey-data';
import { metersToMapCoords } from '../../../../utils/coordinates';
import { encodeFeatureId } from '../../../../utils/ids';
import type { Section } from '../../../../types/ipc/Section';
import type { Station } from '../../../../types/ipc/Station';
import { makeStation } from '../../../../test/factories/survey';

/** #27: both virtual kinds drawn (the default) / neither drawn. */
const ALL_KINDS = { jump: true, reference: true } as const;
const NO_VIRTUAL = { jump: false, reference: false } as const;

/** Deterministic screen projection for tests: 1 degree = 100000 px, y down. */
const SCALE = 100000;
const project = (lngLat: [number, number]) => ({
  x: lngLat[0] * SCALE,
  y: -lngLat[1] * SCALE,
});
const fakeMap = { project: (c: [number, number]) => project(c) } as unknown as MaplibreMap;
const eventAt = (x: number, y: number) => ({ point: { x, y } }) as MapMouseEvent;

function fixture(): { sections: Section[]; stations: Station[] } {
  const shot = (id: number, isVirtual = false) => ({
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 10,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: isVirtual ? ('Reference' as const) : ('Normal' as const),
    marker_index: null,
    temperature: null,
  });
  // Two sections crossing near the origin plus a reference spur, so probe
  // points have competing candidates from different sections.
  const sections: Section[] = [
    {
      id: 1, name: 'A', surveyor: null, date: null, color: '#ff0000',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(101), shot(102), shot(103, true)],
    },
    {
      id: 2, name: 'B', surveyor: null, date: null, color: '#00ff00',
      direction: 'in', quality: 1, next_shot_id: 10, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [shot(201), shot(202)],
    },
  ];
  const st = (section_id: number, station_index: number, x: number, y: number): Station =>
    makeStation({ station_ref: { section_id, station_index }, coordinates: { x, y, z: 0 }, closures_with: [] });
  const stations: Station[] = [
    st(1, 0, 0, 0),
    st(1, 1, 30, 0),
    st(1, 2, 30, 25),
    st(1, 3, 60, 25), // reference-shot endpoint
    st(2, 0, 5, 20),
    st(2, 1, 25, -10),
    st(2, 2, 55, -10),
  ];
  return { sections, stations };
}

/**
 * Build the per-feature station Points + shot LineStrings the equivalence oracle
 * compares against, directly from the SoA. This mirrors the (deleted) stock
 * non-merged feature builder for exactly the fields these tests read: geometry
 * (via the same `metersToMapCoords` transform `SurveyGeometryIndex` uses) plus
 * `sectionId` / `stationIndex` / `shotId`, in the same emit order (stations in
 * global-index order; shots in section -> shot-offset order; reference shots
 * included). It is an INDEPENDENT oracle - it does not call the index under test.
 */
function buildRefFeatures(data: ReturnType<typeof surveyDataFromLegacy>): {
  stationFeatures: GeoJSON.Feature[];
  shotFeatures: GeoJSON.Feature[];
} {
  // (sectionId, stationIndex) -> global station index.
  const stationLookup = new Map<number, Map<number, number>>();
  for (let g = 0; g < data.stationCount; g++) {
    const sid = data.stationSectionIds[g]!;
    const stIdx = data.stationIndices[g]!;
    let inner = stationLookup.get(sid);
    if (!inner) {
      inner = new Map();
      stationLookup.set(sid, inner);
    }
    inner.set(stIdx, g);
  }
  const stationFeatures: GeoJSON.Feature[] = [];
  for (let g = 0; g < data.stationCount; g++) {
    const base = g * 3;
    stationFeatures.push({
      type: 'Feature',
      geometry: {
        type: 'Point',
        coordinates: metersToMapCoords(data.stationCoords[base]!, data.stationCoords[base + 1]!, null),
      },
      properties: { sectionId: data.stationSectionIds[g], stationIndex: data.stationIndices[g] },
    });
  }
  const shotFeatures: GeoJSON.Feature[] = [];
  for (const section of data.sections) {
    const inner = stationLookup.get(section.id);
    if (!inner) continue;
    for (let i = 0; i < section.shotCount; i++) {
      const fromGlobal = inner.get(i);
      const toGlobal = inner.get(i + 1);
      if (fromGlobal === undefined || toGlobal === undefined) continue;
      const shotId = data.shotIds[section.shotOffset + i]!;
      const f = fromGlobal * 3;
      const t = toGlobal * 3;
      shotFeatures.push({
        type: 'Feature',
        geometry: {
          type: 'LineString',
          coordinates: [
            metersToMapCoords(data.stationCoords[f]!, data.stationCoords[f + 1]!, null),
            metersToMapCoords(data.stationCoords[t]!, data.stationCoords[t + 1]!, null),
          ],
        },
        properties: { sectionId: section.id, shotId },
      });
    }
  }
  return { stationFeatures, shotFeatures };
}

function buildPerFeature() {
  const { sections, stations } = fixture();
  const data = surveyDataFromLegacy(sections, stations);
  return { data, sets: buildRefFeatures(data) };
}

/** Wrap a built GeoJSON feature as the minimal MapGeoJSONFeature shape the
 *  old resolvers consumed. */
function asRendered(f: GeoJSON.Feature): MapGeoJSONFeature {
  return { geometry: f.geometry, properties: f.properties } as unknown as MapGeoJSONFeature;
}

/** Probe grid covering the fixture extent (in px). */
function probeGrid(): Array<{ x: number; y: number }> {
  const points: Array<{ x: number; y: number }> = [];
  // Fixture spans roughly x 0..60 m, y -10..25 m; meters/deg = 111320 so px
  // ~ m * (SCALE / 111320) ~ m * 0.898. Probe a dense grid over that region.
  for (let mx = -5; mx <= 65; mx += 2.5) {
    for (let my = -15; my <= 30; my += 2.5) {
      points.push({ x: (mx / 111320) * SCALE, y: -(my / 111320) * SCALE });
    }
  }
  return points;
}

describe('SurveyGeometryIndex equivalence with per-feature resolvers', () => {
  const { data, sets } = buildPerFeature();
  const index = new SurveyGeometryIndex(data, null);
  const allSectionIds = [1, 2];
  const stationHitArea = 10;
  const shotHitArea = 8;

  const stationFeatures = sets.stationFeatures.map(asRendered);
  const shotFeatures = sets.shotFeatures.map(asRendered);

  it('nearest station matches the old resolver across the probe grid', () => {
    let hits = 0;
    for (const p of probeGrid()) {
      // Old path: candidates = features whose rendered circle contains the
      // cursor (radius = hit area), i.e. distance <= stationHitArea.
      const oldCandidates = stationFeatures.filter((f) => {
        const c = (f.geometry as GeoJSON.Point).coordinates as [number, number];
        const s = project(c);
        return Math.hypot(s.x - p.x, s.y - p.y) <= stationHitArea;
      });
      const oldPick = resolveNearestStation(fakeMap, eventAt(p.x, p.y), oldCandidates, stationHitArea);
      const newPick = index.nearestStation(
        (c) => project(c), p.x, p.y, allSectionIds, stationHitArea, { visibleKinds: { jump: true, reference: true } },
      );
      if (oldPick) {
        hits++;
        expect(newPick).not.toBeNull();
        expect(newPick!.ref.section_id).toBe(oldPick.properties!.sectionId);
        expect(newPick!.ref.station_index).toBe(oldPick.properties!.stationIndex);
      } else {
        expect(newPick).toBeNull();
      }
    }
    expect(hits).toBeGreaterThan(0); // the grid actually exercised hits
  });

  it('nearest shot matches the old resolver across the probe grid', () => {
    let hits = 0;
    for (const p of probeGrid()) {
      // Old path: candidates = features within half the rendered line width
      // of the cursor (MapLibre's line hit test), then nearest-of-candidates.
      const oldCandidates = shotFeatures.filter((f) => {
        const coords = (f.geometry as GeoJSON.LineString).coordinates as [number, number][];
        const a = project(coords[0]!);
        const b = project(coords[1]!);
        const t = Math.max(0, Math.min(1,
          ((p.x - a.x) * (b.x - a.x) + (p.y - a.y) * (b.y - a.y)) /
          (((b.x - a.x) ** 2 + (b.y - a.y) ** 2) || 1)));
        const qx = a.x + t * (b.x - a.x);
        const qy = a.y + t * (b.y - a.y);
        return Math.hypot(p.x - qx, p.y - qy) <= shotHitArea / 2;
      });
      const oldPick = resolveNearestShot(fakeMap, eventAt(p.x, p.y), oldCandidates, shotHitArea);
      const newPick = index.nearestShot(
        (c) => project(c), p.x, p.y, allSectionIds, shotHitArea, { visibleKinds: { jump: true, reference: true } },
      );
      if (oldPick) {
        hits++;
        expect(newPick).not.toBeNull();
        expect(newPick!.sectionId).toBe(oldPick.properties!.sectionId);
        expect(newPick!.shotId).toBe(oldPick.properties!.shotId);
      } else if (newPick) {
        // The rendered gate (half line width) is narrower than the resolver
        // ceiling (full hit area): the resolver may hit where the old query
        // returned nothing only OUTSIDE the rendered gate.
        expect(newPick.distance).toBeGreaterThan(shotHitArea / 2);
      }
    }
    expect(hits).toBeGreaterThan(0);
  });

  it('reference filtering hides reference shots and all-reference stations', () => {
    // Probe directly on the reference shot (section 1, shot 103: (30,25)-(60,25)).
    const px = (45 / 111320) * SCALE;
    const py = -(25 / 111320) * SCALE;
    const withRef = index.nearestShot((c) => project(c), px, py, allSectionIds, 8, { visibleKinds: { jump: true, reference: true } });
    expect(withRef?.shotId).toBe(103);
    const withoutRef = index.nearestShot((c) => project(c), px, py, allSectionIds, 8, { visibleKinds: { jump: false, reference: false } });
    expect(withoutRef?.shotId).not.toBe(103);
    // Station (1,3) only touches the reference shot -> hidden when refs are.
    const stPx = (60 / 111320) * SCALE;
    const stPy = -(25 / 111320) * SCALE;
    const stWith = index.nearestStation((c) => project(c), stPx, stPy, allSectionIds, 10, { visibleKinds: { jump: true, reference: true } });
    expect(stWith?.ref).toEqual({ section_id: 1, station_index: 3 });
    const stWithout = index.nearestStation((c) => project(c), stPx, stPy, allSectionIds, 10, { visibleKinds: { jump: false, reference: false } });
    expect(stWithout?.ref).not.toEqual({ section_id: 1, station_index: 3 });
  });

  it('a reference-only station goes non-interactive when only References are hidden (#27)', () => {
    // The regression: station (1,3) touches nothing but the reference shot, but
    // the gate asked "is ANY virtual kind visible anywhere?" rather than "is any
    // kind touching THIS station visible?". With Jumps on and References off the
    // answer was yes, so an orphan marker stayed hit-testable over geometry the
    // line shader had already collapsed.
    const stPx = (60 / 111320) * SCALE;
    const stPy = -(25 / 111320) * SCALE;
    const at = (visibleKinds: { jump: boolean; reference: boolean }) =>
      index.nearestStation((c) => project(c), stPx, stPy, allSectionIds, 10, { visibleKinds });

    expect(at({ jump: true, reference: true })?.ref).toEqual({ section_id: 1, station_index: 3 });
    // Jumps still on - irrelevant to a station no jump touches.
    expect(at({ jump: true, reference: false })?.ref).not.toEqual({
      section_id: 1,
      station_index: 3,
    });
    // And hiding jumps alone must NOT hide it: its own leg is still drawn.
    expect(at({ jump: false, reference: true })?.ref).toEqual({
      section_id: 1,
      station_index: 3,
    });
  });

  it('t parameter matches projectParamOnSegment on the true endpoints', () => {
    // Probe near the midpoint of shot 101 ((0,0)-(30,0)).
    const px = (12 / 111320) * SCALE;
    const py = -(1 / 111320) * SCALE;
    const hit = index.nearestShot((c) => project(c), px, py, [1], 8, { visibleKinds: { jump: true, reference: true } })!;
    expect(hit.shotId).toBe(101);
    expect(hit.t).toBeCloseTo(12 / 30, 3);
    expect(hit.from[0]).toBeCloseTo(0, 9);
    expect(hit.to[0]).toBeCloseTo(30 / 111320, 9);
  });

  it('shotGeometry / stationCoords resolve overlay geometry', () => {
    const g = index.shotGeometry(1, 102)!;
    expect(g.from[0]).toBeCloseTo(30 / 111320, 9);
    expect(g.to[1]).toBeCloseTo(25 / 111320, 9);
    expect(g.isReference).toBe(false);
    const ref = index.stationCoords({ section_id: 2, station_index: 2 })!;
    expect(ref[0]).toBeCloseTo(55 / 111320, 9);
    expect(index.stationCoords({ section_id: 9, station_index: 0 })).toBeNull();
    expect(index.isZeroDatum({ section_id: 1, station_index: 0 })).toBe(false);
  });
});

/**
 * Per-section geometry, used by the drag-preview capture. It must agree with the
 * per-item accessors and expose each shot's TRUE section position (`index`), since
 * the capture maps shot i to stations i / i+1. Reading this from the SoA replaces
 * filtering the lazy `Station[]` proxy per section, which was quadratic and froze
 * segment drags on a 14k-section project.
 */
describe('SurveyGeometryIndex.sectionGeometry', () => {
  const { data } = buildPerFeature();
  const index = new SurveyGeometryIndex(data, null);

  it('returns a section\'s stations in station_index order with matching coords', () => {
    const { stations } = index.sectionGeometry(1);
    expect(stations.map((s) => s.stationIndex)).toEqual([0, 1, 2, 3]);
    for (const s of stations) {
      expect(s.lngLat).toEqual(
        index.stationCoords({ section_id: 1, station_index: s.stationIndex })!,
      );
    }
  });

  it('returns shots with their section position and endpoints matching shotGeometry', () => {
    const { shots } = index.sectionGeometry(1);
    expect(shots.map((s) => s.index)).toEqual([0, 1, 2]);
    for (const sh of shots) {
      const geo = index.shotGeometry(1, sh.shotId)!;
      expect(sh.from).toEqual(geo.from);
      expect(sh.to).toEqual(geo.to);
      expect(sh.globalIdx).toBe(geo.globalIdx);
      expect(sh.isReference).toBe(geo.isReference);
    }
    // Shot `index` joins station index -> index+1, which the drag capture relies on.
    for (const sh of shots) {
      expect(sh.from).toEqual(index.stationCoords({ section_id: 1, station_index: sh.index })!);
      expect(sh.to).toEqual(index.stationCoords({ section_id: 1, station_index: sh.index + 1 })!);
    }
  });

  it('includes reference shots (the drag capture moves them too)', () => {
    // Fixture shot 103 in section 1 is a reference shot.
    const { shots } = index.sectionGeometry(1);
    const ref = shots.find((s) => s.shotId === 103);
    expect(ref?.isReference).toBe(true);
  });

  it('returns empty for an unknown section', () => {
    expect(index.sectionGeometry(999)).toEqual({ stations: [], shots: [] });
  });
});

/**
 * Bulk geometry enumeration. With the custom renderer there are no stock
 * `survey-stations` / `survey-shots` layers, so E2E tests project these coords
 * instead of calling `queryRenderedFeatures`. Must agree with the per-item
 * accessors (`stationCoords` / `shotGeometry`) and honor reference visibility.
 */
describe('SurveyGeometryIndex.enumerateGeometry', () => {
  const { data } = buildPerFeature();
  const index = new SurveyGeometryIndex(data, null);

  it('enumerates every station and shot with identities matching the per-item accessors', () => {
    const { stations, shots } = index.enumerateGeometry(ALL_KINDS);
    // Fixture: section 1 has 3 shots (4 stations), section 2 has 2 shots (3 stations).
    expect(stations).toHaveLength(7);
    expect(shots).toHaveLength(5);
    for (const s of stations) {
      const viaAccessor = index.stationCoords({
        section_id: s.sectionId,
        station_index: s.stationIndex,
      })!;
      expect(s.lngLat).toEqual(viaAccessor);
    }
    for (const sh of shots) {
      const geo = index.shotGeometry(sh.sectionId, sh.shotId)!;
      expect(sh.from).toEqual(geo.from);
      expect(sh.to).toEqual(geo.to);
    }
  });

  it('omits reference shots and all-reference stations when reference is hidden', () => {
    const all = index.enumerateGeometry(ALL_KINDS);
    const visible = index.enumerateGeometry(NO_VIRTUAL);
    // Fixture shot 103 is a reference shot; it drops out, as does the station
    // that only that shot touches.
    expect(visible.shots.length).toBeLessThan(all.shots.length);
    expect(visible.shots.some((s) => s.shotId === 103)).toBe(false);
    expect(all.shots.some((s) => s.shotId === 103)).toBe(true);
    expect(visible.stations.length).toBeLessThan(all.stations.length);
  });
});

/**
 * Measured-path shot resolver (#26). The measured along-line tint feeds the
 * custom measure-path layer through `measuredShotSegments`, which must resolve
 * a shot from its canonical feature id WITHOUT decoding it. Shot ids are
 * globally encoded (`sectionId * FEATURE_ID_STRIDE + local`), so a
 * `floor(fid / STRIDE)` split (the old code) mis-attributes every shot in
 * section >= 1 and drops its tint (stations, whose scoped id is a small local
 * index, decoded fine - the reported "stations tint, shots don't" bug). This
 * fixture reproduces the production id scheme so the regression is pinned.
 */
describe('SurveyGeometryIndex.measuredShotSegments (global shot ids)', () => {
  const mkShot = (id: number, isVirtual = false): Section['shots'][number] => ({
    id, azimuth_in: 0, azimuth_out: 0, length: 10, depth_in: 0, depth_out: 0,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: isVirtual ? ('Reference' as const) : ('Normal' as const), marker_index: null, temperature: null,
  });
  // Section 0 shot ids are local-sized (< STRIDE); section 1 shot ids are the
  // production global form (100000, 100001), which is where the old decode broke.
  const sections: Section[] = [
    {
      id: 0, name: 'A', surveyor: null, date: null, color: '#f00',
      direction: 'in', quality: 1, next_shot_id: 2, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null, shots: [mkShot(0), mkShot(1)],
    },
    {
      id: 1, name: 'B', surveyor: null, date: null, color: '#0f0',
      direction: 'in', quality: 1, next_shot_id: 100002, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [mkShot(100000), mkShot(100001, true)],
    },
  ];
  const st = (section_id: number, station_index: number, x: number, y: number): Station =>
    makeStation({ station_ref: { section_id, station_index }, coordinates: { x, y, z: 0 }, closures_with: [] });
  const stations: Station[] = [
    st(0, 0, 0, 0), st(0, 1, 10, 0), st(0, 2, 10, 10),
    st(1, 0, 100, 0), st(1, 1, 110, 0), st(1, 2, 110, 10),
  ];
  const index = new SurveyGeometryIndex(surveyDataFromLegacy(sections, stations), null);
  const mc = (m: number) => m / 111320;

  it('resolves a section >= 1 shot by its globally-encoded feature id', () => {
    // encodeFeatureId(1, 100000) = 200000; the old floor(200000/STRIDE)=2 split
    // looked up a nonexistent section 2 and returned nothing.
    const segs = index.measuredShotSegments(new Set([encodeFeatureId(1, 100000)]), ALL_KINDS);
    expect(segs).toHaveLength(1);
    expect(segs[0]!.from[0]).toBeCloseTo(mc(100), 9);
    expect(segs[0]!.to[0]).toBeCloseTo(mc(110), 9);
  });

  it('resolves shots across sections in one pass', () => {
    const segs = index.measuredShotSegments(
      new Set([encodeFeatureId(0, 0), encodeFeatureId(1, 100000)]),
      ALL_KINDS,
    );
    expect(segs).toHaveLength(2);
  });

  it('drops a hidden reference leg unless reference shots are shown', () => {
    const fid = new Set([encodeFeatureId(1, 100001)]);
    expect(index.measuredShotSegments(fid, NO_VIRTUAL)).toHaveLength(0);
    expect(index.measuredShotSegments(fid, ALL_KINDS)).toHaveLength(1);
  });

  it('returns nothing for an empty id set', () => {
    expect(index.measuredShotSegments(new Set(), ALL_KINDS)).toHaveLength(0);
  });
});

/**
 * Candidate-section gate (#customRender): `sectionsIntersectingBox` replaces the
 * old `queryRenderedFeatures` prefilter, so candidate discovery no longer needs
 * the stock layers to be rendered. The gate must return a SUPERSET of the
 * sections the per-item SoA resolvers then pick - never dropping an in-tolerance
 * hit - and honor the padded-box reach semantics (`padProbeBox`).
 */
describe('SurveyGeometryIndex.sectionsIntersectingBox candidate gate', () => {
  const { data } = buildPerFeature();
  const index = new SurveyGeometryIndex(data, null);
  const proj = (c: [number, number]) => project(c);
  const stationReach = 10;
  const shotReach = 4;
  /** Fixture meters -> the deterministic test screen space. */
  const screen = (mx: number, my: number) => ({
    x: (mx / 111320) * SCALE,
    y: -(my / 111320) * SCALE,
  });
  const sectionsAt = (box: ReturnType<typeof padProbeBox>) =>
    index.sectionsIntersectingBox(proj, box[0][0], box[0][1], box[1][0], box[1][1]);

  it('returns empty for a probe far from every section', () => {
    const p = screen(500, 500);
    expect(sectionsAt(padProbeBox([p.x, p.y], stationReach)).size).toBe(0);
  });

  it('includes a section whose geometry sits under the probe box', () => {
    // Station (1,3) at (60,25) m: section 1 owns the geometry near there.
    const p = screen(60, 25);
    expect(sectionsAt(padProbeBox([p.x, p.y], stationReach)).has(1)).toBe(true);
  });

  it('is a SUPERSET of the sections the SoA resolver picks (station + shot)', () => {
    for (const p of probeGrid()) {
      const st = index.nearestStation(proj, p.x, p.y, [1, 2], stationReach, {
        visibleKinds: { jump: true, reference: true },
      });
      if (st) {
        expect(sectionsAt(padProbeBox([p.x, p.y], stationReach)).has(st.ref.section_id)).toBe(true);
      }
      const sh = index.nearestShot(proj, p.x, p.y, [1, 2], shotReach, { visibleKinds: { jump: true, reference: true } });
      if (sh) {
        expect(sectionsAt(padProbeBox([p.x, p.y], shotReach)).has(sh.sectionId)).toBe(true);
      }
    }
  });

  it('reach padding captures a section just beyond its bounds (missed unpadded)', () => {
    // Probe 8 px to the RIGHT of station (1,3) at (60,25) m - section 1's max x.
    // The station is within the 10 px reach, but an UNPADDED point box lands past
    // the section's AABB and misses it; padding by the reach restores it.
    const at = screen(60, 25);
    const p = { x: at.x + 8, y: at.y };
    expect(sectionsAt(padProbeBox([p.x, p.y], 0)).has(1)).toBe(false);
    const cand = sectionsAt(padProbeBox([p.x, p.y], stationReach));
    expect(cand.has(1)).toBe(true);
    // The resolver, fed that candidate set, confirms the real in-reach hit.
    const hit = index.nearestStation(proj, p.x, p.y, cand, stationReach, { visibleKinds: { jump: true, reference: true } });
    expect(hit?.ref).toEqual({ section_id: 1, station_index: 3 });
  });
});

/**
 * Camera-keyed projected-AABB cache (#customRender). The gate now reuses each
 * section's projected screen AABB across probes of the SAME camera. The cached
 * path (and its axis-aligned 2-corner fast path) must return EXACTLY the sets
 * the legacy per-call 4-corner reprojection did, and must reproject only when
 * the caller's camera key changes.
 */
describe('SurveyGeometryIndex.sectionsIntersectingBox camera cache', () => {
  const { data } = buildPerFeature();
  const proj = (c: [number, number]) => project(c);
  const stationReach = 10;
  const boxes = probeGrid().map((p) => padProbeBox([p.x, p.y], stationReach));
  const runAt = (
    idx: SurveyGeometryIndex,
    box: ReturnType<typeof padProbeBox>,
    cam?: { key: string; axisAligned: boolean },
  ) => idx.sectionsIntersectingBox(proj, box[0][0], box[0][1], box[1][0], box[1][1], cam);

  it('cached (2-corner + 4-corner) matches the uncached path across the probe grid', () => {
    const naive = new SurveyGeometryIndex(data, null);
    const cached2 = new SurveyGeometryIndex(data, null);
    const cached4 = new SurveyGeometryIndex(data, null);
    for (const box of boxes) {
      const want = [...runAt(naive, box)].sort();
      expect([...runAt(cached2, box, { key: 'cam', axisAligned: true })].sort()).toEqual(want);
      expect([...runAt(cached4, box, { key: 'cam', axisAligned: false })].sort()).toEqual(want);
    }
  });

  it('reprojects only when the camera key changes', () => {
    const index = new SurveyGeometryIndex(data, null);
    const spy = vi.fn(proj);
    const box = boxes[0]!;
    // First probe with key A -> projects (2 sections * 2 corners under axisAligned).
    index.sectionsIntersectingBox(spy, box[0][0], box[0][1], box[1][0], box[1][1], { key: 'A', axisAligned: true });
    const afterFirst = spy.mock.calls.length;
    expect(afterFirst).toBe(4);
    // Same key -> cache hit, no further project() calls even for a different box.
    const box2 = boxes[boxes.length - 1]!;
    index.sectionsIntersectingBox(spy, box2[0][0], box2[0][1], box2[1][0], box2[1][1], { key: 'A', axisAligned: true });
    expect(spy.mock.calls.length).toBe(afterFirst);
    // New key -> reproject once more.
    index.sectionsIntersectingBox(spy, box[0][0], box[0][1], box[1][0], box[1][1], { key: 'B', axisAligned: true });
    expect(spy.mock.calls.length).toBe(afterFirst * 2);
  });

  it('an omitted snapshot reprojects every call (legacy behavior preserved)', () => {
    const index = new SurveyGeometryIndex(data, null);
    const spy = vi.fn(proj);
    const box = boxes[0]!;
    // No camera -> 4-corner reprojection each call (2 sections * 4 corners = 8).
    index.sectionsIntersectingBox(spy, box[0][0], box[0][1], box[1][0], box[1][1]);
    expect(spy.mock.calls.length).toBe(8);
    index.sectionsIntersectingBox(spy, box[0][0], box[0][1], box[1][0], box[1][1]);
    expect(spy.mock.calls.length).toBe(16);
  });
});
