/**
 * Every preview walk lands where the backend will (#576 D15).
 *
 * The import and placement previews draw sections that are not in the project
 * yet, so they walk the shots themselves. Each used to carry its own copy of
 * the walk, and none of them was the backend's: they ran the SLOPE length (a
 * 10 m shot dropping 6 m was drawn 10 m long, not 8 m), the Ariane copy took
 * one reading, and none applied the section's north reference. So a station
 * that should sit on an existing one was drawn metres away from it.
 *
 * Each preview entry point is run over the shared table that the Rust walker
 * asserts too (`stationWalkVectors.json`), on the axes it exposes.
 *
 * `.tsx` so vitest runs it under jsdom - `SectionPreview` imports maplibre-gl at
 * module load, which the node tier cannot execute.
 */
import { describe, it, expect, vi } from 'vitest';

vi.mock('maplibre-gl', () => ({ default: {} }));

import { computeStationsFromDmpShots, computeStationsFromShots } from '../SectionPreview';
import { computeTmlPreviewLayout } from '../../../utils/tmlPreviewLayout';
import {
  expectedStations,
  vectorCases,
  vectorShots,
  type StationWalkVector,
} from '../../../utils/__tests__/stationWalkTable';
import type { DmpShot, TmlFile, TmlSection, TmlShot } from '../../../types/commands';
import type { Shot } from '../../../types/project';

const xyz = (stations: Array<{ x: number; y: number; z: number }>) =>
  stations.map((s) => [s.x, s.y, s.z]);
// The section's own reference, else the project's: `Project::azimuth_reference_for`.
const resolved = (v: StationWalkVector) => v.reference ?? v.project_default ?? null;

describe('SectionPreview: an existing section (computeStationsFromShots)', () => {
  it.each(vectorCases())('%s', (_name, vector) => {
    const shots = vectorShots(vector).map((s, id) => ({ ...s, id }) as unknown as Shot);
    expect(xyz(computeStationsFromShots(shots, resolved(vector)))).toEqual(expectedStations(vector));
  });
});

describe('SectionPreview: a section being imported (computeStationsFromDmpShots)', () => {
  it.each(vectorCases())('%s', (_name, vector) => {
    const shots: DmpShot[] = vectorShots(vector).map((s) => ({
      shot_type: 'Standard',
      heading_in: s.azimuth_in,
      heading_out: s.azimuth_out,
      length: s.length,
      depth_in: s.depth_in,
      depth_out: s.depth_out,
      pitch_in: 0,
      pitch_out: 0,
      left: 0,
      right: 0,
      up: 0,
      down: 0,
      temperature: 0,
      hour: 0,
      minute: 0,
      second: 0,
      marker_index: 0,
    }));
    expect(xyz(computeStationsFromDmpShots(shots, resolved(vector)))).toEqual(
      expectedStations(vector)
    );
  });
});

function tmlShot(id: number, fromId: number, over: Partial<TmlShot>): TmlShot {
  return {
    id,
    from_id: fromId,
    shot_type: 'Real',
    azimuth: 0,
    length: 1,
    depth: 0,
    depth_in: 0,
    inclination: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    comment: null,
    explorer: null,
    date: null,
    color: '#ff0000',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: -1,
    section: '',
    ...over,
  };
}

function tmlSection(name: string, startId: number, shots: TmlShot[], startDepth = 0): TmlSection {
  return {
    name,
    shots,
    surveyor: null,
    date: null,
    color: '#ff0000',
    is_branch_split: false,
    start_ariane_id: startId,
    end_ariane_id: shots[shots.length - 1]?.id ?? -1,
    start_depth: startDepth,
  };
}

function tmlFile(sections: TmlSection[]): TmlFile {
  return {
    cave_name: 'CaveA',
    unit: 'Metric',
    use_magnetic_azimuth: false,
    first_start_absolute_elevation: 0,
    geo_coding: null,
    sections,
    closures: [],
  };
}

/**
 * Where the Ariane preview puts every station of `root`: one child section
 * hangs off each of its stations, and the layout offsets a child by exactly the
 * position of the station it starts from.
 */
function tmlStationPositions(root: TmlShot[], startDepth = 0): number[][] {
  const probes = root.map((_, k) =>
    tmlSection(`Probe${k + 1}`, k + 1, [tmlShot(100 + k, k + 1, { azimuth: 0, length: 1 })])
  );
  const layout = computeTmlPreviewLayout(tmlFile([tmlSection('Root', 0, root, startDepth), ...probes]), {
    includeExcluded: false,
    selected: null,
  });
  return layout.sections.map((s) => [s.offsetX, s.offsetY]);
}

/** Ariane records ONE azimuth per shot and declares north per file, so only
 *  the single-reading, unrotated rows have an Ariane spelling. */
const tmlExpressible = (v: StationWalkVector) =>
  v.project_default === null &&
  (v.reference === null || v.reference.north === 'true') &&
  v.shots.every((s) => s.azimuth_out === null || s.azimuth_out === s.azimuth_in);

describe('Ariane preview layout (computeTmlPreviewLayout)', () => {
  it.each(vectorCases(tmlExpressible))('%s', (_name, vector) => {
    const root = vector.shots.map((s, i) =>
      tmlShot(i + 1, i, { azimuth: s.azimuth_in, length: s.length, depth_in: s.depth_in, depth: s.depth_out })
    );
    expect(tmlStationPositions(root)).toEqual(expectedStations(vector, 2));
  });

  /**
   * Ariane spells "this shot starts where the previous one ended" as a DepthIn
   * of exactly -1; the importer resolves it to the previous surviving shot's
   * depth, or the section's start depth for the first. Taken literally it is a
   * depth change of up to (depth + 1) - enough to fold a shot to nothing.
   */
  it('resolves the "same as previous" depth sentinel the way the importer does', () => {
    const root = [
      tmlShot(1, 0, { azimuth: 90, length: 10, depth_in: -1, depth: 6 }),
      tmlShot(2, 1, { azimuth: 90, length: 10, depth_in: -1, depth: 6 }),
    ];
    const positions = tmlStationPositions(root).map(([x]) => x);
    // Shot 1 starts at the section's start depth (0): 10 m dropping 6 m is 8 m.
    // Shot 2 starts at shot 1's 6 m and stays there: the full 10 m.
    expect(positions[1]).toBeCloseTo(8, 9);
    expect(positions[2]).toBeCloseTo(18, 9);
  });

  it('draws each section with the depths its offsets were walked with', () => {
    const file = tmlFile([
      tmlSection('Root', 0, [
        tmlShot(1, 0, { azimuth: 90, length: 10, depth_in: 2, depth: 6 }),
        tmlShot(2, 1, { azimuth: 90, length: 10, depth_in: -1, depth: 6 }),
      ]),
    ]);
    const layout = computeTmlPreviewLayout(file, { includeExcluded: false, selected: null });
    const drawn = layout.sections[0]!.section.shots;
    expect(drawn.map((s) => [s.depth_in, s.depth_out])).toEqual([
      [2, 6],
      [6, 6],
    ]);
  });
});
