import { describe, expect, it } from 'vitest';
import { computeIssueSeverity, SEV_CLEAR, SEV_WARNING, SEV_ERROR } from '../severity';
import { emptySurveyData, type SurveyData } from '../../../../types/survey-data';
import type { ProjectWarning } from '../../../../types/commands';
import type { WarningSeverity } from '../../../../types/ipc/WarningSeverity';

/**
 * Build a minimal SurveyData from a compact spec, populating only the fields
 * computeIssueSeverity reads. Stations are listed in global order.
 */
function buildSurveyData(spec: {
  sections: { id: number; shotIds: number[] }[];
  stations: { sectionId: number; stationIndex: number }[];
  /** Groups of GLOBAL station indices that are merged (fully transitive). */
  merged?: number[][];
}): SurveyData {
  const data = emptySurveyData();

  let offset = 0;
  for (const s of spec.sections) {
    data.sections.push({
      id: s.id,
      name: null,
      surveyor: null,
      date: null,
      color: '#ffffff',
      direction: 'in',
      quality: 1,
      next_shot_id: 1,
      shotOffset: offset,
      shotCount: s.shotIds.length,
    });
    offset += s.shotIds.length;
  }

  const totalShots = offset;
  data.shotCount = totalShots;
  data.shotIds = new Uint32Array(totalShots);
  data.shotSectionIds = new Uint32Array(totalShots);
  let c = 0;
  for (const s of spec.sections) {
    for (const id of s.shotIds) {
      data.shotIds[c] = id;
      data.shotSectionIds[c] = s.id;
      c++;
    }
  }

  data.stationCount = spec.stations.length;
  data.stationSectionIds = new Uint32Array(spec.stations.length);
  data.stationIndices = new Uint32Array(spec.stations.length);
  spec.stations.forEach((st, i) => {
    data.stationSectionIds[i] = st.sectionId;
    data.stationIndices[i] = st.stationIndex;
  });

  if (spec.merged) {
    for (const group of spec.merged) {
      for (const g of group) {
        // Mirror the backend: every member lists every OTHER member.
        const others = group
          .filter((x) => x !== g)
          .map((x) => ({
            section_id: spec.stations[x]!.sectionId,
            station_index: spec.stations[x]!.stationIndex,
          }));
        data.stationMergedWith.set(g, others);
      }
    }
  }

  return data;
}

function warn(
  sectionId: number,
  shotId: number,
  severity: WarningSeverity,
  isResolved = false,
): ProjectWarning {
  return { sectionId, shotId, shotIndex: 0, code: 'TEST', message: 'm', severity, isResolved };
}

/**
 * Standard fixture:
 *   section 1: shots [101, 102] → stations idx 0,1,2 (global 0,1,2)
 *   section 2: shot  [201]      → stations idx 0,1   (global 3,4)
 * shot 101 (g0) connects global 0-1; shot 102 (g1) connects global 1-2;
 * shot 201 (g2) connects global 3-4.
 */
function fixture(merged?: number[][]): SurveyData {
  return buildSurveyData({
    sections: [
      { id: 1, shotIds: [101, 102] },
      { id: 2, shotIds: [201] },
    ],
    stations: [
      { sectionId: 1, stationIndex: 0 },
      { sectionId: 1, stationIndex: 1 },
      { sectionId: 1, stationIndex: 2 },
      { sectionId: 2, stationIndex: 0 },
      { sectionId: 2, stationIndex: 1 },
    ],
    ...(merged ? { merged } : {}),
  });
}

describe('computeIssueSeverity', () => {
  it('no warnings ⇒ everything clear (green)', () => {
    const { shotSeverity, stationSeverity } = computeIssueSeverity(fixture(), []);
    expect([...shotSeverity]).toEqual([SEV_CLEAR, SEV_CLEAR, SEV_CLEAR]);
    expect([...stationSeverity]).toEqual([SEV_CLEAR, SEV_CLEAR, SEV_CLEAR, SEV_CLEAR, SEV_CLEAR]);
  });

  it('an error shot colors the shot and both its endpoint stations red', () => {
    const { shotSeverity, stationSeverity } = computeIssueSeverity(fixture(), [
      warn(1, 101, 'error'),
    ]);
    expect([...shotSeverity]).toEqual([SEV_ERROR, SEV_CLEAR, SEV_CLEAR]);
    // global 0 & 1 are endpoints of shot 101; global 2 untouched.
    expect([...stationSeverity]).toEqual([SEV_ERROR, SEV_ERROR, SEV_CLEAR, SEV_CLEAR, SEV_CLEAR]);
  });

  it('error beats warning on the same shot', () => {
    const { shotSeverity } = computeIssueSeverity(fixture(), [
      warn(1, 101, 'warning'),
      warn(1, 101, 'error'),
    ]);
    expect(shotSeverity[0]).toBe(SEV_ERROR);
  });

  it('resolved warnings are ignored', () => {
    const { shotSeverity, stationSeverity } = computeIssueSeverity(fixture(), [
      warn(1, 101, 'error', true), // resolved → does not count
      warn(1, 102, 'warning'), // unresolved → counts
    ]);
    expect([...shotSeverity]).toEqual([SEV_CLEAR, SEV_WARNING, SEV_CLEAR]);
    // shot 102 connects global 1-2 ⇒ those are yellow; global 0 clear.
    expect([...stationSeverity]).toEqual([SEV_CLEAR, SEV_WARNING, SEV_WARNING, SEV_CLEAR, SEV_CLEAR]);
  });

  it('a station shared by two shots takes the highest severity', () => {
    const { stationSeverity } = computeIssueSeverity(fixture(), [
      warn(1, 101, 'warning'), // global 0,1 ← warning
      warn(1, 102, 'error'), // global 1,2 ← error
    ]);
    // global 1 is shared by both shots ⇒ error wins.
    expect([...stationSeverity]).toEqual([SEV_WARNING, SEV_ERROR, SEV_ERROR, SEV_CLEAR, SEV_CLEAR]);
  });

  it('severity propagates to merged stations', () => {
    // Merge global 2 (section1 idx2) with global 3 (section2 idx0).
    const { stationSeverity } = computeIssueSeverity(fixture([[2, 3]]), [
      warn(2, 201, 'warning'), // shot 201 connects global 3-4 ⇒ both yellow
    ]);
    // global 3 is merged with global 2 ⇒ global 2 inherits yellow.
    expect(stationSeverity[2]).toBe(SEV_WARNING);
    expect(stationSeverity[3]).toBe(SEV_WARNING);
    expect(stationSeverity[4]).toBe(SEV_WARNING);
    expect(stationSeverity[0]).toBe(SEV_CLEAR);
    expect(stationSeverity[1]).toBe(SEV_CLEAR);
  });

  it('propagates across a full (transitive) merge group, taking the max', () => {
    // Group {global 2, 3, 4}; error on shot 102 flags global 2 (its endpoint).
    const { stationSeverity } = computeIssueSeverity(fixture([[2, 3, 4]]), [
      warn(1, 102, 'error'), // shot 102 connects global 1-2 ⇒ global 2 red
    ]);
    expect(stationSeverity[2]).toBe(SEV_ERROR);
    expect(stationSeverity[3]).toBe(SEV_ERROR); // via the group, not a direct shot
    expect(stationSeverity[4]).toBe(SEV_ERROR);
    expect(stationSeverity[1]).toBe(SEV_ERROR); // direct endpoint of shot 102
    expect(stationSeverity[0]).toBe(SEV_CLEAR);
  });

  it('a station with no shots and no flagged merge-partner stays clear', () => {
    const data = buildSurveyData({
      sections: [
        { id: 1, shotIds: [101] },
        { id: 3, shotIds: [] }, // section with a lone, shot-less station
      ],
      stations: [
        { sectionId: 1, stationIndex: 0 },
        { sectionId: 1, stationIndex: 1 },
        { sectionId: 3, stationIndex: 0 }, // global 2 - isolated
      ],
    });
    const { stationSeverity } = computeIssueSeverity(data, [warn(1, 101, 'error')]);
    expect(stationSeverity[2]).toBe(SEV_CLEAR);
  });

  it('over-anchoring colors every datum station, not just the representative shot', () => {
    // The warning is attached to one representative shot (101 → g0/g1) but its
    // datumRefs include section 2 station 0 (g3), which is not on that shot and
    // (here) not merged with it. Both datum stations must still be colored (#57).
    const w: ProjectWarning = {
      sectionId: 1,
      shotId: 101,
      shotIndex: 0,
      code: 'multiple_datums_per_merge_component',
      message: 'm',
      severity: 'error',
      isResolved: false,
      datumRefs: [
        { section_id: 1, station_index: 0 },
        { section_id: 2, station_index: 0 },
      ],
    };
    const { stationSeverity } = computeIssueSeverity(fixture(), [w]);
    expect(stationSeverity[0]).toBe(SEV_ERROR); // representative datum
    expect(stationSeverity[3]).toBe(SEV_ERROR); // the other datum, off the representative shot
  });

  it('ignores warnings for shots not present in the SoA without throwing', () => {
    expect(() => computeIssueSeverity(fixture(), [warn(999, 999, 'error')])).not.toThrow();
    const { shotSeverity, stationSeverity } = computeIssueSeverity(fixture(), [
      warn(999, 999, 'error'),
      warn(1, 999, 'error'), // valid section, unknown shotId
    ]);
    expect([...shotSeverity]).toEqual([SEV_CLEAR, SEV_CLEAR, SEV_CLEAR]);
    expect([...stationSeverity]).toEqual([SEV_CLEAR, SEV_CLEAR, SEV_CLEAR, SEV_CLEAR, SEV_CLEAR]);
  });
});
