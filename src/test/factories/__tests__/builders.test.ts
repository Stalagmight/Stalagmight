/**
 * Pins the data builders' defaults (#576 E15): the tests built on them read these
 * values, so a changed default is a changed fixture in every one of them.
 */
import { describe, expect, it } from 'vitest';
import { makeProject, makeSection, makeShot, makeStation, stationRef, TEST_VALIDATION } from '../survey';
import { makeDmpFile, makeDmpSection, makeDmpShot, makeDmpShots } from '../dmp';
import { makeSpeleoLink } from '../speleo';

describe('survey builders', () => {
  it('build a complete shot in the generated key order, optional uuid absent', () => {
    const shot = makeShot({ id: 7, length: 3 });
    expect(Object.keys(shot)).toEqual([
      'id', 'azimuth_in', 'azimuth_out', 'length', 'depth_in', 'depth_out', 'pitch_in', 'pitch_out',
      'left', 'right', 'up', 'down', 'comment', 'kind', 'marker_index', 'temperature',
    ]);
    expect(shot).toMatchObject({ id: 7, length: 3, azimuth_in: 0, depth_out: 0, pitch_in: null, kind: 'Normal' });
  });

  it('default next_shot_id / next_section_id to one past the highest id', () => {
    expect(makeSection().next_shot_id).toBe(1);
    expect(makeSection({ shots: [makeShot({ id: 1 }), makeShot({ id: 5 })] }).next_shot_id).toBe(6);
    expect(makeSection({ shots: [makeShot({ id: 5 })], next_shot_id: 100 }).next_shot_id).toBe(100);
    expect(makeProject().next_section_id).toBe(1);
    expect(makeProject({ sections: [makeSection({ id: 3 })] }).next_section_id).toBe(4);
  });

  it('section and project defaults', () => {
    expect(makeSection()).toEqual({
      id: 1, name: null, surveyor: null, date: null, color: '#3388ff', direction: 'in', quality: 0, shots: [],
      next_shot_id: 1, reverse_parity: false, gauge_shot_uuid: null, gauge_neighbour_uuid: null,
    });
    const project = makeProject();
    expect(project).toMatchObject({ name: 'CaveA', file_path: null, validation: TEST_VALIDATION, surface_elevation: null });
    expect(project.validation).not.toBe(TEST_VALIDATION);
    expect('default_azimuth_reference' in project).toBe(false);
  });

  it('station defaults leave the optional closures_with / comment out', () => {
    expect(makeStation()).toEqual({
      station_ref: { section_id: 1, station_index: 0 },
      coordinates: { x: 0, y: 0, z: 0 },
      is_zero_datum: false,
      gps_coordinates: null,
      merged_with: [],
    });
    expect(stationRef(2, 9)).toEqual({ section_id: 2, station_index: 9 });
  });
});

describe('speleo link builder', () => {
  it('is a read-only, lock-free, up-to-date link in the interface order', () => {
    expect(makeSpeleoLink({ editState: 'editing' })).toEqual({
      permission: 'READ_AND_WRITE', editState: 'editing', lockHolder: null, lockedByOther: false, hasUnpushedEdits: false,
      unwound: false, forceEditing: false, forceEditAllowed: false, undoMode: 'full', remoteState: 'up_to_date',
      noAccess: false, serverError: null,
    });
  });
});

describe('dmp builders', () => {
  it('build a Standard 5 m shot, separate shots per count, an Anchor section and an empty file', () => {
    expect(makeDmpShot()).toMatchObject({ shot_type: 'Standard', heading_in: 10, heading_out: 10, length: 5, marker_index: 0 });
    expect('comment' in makeDmpShot()).toBe(false);
    const shots = makeDmpShots(2, { length: 4 });
    expect(shots).toHaveLength(2);
    expect(shots[0]).not.toBe(shots[1]);
    expect(shots[1]!.length).toBe(4);
    expect(makeDmpSection()).toEqual({
      version: 1, name: 'Anchor', direction: 'in', date: '2026-01-01', shots: [], content_hash: 'hash-anchor',
    });
    expect(makeDmpFile()).toEqual({ version: 1, sections: [], warnings: [] });
  });
});
