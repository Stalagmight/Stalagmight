import { describe, expect, it } from 'vitest';
import type { OrphanPlacementSuggestion } from '../../types/commands';
import type { Project } from '../../types/ipc/Project';
import type { DatumRole } from '../../types/ipc/DatumRole';
import { isRepeatDoublyAnchored } from '../orphanPlacement';

// A repeat of shots 2..3 of section 0 (stations 2 to 4), with a datum at
// each end of the section: station 0 before the repeat, station 6 after it.
const suggestion = {
  targetSectionId: 0,
  targetStationIndex: 2,
  matchedShots: 2,
  direction: 'forward',
} as unknown as OrphanPlacementSuggestion;

function project(roles: [DatumRole, DatumRole]): Project {
  const datum = (station_index: number, role: DatumRole) => ({
    station_ref: { section_id: 0, station_index },
    gps: { latitude: 20.5, longitude: -87.3, depth_reference: 0 },
    role,
    is_entrance: true,
  });
  return {
    zero_datums: [datum(0, roles[0]), datum(6, roles[1])],
    station_merges: [],
    loop_closures: [],
  } as unknown as Project;
}

describe('isRepeatDoublyAnchored with GPS fixes (#741)', () => {
  it('counts two placement datums as anchors at both ends', () => {
    expect(isRepeatDoublyAnchored(suggestion, project(['Placement', 'Placement']))).toBe(true);
  });

  // A GPS fix pins nothing in raw geometry, the rule the backend's import
  // anchor check uses (placement datums only), so it is no anchor here either.
  it('does not count a GPS fix as an anchor', () => {
    expect(isRepeatDoublyAnchored(suggestion, project(['Placement', 'GpsFix']))).toBe(false);
  });
});
