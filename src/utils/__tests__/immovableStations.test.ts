import { describe, it, expect } from 'vitest';
import type { Line } from '../../types/ipc/Line';
import type { Station } from '../../types/ipc/Station';
import { surveyDataFromLegacy, lazyStationArray } from '../../types/survey-data';
import { computeImmovableStationKeys } from '../immovableStations';

/**
 * Regression coverage for the SoA immovable-station BFS (#customRender, Codex
 * round 5, P2). The move tools previously walked a materialized `Station[]`
 * (rebuilding + RETAINING the whole ~89k-object graph via the lazy proxy);
 * `computeImmovableStationKeys` reads the SoA directly instead. These tests pin
 * the behavior by running a `Station[]` walk as an independent oracle over the
 * same fixtures and asserting the SoA helper matches it exactly.
 */

// The original algorithm from MapCanvas before the SoA migration, over a
// materialized Station[], with plan D15's rule written independently of
// `closureTies.ts`: an aligned `closures_with` entry whose `via_station` is a
// slot of neither the holder's station nor the target's (by Station ID) is a
// closure-chain entry and does not tie the holder.
function immovableViaStationArray(
  stations: readonly Station[],
  alignedClosureIds: ReadonlySet<bigint>,
): Set<string> {
  const stationByKey = new Map(
    stations.map((s) => [`${s.station_ref.line_id}:${s.station_ref.station_index}`, s]),
  );
  const stationIdOf = (ref: { line_id: number; station_index: number }): number | undefined =>
    stationByKey.get(`${ref.line_id}:${ref.station_index}`)?.id;
  const immovable = new Set<string>();
  const queue: string[] = [];
  for (const s of stations) {
    if (s.is_zero_datum) {
      queue.push(`${s.station_ref.line_id}:${s.station_ref.station_index}`);
    }
  }
  while (queue.length > 0) {
    const key = queue.pop()!;
    if (immovable.has(key)) continue;
    immovable.add(key);
    const st = stationByKey.get(key);
    if (!st) continue;
    for (const ref of st.merged_with) {
      queue.push(`${ref.line_id}:${ref.station_index}`);
    }
    for (const cr of st.closures_with ?? []) {
      if (!alignedClosureIds.has(cr.closure_id)) continue;
      if (cr.via_station) {
        const via = stationIdOf(cr.via_station);
        if (via !== st.id && via !== stationIdOf(cr.station_ref)) continue;
      }
      queue.push(`${cr.station_ref.line_id}:${cr.station_ref.station_index}`);
    }
  }
  return immovable;
}

const st = (
  line_id: number,
  station_index: number,
  opts: Partial<Station> = {},
): Station => ({
  station_ref: { line_id, station_index },
  coordinates: { x: station_index, y: line_id, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: [],
  closures_with: [],
  // A slot of its own station unless the case says otherwise (a junction's
  // slots share one id).
  id: line_id * 1000 + station_index,
  name: String(station_index),
  ...opts,
});

/** One line per station so `next_shot_id`/shots don't matter; the BFS only
 *  reads station topology. Each station needs a line shell to exist in SoA. */
function fixture(stations: Station[]): { data: ReturnType<typeof surveyDataFromLegacy> } {
  const lineIds = [...new Set(stations.map((s) => s.station_ref.line_id))];
  const lines: Line[] = lineIds.map((id) => ({
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
  return { data: surveyDataFromLegacy(lines, stations) };
}

describe('computeImmovableStationKeys', () => {
  it('propagates from a zero-datum through merged_with and aligned closures, but not unaligned ones', () => {
    // 0:0 is the datum. merged 0:0 <-> 1:0. 1:0 aligned-closure to 2:0.
    // 3:0 links to 0:0 only through an UNALIGNED closure -> must stay movable.
    const stations = [
      st(0, 0, { is_zero_datum: true, merged_with: [{ line_id: 1, station_index: 0 }] }),
      st(1, 0, {
        merged_with: [{ line_id: 0, station_index: 0 }],
        closures_with: [
          { closure_id: 1n, station_ref: { line_id: 2, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
      st(2, 0, {
        closures_with: [
          { closure_id: 1n, station_ref: { line_id: 1, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
      st(3, 0, {
        closures_with: [
          { closure_id: 2n, station_ref: { line_id: 0, station_index: 0 }, via_station: null, is_transitive: false },
        ],
      }),
    ];
    const { data } = fixture(stations);
    const aligned = new Set([1n]);

    const got = computeImmovableStationKeys(data, aligned);
    expect([...got].sort()).toEqual(['0:0', '1:0', '2:0']);
    // Matches the original Station[] algorithm exactly.
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), aligned));
  });

  it('returns an empty set when there is no zero-datum', () => {
    const { data } = fixture([st(0, 0), st(0, 1, { merged_with: [{ line_id: 0, station_index: 0 }] })]);
    const got = computeImmovableStationKeys(data, new Set<bigint>());
    expect(got.size).toBe(0);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), new Set<bigint>()));
  });

  it('handles a merge cycle without looping and matches the oracle', () => {
    const stations = [
      st(0, 0, { is_zero_datum: true, merged_with: [{ line_id: 0, station_index: 1 }] }),
      st(0, 1, { merged_with: [{ line_id: 0, station_index: 0 }] }), // cycle back
    ];
    const { data } = fixture(stations);
    const got = computeImmovableStationKeys(data, new Set<bigint>());
    expect([...got].sort()).toEqual(['0:0', '0:1']);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), new Set<bigint>()));
  });

  // Plan D15. X = 1:1, Y = 2:1, Z = 3:1; closure 7 joins X and Y (unaligned),
  // closure 8 joins X and Z (aligned). The wire gives Y a closure-chain entry
  // "Z via X" under 8, and Z one "Y via X" under 7.
  const x = { line_id: 1, station_index: 1 };
  const y = { line_id: 2, station_index: 1 };
  const z = { line_id: 3, station_index: 1 };
  const chainCase = (datumOn: 'x' | 'y'): Station[] => [
    st(1, 1, {
      is_zero_datum: datumOn === 'x',
      closures_with: [
        { closure_id: 7n, station_ref: y, via_station: null, is_transitive: false },
        { closure_id: 8n, station_ref: z, via_station: null, is_transitive: false },
      ],
    }),
    st(2, 1, {
      is_zero_datum: datumOn === 'y',
      closures_with: [
        { closure_id: 7n, station_ref: x, via_station: null, is_transitive: false },
        { closure_id: 8n, station_ref: z, via_station: x, is_transitive: true },
      ],
    }),
    st(3, 1, {
      closures_with: [
        { closure_id: 8n, station_ref: x, via_station: null, is_transitive: false },
        { closure_id: 7n, station_ref: y, via_station: x, is_transitive: true },
      ],
    }),
  ];

  it("a datum on Y does not freeze the aligned X-Z pair its unaligned closure reaches (D15)", () => {
    const { data } = fixture(chainCase('y'));
    const aligned = new Set([8n]);
    const got = computeImmovableStationKeys(data, aligned);
    // Y's tie group over aligned closures is Y alone: 8 ties X and Z, not Y.
    expect([...got].sort()).toEqual(['2:1']);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), aligned));
  });

  it('a datum on X freezes Z through the aligned closure, and not Y', () => {
    const { data } = fixture(chainCase('x'));
    const aligned = new Set([8n]);
    const got = computeImmovableStationKeys(data, aligned);
    expect([...got].sort()).toEqual(['1:1', '3:1']);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), aligned));
  });

  it("follows an aligned closure attached on a junction's other slot", () => {
    // J is one station on lines 4 and 5 (id 4000); closure 9 joins it, on its
    // 4:0 slot, to W = 6:2. The datum is on the 5:0 slot, which reaches W via 4:0.
    const j = { line_id: 4, station_index: 0 };
    const j2 = { line_id: 5, station_index: 0 };
    const w = { line_id: 6, station_index: 2 };
    const stations = [
      st(4, 0, {
        id: 4000,
        merged_with: [j2],
        closures_with: [{ closure_id: 9n, station_ref: w, via_station: null, is_transitive: false }],
      }),
      st(5, 0, {
        id: 4000,
        is_zero_datum: true,
        merged_with: [j],
        closures_with: [{ closure_id: 9n, station_ref: w, via_station: j, is_transitive: true }],
      }),
      st(6, 2, {
        closures_with: [{ closure_id: 9n, station_ref: j, via_station: null, is_transitive: false }],
      }),
    ];
    const { data } = fixture(stations);
    const aligned = new Set([9n]);
    const got = computeImmovableStationKeys(data, aligned);
    expect([...got].sort()).toEqual(['4:0', '5:0', '6:2']);
    expect(got).toEqual(immovableViaStationArray(lazyStationArray(data), aligned));
  });
});
