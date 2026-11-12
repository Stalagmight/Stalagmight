/**
 * Round-trip and view-shape tests for SurveyData.
 *
 * Coverage goals:
 *   - `getShot(soaFromLegacy(shots), i)` deep-equals the original shot,
 *     for every field including the null sentinels and the sparse
 *     comment map.
 *   - Same for `getStation` / station_ref / gps / merged_with / etc.
 *   - `findShotGlobalIndex` / `findStationGlobalIndex` return correct
 *     indices, including the "not found" path.
 *   - `emptySurveyData()` is consistent (lengths zero, accessors
 *     refuse out-of-range without crashing).
 */
import { describe, expect, it } from 'vitest';
import {
  emptySurveyData,
  findShotGlobalIndex,
  findStationGlobalIndex,
  getShot,
  getStation,
  lazyStationArray,
  surveyDataFromLegacy as soaFromLegacy,
  SHOT_MARKER_NULL,
} from '../survey-data';
import type { Shot } from '../ipc/Shot';
import type { Station } from '../ipc/Station';
import type { Section } from '../ipc/Section';

/** Minimal fixture: 2 sections, 3 shots, 4 stations.
 *  Covers nullable fields, reference shots, comments. */
function fixture(): { sections: Section[]; stations: Station[] } {
  const sections: Section[] = [
    {
      id: 1,
      name: 'Main',
      color: '#ff0000',
      direction: 'in',
      quality: 1.0,
      next_shot_id: 103,
      reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [
        {
          id: 101,
          azimuth_in: 0.0,
          azimuth_out: 0.0,
          length: 10.0,
          depth_in: 0.0,
          depth_out: -1.0,
          pitch_in: 5.0,
          pitch_out: 5.5,
          left: 1.0,
          right: 1.2,
          up: 2.0,
          down: 0.5,
          comment: 'start of survey',
          kind: 'Normal' as const,
          marker_index: 1,
          temperature: 12.0,
        },
        {
          id: 102,
          azimuth_in: 90.0,
          azimuth_out: 91.0,
          length: 5.0,
          depth_in: -1.0,
          depth_out: -2.0,
          pitch_in: null,
          pitch_out: null,
          left: null,
          right: null,
          up: null,
          down: null,
          comment: null,
          kind: 'Reference' as const,
          marker_index: null,
          temperature: null,
        },
      ],
    },
    {
      id: 2,
      name: null,
      color: '#00ff00',
      direction: 'out',
      quality: 0.5,
      next_shot_id: 202,
      reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
      shots: [
        {
          id: 201,
          azimuth_in: 45.0,
          azimuth_out: 45.0,
          length: 7.07,
          depth_in: 0.0,
          depth_out: 0.0,
          pitch_in: null,
          pitch_out: null,
          left: null,
          right: null,
          up: null,
          down: null,
          comment: '',  // empty string → should NOT round-trip as a comment
          kind: 'Normal' as const,
          marker_index: null,
          temperature: null,
        },
      ],
    },
  ];

  const stations: Station[] = [
    {
      station_ref: { section_id: 1, station_index: 0 },
      coordinates: { x: 0, y: 0, z: 0 },
      is_zero_datum: true,
      gps_coordinates: { latitude: 20.5, longitude: -87.3, depth_reference: 0 },
      merged_with: [],
      closures_with: [],
    },
    {
      station_ref: { section_id: 1, station_index: 1 },
      coordinates: { x: 10, y: 0, z: -1 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [{ section_id: 2, station_index: 0 }],
      closures_with: [],
      comment: 'junction',
    },
    {
      station_ref: { section_id: 1, station_index: 2 },
      coordinates: { x: 10, y: 5, z: -2 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [],
      closures_with: [],
    },
    {
      station_ref: { section_id: 2, station_index: 0 },
      coordinates: { x: 10, y: 0, z: -1 },  // merged with section 1, index 1
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [{ section_id: 1, station_index: 1 }],
      closures_with: [],
    },
  ];

  return { sections, stations };
}

describe('SurveyData', () => {
  describe('emptySurveyData', () => {
    it('has zero counts and zero-length typed arrays', () => {
      const data = emptySurveyData();
      expect(data.shotCount).toBe(0);
      expect(data.stationCount).toBe(0);
      expect(data.shotIds.length).toBe(0);
      expect(data.stationCoords.length).toBe(0);
      expect(data.sections).toEqual([]);
      expect(data.shotComments.size).toBe(0);
    });

    it('getShot throws out-of-range', () => {
      const data = emptySurveyData();
      expect(() => getShot(data, 0)).toThrow(/out of range/);
    });

    it('getStation throws out-of-range', () => {
      const data = emptySurveyData();
      expect(() => getStation(data, 0)).toThrow(/out of range/);
    });

    it('findShotGlobalIndex returns null for empty data', () => {
      expect(findShotGlobalIndex(emptySurveyData(), 1, 1)).toBeNull();
    });
  });

  describe('soaFromLegacy + getShot round-trip', () => {
    it('preserves every shot field through the round trip', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);

      /** Compare two shots field-by-field. Numeric fields are checked
       *  with f32-precision tolerance (the SoA stores doubles as f32). */
      const expectShotsEqual = (actual: Shot, expected: Shot) => {
        // Integer / categorical fields - exact equality.
        expect(actual.id).toBe(expected.id);
        expect(actual.kind).toBe(expected.kind);
        expect(actual.marker_index).toBe(expected.marker_index);
        expect(actual.comment).toBe(expected.comment);
        // f32-stored doubles: ~7 decimal digits of precision; relative
        // tolerance of ~1e-6 is safely outside f32 quantization.
        const closeOrNull = (a: number | null, b: number | null) => {
          if (b === null) {
            expect(a).toBeNull();
          } else {
            expect(a).not.toBeNull();
            expect(a as number).toBeCloseTo(b, 4);
          }
        };
        closeOrNull(actual.azimuth_in, expected.azimuth_in);
        closeOrNull(actual.azimuth_out, expected.azimuth_out);
        closeOrNull(actual.length, expected.length);
        closeOrNull(actual.depth_in, expected.depth_in);
        closeOrNull(actual.depth_out, expected.depth_out);
        closeOrNull(actual.pitch_in, expected.pitch_in);
        closeOrNull(actual.pitch_out, expected.pitch_out);
        closeOrNull(actual.left, expected.left);
        closeOrNull(actual.right, expected.right);
        closeOrNull(actual.up, expected.up);
        closeOrNull(actual.down, expected.down);
        closeOrNull(actual.temperature, expected.temperature);
      };

      // Iterate over every shot in declaration order and compare.
      let cursor = 0;
      for (const section of sections) {
        for (const original of section.shots) {
          const viewed = getShot(data, cursor);
          // `comment` of empty string → null in SoA (we don't store empties).
          const expected: Shot = {
            ...original,
            comment: original.comment ? original.comment : null,
          };
          expectShotsEqual(viewed, expected);
          cursor++;
        }
      }
      expect(cursor).toBe(data.shotCount);
    });

    it('shotSectionIds matches the declaring section', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      // shot 0..1 → section 1; shot 2 → section 2
      expect(Array.from(data.shotSectionIds)).toEqual([1, 1, 2]);
    });

    it('section metadata records correct shotOffset/shotCount', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(data.sections[0]!.shotOffset).toBe(0);
      expect(data.sections[0]!.shotCount).toBe(2);
      expect(data.sections[1]!.shotOffset).toBe(2);
      expect(data.sections[1]!.shotCount).toBe(1);
    });
  });

  describe('getStation round-trip', () => {
    it('preserves coordinates, refs, flags, GPS, merges, comments', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);

      stations.forEach((expected, i) => {
        const viewed = getStation(data, i);
        expect(viewed.station_ref).toEqual(expected.station_ref);
        expect(viewed.coordinates).toEqual(expected.coordinates);
        expect(viewed.is_zero_datum).toBe(expected.is_zero_datum);
        expect(viewed.gps_coordinates).toEqual(expected.gps_coordinates);
        expect(viewed.merged_with).toEqual(expected.merged_with);
        // `closures_with` is optional on the wire, and the SoA normalizes the
        // empty case to absent exactly like `get_project_state` does, so
        // compare through that normalization (#441).
        expect(viewed.closures_with ?? []).toEqual(expected.closures_with ?? []);
        // `comment` is an optional field on Station; treat both
        // `undefined` and "absent" as equivalent.
        expect(viewed.comment ?? undefined).toBe(expected.comment ?? undefined);
      });
    });

    // #441: the SoA view must speak the same shape as the wire - a station with
    // no closures OMITS the key rather than materializing `[]`. A regression to
    // `?? []` here would put a shape on screen that the backend never sends, and
    // silently re-teach consumers that the field is always there.
    //
    // Turns red if `getStation` goes back to defaulting the field.
    it('omits closures_with entirely for a station that has none', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);

      const bare = stations.findIndex((s) => (s.closures_with ?? []).length === 0);
      expect(bare).toBeGreaterThanOrEqual(0);

      const viewed = getStation(data, bare);
      expect('closures_with' in viewed).toBe(false);
      expect(viewed.closures_with).toBeUndefined();
    });
  });

  describe('lazyStationArray', () => {
    it('is array-like: length, indexing, iteration match the eager stations', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      const lazy = lazyStationArray(data);

      // Reports as a real array so `.map`/`.filter`/`.slice` stay array-typed.
      expect(Array.isArray(lazy)).toBe(true);
      expect(lazy.length).toBe(data.stationCount);
      expect(lazy.length).toBe(stations.length);

      // Deep-equals what the old eager loop would have produced - which is
      // exactly `getStation(data, i)` for every index.
      for (let i = 0; i < data.stationCount; i++) {
        expect(lazy[i]).toEqual(getStation(data, i));
      }

      // for…of visits every element in order.
      const iterated = [...lazy];
      expect(iterated.length).toBe(data.stationCount);
      iterated.forEach((s, i) => expect(s).toEqual(getStation(data, i)));
    });

    it('supports the read-only array methods consumers use', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      const lazy = lazyStationArray(data);

      // `.find` (tool origin lookups)
      const found = lazy.find(
        (s) => s.station_ref.section_id === 2 && s.station_ref.station_index === 0,
      );
      expect(found).toEqual(getStation(data, 3));

      // `.filter` (fitToSection)
      const section1 = lazy.filter((s) => s.station_ref.section_id === 1);
      expect(section1.length).toBe(3);

      // `.map` (fit-to-bounds coordinate extraction) returns a real array
      const coords = lazy.map((s) => s.coordinates);
      expect(Array.isArray(coords)).toBe(true);
      expect(coords).toEqual(stations.map((s) => s.coordinates));

      // `.some`
      expect(lazy.some((s) => s.is_zero_datum)).toBe(true);
    });

    it('out-of-range index reads undefined; empty data yields an empty view', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      const lazy = lazyStationArray(data);
      expect(lazy[data.stationCount]).toBeUndefined();

      const emptyLazy = lazyStationArray(emptySurveyData());
      expect(emptyLazy.length).toBe(0);
      expect([...emptyLazy]).toEqual([]);
    });

    it('throws on writes/deletes instead of silently no-opping (read-only view)', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      const lazy = lazyStationArray(data);
      // A write against the empty backing target would silently vanish and
      // desync from the SoA; the traps must surface it as a TypeError.
      expect(() => {
        (lazy as unknown as Station[])[0] = getStation(data, 0);
      }).toThrow(TypeError);
      expect(() => {
        (lazy as unknown as Station[]).push(getStation(data, 0));
      }).toThrow(TypeError);
      expect(() => {
        delete (lazy as unknown as Record<number, unknown>)[0];
      }).toThrow(TypeError);
      // Reads still work after the failed writes.
      expect(lazy.length).toBe(data.stationCount);
      expect(lazy[0]).toEqual(getStation(data, 0));
    });
  });

  describe('findShotGlobalIndex', () => {
    it('finds shots by (section_id, shot_id)', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(findShotGlobalIndex(data, 1, 101)).toBe(0);
      expect(findShotGlobalIndex(data, 1, 102)).toBe(1);
      expect(findShotGlobalIndex(data, 2, 201)).toBe(2);
    });

    it('returns null for nonexistent shot or section', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(findShotGlobalIndex(data, 999, 1)).toBeNull();
      expect(findShotGlobalIndex(data, 1, 999)).toBeNull();
    });
  });

  describe('findStationGlobalIndex', () => {
    it('finds stations by (section_id, station_index)', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(findStationGlobalIndex(data, 1, 0)).toBe(0);
      expect(findStationGlobalIndex(data, 1, 1)).toBe(1);
      expect(findStationGlobalIndex(data, 2, 0)).toBe(3);
    });

    it('returns null for nonexistent station', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(findStationGlobalIndex(data, 99, 0)).toBeNull();
      expect(findStationGlobalIndex(data, 1, 99)).toBeNull();
    });
  });

  describe('null sentinel encoding', () => {
    it('NaN in the float arrays maps back to null', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      // shot 1 (the reference shot) has all nullable floats == null
      expect(Number.isNaN(data.shotPitchIn[1]!)).toBe(true);
      const shot1 = getShot(data, 1);
      expect(shot1.pitch_in).toBeNull();
      expect(shot1.pitch_out).toBeNull();
      expect(shot1.left).toBeNull();
      expect(shot1.right).toBeNull();
      expect(shot1.up).toBeNull();
      expect(shot1.down).toBeNull();
      expect(shot1.temperature).toBeNull();
    });

    it('marker_index = -1 maps back to null', () => {
      const { sections, stations } = fixture();
      const data = soaFromLegacy(sections, stations);
      expect(data.shotMarkerIndex[1]).toBe(SHOT_MARKER_NULL);
      expect(getShot(data, 1).marker_index).toBeNull();
      // shot 0 has marker_index = 1
      expect(data.shotMarkerIndex[0]).toBe(1);
      expect(getShot(data, 0).marker_index).toBe(1);
    });
  });

  // #555. Rust mints shot ids as `section_id * 100_000 + sequence_order` in
  // u32, so the id space runs up to 4_294_899_999 at the highest permitted
  // section id (42_948) - well past i32's 2_147_483_647. The SoA used to keep
  // those in an `Int32Array`, which wrapped every id from 2^31 up to a NEGATIVE
  // number, while the very same id read off `section.shots[j].id` (the legacy /
  // lazy object path), off `ProjectWarning.shotId`, and off the map's feature
  // ids stayed unsigned. Consumers that compare the two sides - the immovable
  // shot set (MapCanvas builds it from `section.shots[i].id`, controller.ts
  // tests membership with `surveyData.shotIds[...]`), the severity map
  // (severity.ts keys by `data.shotIds[i]`, looks up by `warning.shotId`) -
  // then silently never matched for those sections.
  describe('u32 shot ids at high section ids (#555)', () => {
    /** Highest section id the backend can mint (42_949 sections, 0-based). */
    const MAX_SECTION_ID = 42_948;
    /** Mirror of Rust's shot-id stride. */
    const SHOT_ID_STRIDE = 100_000;
    const I32_MAX = 2_147_483_647;

    /** One section at `sectionId` whose shots carry the real minted ids for
     *  `seqs`, plus its `seqs.length + 1` stations. */
    function highIdFixture(
      sectionId: number,
      seqs: number[],
    ): { sections: Section[]; stations: Station[] } {
      const shots: Shot[] = seqs.map((seq) => ({
        id: sectionId * SHOT_ID_STRIDE + seq,
        azimuth_in: 0,
        azimuth_out: 0,
        length: 1,
        depth_in: 0,
        depth_out: 0,
        pitch_in: null,
        pitch_out: null,
        left: null,
        right: null,
        up: null,
        down: null,
        comment: null,
        kind: 'Normal' as const,
        marker_index: null,
        temperature: null,
      }));
      const sections: Section[] = [
        {
          id: sectionId,
          name: 'Deep',
          color: '#0000ff',
          direction: 'in',
          quality: 1.0,
          next_shot_id: sectionId * SHOT_ID_STRIDE + seqs.length,
          reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
          shots,
        },
      ];
      const stations: Station[] = shots.map((_, i) => ({
        station_ref: { section_id: sectionId, station_index: i },
        coordinates: { x: i, y: 0, z: -i },
        is_zero_datum: i === 0,
        gps_coordinates: null,
        merged_with: [],
        closures_with: [],
      }));
      stations.push({
        station_ref: { section_id: sectionId, station_index: shots.length },
        coordinates: { x: shots.length, y: 0, z: -shots.length },
        is_zero_datum: false,
        gps_coordinates: null,
        merged_with: [],
        closures_with: [],
      });
      return { sections, stations };
    }

    // 2_147_483_648 is exactly 2^31: the first id an Int32Array turns into
    // -2_147_483_648. 4_294_899_999 is the largest id the section bound allows.
    const CASES: Array<{ label: string; sectionId: number; seqs: number[] }> = [
      { label: 'first id past i32 (section 21_474)', sectionId: 21_474, seqs: [83_648, 83_649] },
      { label: 'highest permitted section (42_948)', sectionId: MAX_SECTION_ID, seqs: [0, 1, 99_999] },
    ];

    for (const { label, sectionId, seqs } of CASES) {
      it(`round-trips shot ids exactly - ${label}`, () => {
        const { sections, stations } = highIdFixture(sectionId, seqs);
        const data = soaFromLegacy(sections, stations);
        const section = sections[0]!;

        // Guard the fixture itself: if these ids ever stop exceeding i32 the
        // test would keep passing while proving nothing.
        for (const shot of section.shots) expect(shot.id).toBeGreaterThan(I32_MAX);

        section.shots.forEach((shot, j) => {
          // The SoA column and the legacy object are the same id, exactly.
          expect(data.shotIds[j]).toBe(shot.id);
          expect(data.shotIds[j]).toBeGreaterThan(I32_MAX);
          // The materializer agrees with the column it reads from.
          expect(getShot(data, j).id).toBe(shot.id);
          expect(data.shotSectionIds[j]).toBe(sectionId);
        });
        // No slot wrapped negative (the exact i32 failure mode).
        expect(Array.from(data.shotIds).filter((id) => id < 0)).toEqual([]);
        expect(Array.from(data.shotSectionIds).filter((id) => id < 0)).toEqual([]);
      });

      it(`finds high-id shots by (section_id, shot_id) - ${label}`, () => {
        const { sections, stations } = highIdFixture(sectionId, seqs);
        const data = soaFromLegacy(sections, stations);
        sections[0]!.shots.forEach((shot, j) => {
          expect(findShotGlobalIndex(data, sectionId, shot.id)).toBe(j);
        });
      });

      it(`SoA-keyed lookups hit when probed with the unsigned id - ${label}`, () => {
        const { sections, stations } = highIdFixture(sectionId, seqs);
        const data = soaFromLegacy(sections, stations);
        // Mirrors surveyLayer/severity.ts: the index is keyed by the SoA
        // column, then probed with the id a ProjectWarning / feature carries.
        const byShotId = new Map<number, number>();
        for (let i = 0; i < data.shotCount; i++) byShotId.set(data.shotIds[i]!, i);
        sections[0]!.shots.forEach((shot, j) => {
          expect(byShotId.get(shot.id)).toBe(j);
        });
      });
    }

    it('station section ids survive the highest permitted section id', () => {
      const { sections, stations } = highIdFixture(MAX_SECTION_ID, [0, 1]);
      const data = soaFromLegacy(sections, stations);
      for (let g = 0; g < data.stationCount; g++) {
        expect(data.stationSectionIds[g]).toBe(MAX_SECTION_ID);
        expect(getStation(data, g).station_ref.section_id).toBe(MAX_SECTION_ID);
        expect(findStationGlobalIndex(data, MAX_SECTION_ID, g)).toBe(g);
      }
    });
  });
});
