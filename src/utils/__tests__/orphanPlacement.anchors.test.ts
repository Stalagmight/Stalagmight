import { describe, expect, it } from 'vitest';
import type { OrphanPlacementSuggestion } from '../../types/commands';
import type { Project } from '../../types/ipc/Project';
import { isRepeatDoublyAnchored } from '../orphanPlacement';

// A repeat of shots 2..3 of section 0 (stations 2 to 4). Section 0 holds its
// placement datum at station 0, before the repeat; what happens after it
// (station 6) is what each case varies.
const suggestion = {
  targetSectionId: 0,
  targetStationIndex: 2,
  matchedShots: 2,
  direction: 'forward',
} as unknown as OrphanPlacementSuggestion;

const at = (section_id: number, station_index: number) => ({ section_id, station_index });
const datum = (section_id: number, station_index: number, role: 'Placement' | 'GpsFix') => ({
  station_ref: at(section_id, station_index),
  gps: { latitude: 20.5, longitude: -87.3, depth_reference: 0 },
  role,
  is_entrance: role === 'Placement',
});

function project(over: Partial<Record<'zero_datums' | 'station_merges' | 'loop_closures', unknown[]>>): Project {
  return { zero_datums: [], station_merges: [], loop_closures: [], ...over } as unknown as Project;
}

describe('isRepeatDoublyAnchored with GPS fixes (#741)', () => {
  it('counts an aligned closure to a separately placed section as the far anchor', () => {
    const p = project({
      zero_datums: [datum(0, 0, 'Placement'), datum(9, 0, 'Placement')],
      loop_closures: [{ station_a: at(0, 6), station_b: at(9, 0), is_aligned: true }],
    });
    expect(isRepeatDoublyAnchored(suggestion, p)).toBe(true);
  });

  // A GPS fix pins nothing in raw geometry, the rule the backend's import
  // anchor check uses (placement datums only), so it is no anchor here either:
  // not on the target section itself...
  it('does not count a GPS fix on the target section', () => {
    const p = project({ zero_datums: [datum(0, 0, 'Placement'), datum(0, 6, 'GpsFix')] });
    expect(isRepeatDoublyAnchored(suggestion, p)).toBe(false);
  });

  // ...nor at the end of a path: section 9 is merged to section 0 at station
  // 6, so its datum is a GPS fix of section 0's piece, and the only placement
  // datum it could reach is through section 0 itself.
  it('does not count a path that reaches only a GPS fix', () => {
    const p = project({
      zero_datums: [datum(0, 0, 'Placement'), datum(9, 3, 'GpsFix')],
      station_merges: [{ station_a: at(0, 6), station_b: at(9, 0) }],
    });
    expect(isRepeatDoublyAnchored(suggestion, p)).toBe(false);
  });
});
