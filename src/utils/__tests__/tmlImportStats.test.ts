/**
 * The TML import preview's "Total length" is the length the project shows
 * once imported (#689): Reference shots are left out of every survey length,
 * so the virtual shots the import makes References must be left out of the
 * preview too, per the dialog's virtual-shot mode (#719 review).
 */
import { describe, expect, it } from 'vitest';
import type { TmlFile, TmlSection, TmlShot } from '../../types/commands';
import { computeTmlImportStats, jumpShots } from '../tmlImportStats';

const shot = (id: number, from: number, length: number, shotType: TmlShot['shot_type'] = 'Real', closureTo = -1): TmlShot =>
  ({
    id: BigInt(id),
    from_id: BigInt(from),
    shot_type: shotType,
    azimuth: 0,
    length,
    depth: 1,
    depth_in: 0,
    inclination: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    comment: null,
    explorer: null,
    date: null,
    color: '#3b82f6',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: BigInt(closureTo),
    section: 'S',
  }) as TmlShot;

const section = (name: string, shots: TmlShot[]): TmlSection => ({
  name,
  shots,
  surveyor: null,
  date: null,
  color: '#3b82f6',
  is_branch_split: false,
  start_ariane_id: shots[0]?.from_id ?? 0n,
  end_ariane_id: shots[shots.length - 1]?.id ?? 0n,
  start_depth: 0,
});

const file = (sections: TmlSection[]): TmlFile => ({
  cave_name: 'Cave',
  unit: 'Metric',
  use_magnetic_azimuth: false,
  first_start_absolute_elevation: 0,
  geo_coding: null,
  sections,
  closures: [],
});

// Real 10 m, a 5 m virtual shot between two real stations (bounded: a Jump),
// real 10 m, then a 7 m virtual shot dangling off the end (a Reference).
const bounded = shot(2, 1, 5, 'Virtual');
const dangling = shot(4, 3, 7, 'Virtual');
const line = file([section('A', [shot(1, 0, 10), bounded, shot(3, 2, 10), dangling])]);

describe('TML import preview length', () => {
  it('Auto counts the virtual shots the chain rule makes Jumps, not References', () => {
    expect(jumpShots(line.sections[0]!.shots, [])).toEqual(new Set([bounded]));
    expect(computeTmlImportStats(line, false).totalLength).toBe(25);
    expect(computeTmlImportStats(line, false, null, 'Auto').totalLength).toBe(25);
  });

  it('All jumps counts every virtual shot, All references none', () => {
    expect(computeTmlImportStats(line, false, null, 'AllJumps').totalLength).toBe(32);
    expect(computeTmlImportStats(line, false, null, 'AllReferences').totalLength).toBe(20);
  });

  it('a virtual run reaching a second real point through a loop closure is a Jump', () => {
    // Section B hangs off station 1 with real survey; the dangling virtual
    // shot's far end (4) is closed onto station 6 of B, a real point.
    const closed = file([
      section('A', [shot(1, 0, 10), bounded, shot(3, 2, 10), dangling]),
      section('B', [shot(5, 1, 4), shot(6, 5, 4), shot(7, 4, 0, 'Closure', 6)]),
    ]);
    expect(computeTmlImportStats(closed, false).totalLength).toBe(10 + 5 + 10 + 7 + 4 + 4);
    expect(computeTmlImportStats(closed, false, null, 'AllReferences').totalLength).toBe(28);
  });

  it('a virtual run touching real survey at one point only is a Reference', () => {
    const tie = file([section('A', [shot(1, 0, 10), shot(2, 1, 3, 'Virtual'), shot(9, 2, 3, 'Virtual')])]);
    expect(computeTmlImportStats(tie, false).totalLength).toBe(10);
    expect(computeTmlImportStats(tie, false).virtualCount).toBe(2);
  });
});
