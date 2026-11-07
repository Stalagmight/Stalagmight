import { describe, it, expect } from 'vitest';
import type { Section } from '../../types/ipc/Section';
import type { Station } from '../../types/ipc/Station';
import { surveyDataFromLegacy, lazyStationArray } from '../../types/survey-data';
import { computeImmovableStationKeys } from '../immovableStations';
import { makeStation } from '../../test/factories/survey';

/**
 * Regression coverage for the SoA immovable-station BFS (#customRender, Codex
 * round 5, P2). The move tools previously walked a materialized `Station[]`
 * (rebuilding + RETAINING the whole ~89k-object graph via the lazy proxy);
 * `computeImmovableStationKeys` reads the SoA directly instead. These tests pin
 * the behavior by running the ORIGINAL `Station[]` algorithm as an independent
 * oracle over the same fixtures and asserting the SoA helper matches it exactly.
 */

// The original algorithm, verbatim from MapCanvas before the SoA migration, over
// a materialized Station[] - the behavioral oracle the SoA path must reproduce.
function immovableViaStationArray(
  stations: readonly Station[],
  alignedClosureIds: ReadonlySet<number>,
): Set<string> {
  const stationByKey = new Map(
    stations.map((s) => [`${s.station_ref.section_id}:${s.station_ref.station_index}`, s]),
  );
  const immovable = new Set<string>();
  const queue: string[] = [];
  for (const s of stations) {
    if (s.is_zero_datum) {
      queue.push(`${s.station_ref.section_id}:${s.station_ref.station_index}`);
    }
  }
  while (queue.length > 0) {
    const key = queue.pop()!;
    if (immovable.has(key)) continue;
    immovable.add(key);
    const st = stationByKey.get(key);
    if (!st) continue;
    for (const ref of st.merged_with) {
      queue.push(`${ref.section_id}:${ref.station_index}`);
    }
    for (const cr of st.closures_with ?? []) {
      if (!alignedClosureIds.has(cr.closure_id)) continue;
      queue.push(`${cr.station_ref.section_id}:${cr.station_ref.station_index}`);
    }
  }
  return immovable;
}

const st = (
  section_id: number,
  station_index: number,
  opts: Partial<Station> = {},
): Station =>
  makeStation({
    station_ref: { section_id, station_index },
    coordinates: { x: station_index, y: section_id, z: 0 },
    closures_with: [],
    ...opts,
  });

/** One section per station so `next_shot_id`/shots don't matter; the BFS only
 *  reads station topology. Each station needs a section shell to exist in SoA. */
function fixture(stations: Station[]): { data: ReturnType<typeof surveyDataFromLegacy> } {
  const sectionIds = [...new Set(stations.map((s) => s.station_ref.section_id))];
  const sections: Section[] = sectionIds.map((id) => ({
    id,
    name: `S${id}`,
    surveyor: null,
    date: null,
    color: '#fff',
    direction: 'in',
    quality: 1,
    next_shot_id: 0,
    reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
    shots: [],
  }));
  return { data: surveyDataFromLegacy(sections, stations) };
}

describe('computeImmovableStationKeys', () => {
  it('propagates from a zero-datum through merged_with and aligned closures, but not unaligned ones', () => {
    // 0:0 is the datum. merged 0:0 <-> 1:0. 1:0 aligned-closure to 2:0.
    // 3:0 links to 0:0 only through an UNALIGNED closure -> must stay movable.
    const stations = [
      st(0, 0, { is_zero_datum: true, merged_with: [{ section_id: 1, station_index: 0 }] }),
      st(1, 0, {
        merged_with: [{ section_id: 0, station_index: 0 }],
        closures_with: [
          { closure_id: 1, station_ref: { section_id: 2, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
      st(2, 0, {
        closures_with: [
          { closure_id: 1, station_ref: { section_id: 1, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
      st(3, 0, {
        closures_with: [
          { closure_id: 2, station_ref: { section_id: 0, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
    ];
    const { data } = fixture(stations);
    const aligned = new Set([1]);

    const got = computeImmovableStationKeys(data, aligned);
    expect([...got].sort()).toEqual(['0:0', '1:0', '2:0']);
    // Matches the original Station[] algorithm exactly.
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), aligned));
  });

  it('returns an empty set when there is no zero-datum', () => {
    const { data } = fixture([st(0, 0), st(0, 1, { merged_with: [{ section_id: 0, station_index: 0 }] })]);
    const got = computeImmovableStationKeys(data, new Set<number>());
    expect(got.size).toBe(0);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), new Set<number>()));
  });

  it('handles a merge cycle without looping and matches the oracle', () => {
    const stations = [
      st(0, 0, { is_zero_datum: true, merged_with: [{ section_id: 0, station_index: 1 }] }),
      st(0, 1, { merged_with: [{ section_id: 0, station_index: 0 }] }), // cycle back
    ];
    const { data } = fixture(stations);
    const got = computeImmovableStationKeys(data, new Set<number>());
    expect([...got].sort()).toEqual(['0:0', '0:1']);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), new Set<number>()));
  });
});
