/**
 * Zero-datum config building and the reverse rewrite it applies (#24).
 *
 * `stationIndex` arrives already addressing the POST-EDIT station order - the
 * dialog re-anchors the committed pick on every shot-table edit, and both the
 * dropdown and the preview are built from the surviving shots. So the only
 * translation left here is the reverse mirror, plus a clamp that keeps a stale
 * pick from mirroring NEGATIVE: `station_index` is `u32` on the Rust side, so
 * serde would reject the whole import with an opaque error.
 *
 * The regression these tests pin: this function used to run the pick through
 * `adjustStationForDeletedShots`, whose domain is the `EditableShot.id` space
 * (positions in the `isImportableDmpShot`-filtered file order), NOT the
 * post-edit order. That shrank an already-post-edit index a second time and
 * moved the GPS anchor one shot per deletion.
 */
import { describe, it, expect } from 'vitest';
import { buildZeroDatumSectionConfig, finalizePlacementForReverse } from '../importSectionConfig';
import type { ZeroDatumConfigResult } from '../../components/Dialogs/ZeroDatumConfigDialog';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { ImportPlacement } from '../../types/commands';
import type { SectionEditState } from '../batchImportHelpers';
import type { VirtualConnection } from '../segmentContext';

const GPS = { latitude: 20.2, longitude: -87.4, depth_reference: 0 };

/**
 * `id` is the shot's 0-based position in the `isImportableDmpShot`-filtered file
 * order. Deliberately NOT what a station index addresses - the ids are set here
 * only so the fixture looks like a real one; nothing under test reads them.
 */
function shot(id: number, isDeleted = false): EditableShot {
  return {
    id,
    azimuth_in: 10,
    azimuth_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    isDeleted,
  } as unknown as EditableShot;
}

/** A 10-shot section with `deleted` shot ids removed and station `stationIndex` picked. */
function zeroDatum(
  stationIndex: number,
  reverse: boolean,
  deleted: number[] = [],
  shotCount = 10,
): ZeroDatumConfigResult {
  return {
    sectionIndex: 2,
    stationIndex,
    gps: GPS,
    metadata: {
      name: 'Anchor',
      surveyors: [{ name: 'S' }],
      explorers: [],
      date: '2026-01-01',
      color: '#4ecdc4',
      direction: 'in',
      quality: 1,
      reverse,
    },
    shots: Array.from({ length: shotCount }, (_, i) => shot(i, deleted.includes(i))),
  };
}

function stationIndexOf(config: ReturnType<typeof buildZeroDatumSectionConfig>): number {
  expect(config.placement.type).toBe('zero_datum');
  return (config.placement as Extract<ImportPlacement, { type: 'zero_datum' }>).station_index;
}

describe('buildZeroDatumSectionConfig', () => {
  it('carries the section metadata and the surviving shots', () => {
    const config = buildZeroDatumSectionConfig(zeroDatum(0, false, [3]));
    expect(config.section_index).toBe(2);
    expect(config.name).toBe('Anchor');
    expect(config.surveyor).toBeNull();
    expect(config.surveyors).toEqual([{ name: 'S' }]);
    expect(config.color).toBe('#4ecdc4');
    expect(config.reverse).toBe(false);
    expect(config.shots).toHaveLength(9);
    expect(config.station_comments).toEqual([]);
  });

  it('sends the dialog people and date as overrides of every imported shot', () => {
    // Injection: drop `...importCreditFields(...)` (or send the legacy text
    // instead) and the backend gets no surveyors / explorers / override_date,
    // so the pills the user entered never reach a shot.
    const base = zeroDatum(0, false);
    const config = buildZeroDatumSectionConfig({
      ...base,
      metadata: {
        ...base.metadata,
        surveyors: [{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }],
        explorers: [{ name: 'Cy' }],
      },
    });
    expect(config.surveyor).toBeNull();
    expect(config.surveyors).toEqual([{ name: 'Ann' }, { name: 'Bo', email: 'bo@example.org' }]);
    expect(config.explorers).toEqual([{ name: 'Cy' }]);
    expect(config.override_date).toBe('2026-01-01');
    expect(config.date).toBe('2026-01-01');
  });

  it('leaves the shots as the file credits them when no people or date are set', () => {
    // An empty list must NOT be sent: `surveyors: []` would replace what the
    // file said with nobody.
    const base = zeroDatum(0, false);
    const config = buildZeroDatumSectionConfig({
      ...base,
      metadata: { ...base.metadata, surveyors: [], explorers: [], date: null },
    });
    expect('surveyors' in config).toBe(false);
    expect('explorers' in config).toBe(false);
    expect('override_date' in config).toBe(false);
    expect(config.date).toBeNull();
  });

  it('sends an in-range pick verbatim, deletions or not', () => {
    // Station 5 of the 9 shots being sent IS station 5. The dialog already
    // re-anchored the pick when the row was deleted; adjusting again here
    // dragged the anchor to station 4, one shot down the passage.
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(5, false, [0])))).toBe(5);
  });

  it('mirrors the pick against the shots actually sent', () => {
    // 9 shots survive and the pick is station 5 of those, so the reversed
    // section numbers it 9 - 5 = 4. Adjusting first mirrored 9 - 4 = 5 - one
    // station off the one the user chose, in the opposite direction.
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(5, true, [0])))).toBe(4);
  });

  it('never emits a negative index when a trailing shot was deleted', () => {
    // A pick at station 10 that only 9 shots survive under: clamped to 9, then
    // mirrored to 0. Mirroring unclamped gave 9 - 10 = -1, which serde rejects
    // for a u32 - killing the whole import with an opaque error.
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(10, true, [9])))).toBe(0);
  });

  it('keeps a pick past the surviving shots inside the section', () => {
    // A pick the shot editor then deleted out from under: the clamp caps it at
    // the last surviving station, so the mirror stays in range (unclamped it
    // would have mirrored to 1 - 9 = -8).
    const config = buildZeroDatumSectionConfig(zeroDatum(9, true, [1, 2, 3, 4, 5, 6, 7, 8, 9]));
    expect(config.shots).toHaveLength(1);
    expect(stationIndexOf(config)).toBe(0);
  });

  it('clamps a stale pick to the last real station, in both directions', () => {
    // 9 shots, so stations 0..9; a pick of 10 addresses nothing.
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(10, false, [], 9)))).toBe(9);
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(10, true, [], 9)))).toBe(0);
  });

  it('never emits a negative index for any reversed pick', () => {
    for (let pick = 0; pick <= 14; pick++) {
      for (const deleted of [[], [0], [9], [1, 2, 3, 4, 5, 6, 7, 8, 9]]) {
        const config = buildZeroDatumSectionConfig(zeroDatum(pick, true, deleted));
        expect(stationIndexOf(config)).toBeGreaterThanOrEqual(0);
        expect(stationIndexOf(config)).toBeLessThanOrEqual(config.shots.length);
      }
    }
  });

  it('leaves an unreversed pick at the far end alone', () => {
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(10, false)))).toBe(10);
    expect(stationIndexOf(buildZeroDatumSectionConfig(zeroDatum(10, true)))).toBe(0);
  });
});

describe('finalizePlacementForReverse', () => {
  const placement: ImportPlacement = {
    type: 'from_importing',
    import_station: 1,
    target_section_index: 0,
    target_station_index: 2,
  };
  const connection: VirtualConnection = {
    importSectionIndex: 1,
    importStationIndex: 1,
    targetType: 'importing',
    targetImportIndex: 3,
    targetImportStationIndex: 2,
  };
  const sectionEdits = new Map<number, SectionEditState>([
    [3, { reverse: true, shots: Array.from({ length: 6 }, () => ({})) } as unknown as SectionEditState],
  ]);

  it('mirrors both sides when the connection reaches a reversed target', () => {
    expect(finalizePlacementForReverse(placement, true, 4, connection, sectionEdits)).toEqual({
      ...placement,
      import_station: 3,
      target_station_index: 4,
    });
  });

  it('leaves the target side alone without a connection (direct placement)', () => {
    expect(finalizePlacementForReverse(placement, true, 4, null, sectionEdits)).toEqual({
      ...placement,
      import_station: 3,
    });
  });
});
