/**
 * Buffer-building tests for the global view's custom-render tessellation
 * (#customRender). Pure typed-array output from the SystemGeometry shape:
 * segment/station counts, per-system color + index, reference / dim / selected
 * flags, and the cheap dim/selected refresh paths.
 */
import { describe, it, expect } from 'vitest';
import type { ProjectMetadata } from '../../../../types/project';
import type { GeometryShot, SurveyGeometry } from '../../../../types/survey';
import type { SystemGeometry } from '../../systemColor';
import { generateSystemColor } from '../../systemColor';
import { packCssColor } from '../../../Map/surveyLayer/customRender/colorPack';
import {
  buildGlobalBuffers,
  extractSystemStations,
  recomputeDimFlags,
  recomputeSelectedFlags,
} from '../globalTessellate';

function shot(
  from: [number, number],
  to: [number, number],
  kind: GeometryShot['kind'] = 'Normal',
): GeometryShot {
  return { from, to, kind, line_id: 0 };
}

function makeSystem(filePath: string, name: string, shots: GeometryShot[]): SystemGeometry {
  const metadata: ProjectMetadata = {
    name,
    location: null,
    file_path: filePath,
    modified_at: '2026-01-01T00:00:00Z',
    line_count: 1,
    total_length: 100,
    station_count: 3,
    gps_center: [0, 0],
    gps_bounds: null,
    is_remote: false,
  };
  const geometry: SurveyGeometry = { shots, origin: [0, 0] };
  return { metadata, geometry };
}

// System A: 2 non-reference shots -> 2 segments, 3 unique stations.
const systemA = makeSystem('a.tml', 'Alpha', [
  shot([0, 0], [1, 0]),
  shot([1, 0], [2, 0]),
]);
// System B: 1 non-reference + 1 reference shot -> 2 segments, 3 stations; the
// far station only touches the reference shot (allShotsReference).
const systemB = makeSystem('b.tml', 'Bravo', [
  shot([10, 10], [11, 10]),
  shot([11, 10], [12, 10], 'Reference'),
]);

const NO_OPTS = { searchTerm: null, selectedIds: new Set<string>() };

describe('extractSystemStations', () => {
  it('dedups stations and flags reference-only ones', () => {
    const stations = extractSystemStations(systemB.geometry.shots);
    expect(stations).toHaveLength(3);
    // Exactly one station (the far endpoint of the reference shot) is all-reference.
    expect(stations.filter(([, , allRef]) => allRef)).toHaveLength(1);
  });
});

describe('buildGlobalBuffers', () => {
  it('counts segments + stations across systems', () => {
    const { line, circle } = buildGlobalBuffers([systemA, systemB], NO_OPTS);
    expect(line.segmentCount).toBe(4); // 2 + 2 shots
    expect(line.vertexCount).toBe(16); // 4 verts / segment
    expect(line.indices.length).toBe(24); // 6 indices / segment
    expect(circle.stationCount).toBe(6); // 3 + 3 stations
    expect(circle.vertexCount).toBe(24);
  });

  it('assigns a dense per-system index baked into every vertex', () => {
    const { line, circle, systemIndex } = buildGlobalBuffers([systemA, systemB], NO_OPTS);
    expect(systemIndex.get('a.tml')).toBe(0);
    expect(systemIndex.get('b.tml')).toBe(1);
    // System A owns segments 0..1 (index 0), System B segments 2..3 (index 1).
    expect(line.system[0]).toBe(0);
    expect(line.system[8]).toBe(1); // vertex 8 = segment 2, first B segment
    // Stations follow the same partition (first 3 A, next 3 B).
    expect(circle.system[0]).toBe(0);
    expect(circle.system[3 * 4]).toBe(1);
  });

  it('packs the stable per-system hsl color into the vertex color bytes', () => {
    const { line } = buildGlobalBuffers([systemA], NO_OPTS);
    const [r, g, b, a] = packCssColor(generateSystemColor('a.tml'));
    expect(Array.from(line.color.slice(0, 4))).toEqual([r, g, b, a]);
  });

  it('flags reference shots and all-reference stations', () => {
    const { line, circle } = buildGlobalBuffers([systemB], NO_OPTS);
    // `reference` carries the ShotKind discriminant, not a bool (#27):
    // 0 Normal / 1 Jump / 2 Reference. Segment 0 is Normal, segment 1 Reference.
    expect(line.reference[0]).toBe(0);
    expect(line.reference[4]).toBe(2);
    // Exactly one station is all-reference (4 verts).
    let allRefVerts = 0;
    for (let v = 0; v < circle.vertexCount; v++) if (circle.allReference[v] === 1) allRefVerts++;
    expect(allRefVerts).toBe(4);
  });

  it('keeps Jump and Reference distinct in the line buffer (#27)', () => {
    // Regression: this collapsed to a single `kind !== 'Normal' ? 1 : 0` bit, so
    // the global shader could not dim a Reference further than a Jump - the
    // distinction was gone before it ever reached the GPU.
    const mixed = makeSystem('c.tml', 'Charlie', [
      shot([10, 10], [11, 10], 'Jump'),
      shot([11, 10], [12, 10], 'Reference'),
      shot([12, 10], [13, 10], 'Normal'),
    ]);
    const { line } = buildGlobalBuffers([mixed], NO_OPTS);
    expect(line.reference[0]).toBe(1); // segment 0 = Jump
    expect(line.reference[4]).toBe(2); // segment 1 = Reference
    expect(line.reference[8]).toBe(0); // segment 2 = Normal
  });

  it('bakes search-dim from the name and selection from the id set', () => {
    const { line, circle } = buildGlobalBuffers([systemA, systemB], {
      searchTerm: 'alph', // matches Alpha only
      selectedIds: new Set(['b.tml']),
    });
    // A matches search -> dim 0; B does not -> dim 1.
    expect(line.dim[0]).toBe(0);
    expect(line.dim[8]).toBe(1);
    // B selected -> selected 1; A not.
    expect(line.selected[0]).toBe(0);
    expect(line.selected[8]).toBe(1);
    expect(circle.selected[3 * 4]).toBe(1);
  });

  it('stores anchor-relative positions with tiny magnitudes', () => {
    const { line, anchorX, anchorY } = buildGlobalBuffers([systemA], NO_OPTS);
    expect(anchorX).toBeGreaterThan(0);
    expect(anchorY).toBeGreaterThan(0);
    // Anchor-relative coords for a cave-scale span stay small (< 0.5 world span).
    for (let i = 0; i < line.posA.length; i++) {
      expect(Math.abs(line.posA[i]!)).toBeLessThan(0.5);
    }
  });

  it('returns empty buffers + world-center anchor for no systems', () => {
    const { line, circle, anchorX, anchorY } = buildGlobalBuffers([], NO_OPTS);
    expect(line.segmentCount).toBe(0);
    expect(circle.stationCount).toBe(0);
    expect(anchorX).toBe(0.5);
    expect(anchorY).toBe(0.5);
  });
});

describe('recompute refresh paths', () => {
  it('recomputeDimFlags rewrites per-vertex dim from a system predicate', () => {
    const { line, circle } = buildGlobalBuffers([systemA, systemB], NO_OPTS);
    // Dim only system index 1 (B).
    recomputeDimFlags(line, circle, (i) => i === 1);
    expect(line.dim[0]).toBe(0);
    expect(line.dim[8]).toBe(1);
    expect(circle.dim[0]).toBe(0);
    expect(circle.dim[3 * 4]).toBe(1);
    // Clearing the predicate zeroes them again.
    recomputeDimFlags(line, circle, () => false);
    expect(line.dim.every((v) => v === 0)).toBe(true);
  });

  it('recomputeSelectedFlags rewrites per-vertex selected from a predicate', () => {
    const { line, circle } = buildGlobalBuffers([systemA, systemB], NO_OPTS);
    recomputeSelectedFlags(line, circle, (i) => i === 0);
    expect(line.selected[0]).toBe(1);
    expect(line.selected[8]).toBe(0);
    expect(circle.selected[0]).toBe(1);
  });
});
