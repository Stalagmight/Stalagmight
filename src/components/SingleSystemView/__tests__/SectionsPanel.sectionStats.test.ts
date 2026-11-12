import { describe, expect, it } from 'vitest';
import { calculateSectionStats } from '../SectionsPanel';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Section } from '../../../types/ipc/Section';
import type { Shot } from '../../../types/ipc/Shot';
import type { ShotKind } from '../../../types/ipc/ShotKind';

function shot(id: number, length: number, kind: ShotKind): Shot {
  return {
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind,
    marker_index: null,
    temperature: null,
  };
}

/** 10 m Normal, 4 m Reference, 3 m Jump: 13 m surveyed, two virtual shots. */
function section(): Section {
  return {
    id: 1,
    name: 'Main',
    surveyor: null,
    date: null,
    color: '#ff0000',
    direction: 'in',
    quality: 1,
    shots: [shot(1, 10, 'Normal'), shot(2, 4, 'Reference'), shot(3, 3, 'Jump')],
    next_shot_id: 4,
    reverse_parity: false,
    gauge_shot_uuid: null,
    gauge_neighbour_uuid: null,
  };
}

/**
 * The sections-list row shows each section's length; it must leave out Reference
 * shots like the project total it sits beside (#689), on both the flat-array
 * path and the legacy `section.shots` fallback.
 */
describe('calculateSectionStats length', () => {
  it('leaves out Reference shots on the flat-array path', () => {
    const s = section();
    const data = surveyDataFromLegacy([s], []);
    const stats = calculateSectionStats(s, data.sections[0], data);
    expect(stats).toEqual({ shotCount: 3, length: 13, referenceCount: 2 });
  });

  it('leaves out Reference shots on the legacy fallback', () => {
    const stats = calculateSectionStats(section(), undefined, null);
    expect(stats).toEqual({ shotCount: 3, length: 13, referenceCount: 2 });
  });
});
