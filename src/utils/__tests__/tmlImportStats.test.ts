import { describe, it, expect } from 'vitest';
import { importableShots, survivesImport } from '../tmlImportStats';
import type { TmlSection, TmlShot, TmlShotType } from '../../types/commands';

/**
 * THE rule for which Ariane rows the import builds (#576 E17), as a truth table:
 * (type, excluded, include-excluded toggle) -> imported.
 *
 * The SAME table pins the Rust rule the converter applies -
 * `survives_import_truth_table` in src-tauri/crates/parsers/src/parsers/tml/mod.rs.
 * The preview recomputes from this TypeScript copy on every toggle, so the two
 * drifting apart would show a shot count and layout the import does not build:
 * change one table, change both.
 */
const TRUTH_TABLE: Array<[TmlShotType, boolean, boolean, boolean]> = [
  ['Real', false, false, true],
  ['Real', false, true, true],
  ['Real', true, false, false],
  ['Real', true, true, true],
  ['Virtual', false, false, true],
  ['Virtual', false, true, true],
  ['Virtual', true, false, false],
  ['Virtual', true, true, true],
  ['Closure', false, false, false],
  ['Closure', false, true, false],
  ['Closure', true, false, false],
  ['Closure', true, true, false],
];

function shot(shotType: TmlShotType, excluded: boolean): TmlShot {
  return {
    id: 1,
    from_id: 0,
    shot_type: shotType,
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
    color: '#000000',
    excluded,
    latitude: 0,
    longitude: 0,
    closure_to_id: -1,
    section: '',
  };
}

describe('survivesImport mirrors the Rust survives_import', () => {
  it.each(TRUTH_TABLE)('%s excluded=%s includeExcluded=%s -> %s', (type, excluded, include, imported) => {
    expect(survivesImport(shot(type, excluded), include)).toBe(imported);
  });

  it('importableShots keeps exactly the rows survivesImport keeps, in order', () => {
    const shots = TRUTH_TABLE.map(([type, excluded]) => shot(type, excluded));
    const section: TmlSection = {
      name: 'S',
      shots,
      surveyor: null,
      date: null,
      color: '#000000',
      is_branch_split: false,
      start_ariane_id: -1,
      end_ariane_id: -1,
      start_depth: 0,
    };
    for (const include of [false, true]) {
      expect(importableShots(section, include)).toEqual(
        shots.filter((s) => survivesImport(s, include))
      );
    }
  });
});
