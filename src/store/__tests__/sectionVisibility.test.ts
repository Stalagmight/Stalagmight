import { describe, expect, it } from 'vitest';
import type { Line } from '../../types/ipc/Line';
import { emptySurveyData, type SurveyData } from '../../types/survey-data';
import { computeMapHiddenLines, mapHiddenLines, revealedLineIds, selectMapSurveyData } from '../sectionVisibility';
import type { Selection } from '../uiStore';

const lines = [
  { id: 1, cave_section_id: 7 },
  { id: 2, cave_section_id: 7 },
  { id: 3 },
  { id: 4, cave_section_id: 8 },
] as unknown as Line[];

describe('revealedLineIds', () => {
  it('collects the line of every selected line, shot and station', () => {
    const sel: Selection = { type: 'station', stationRef: { line_id: 4, station_index: 2 } };
    const multi: Selection[] = [{ type: 'line', lineId: 1 }, { type: 'shot', lineId: 2, shotId: 9 }, sel];
    expect([...revealedLineIds(sel, multi)].sort()).toEqual([1, 2, 4]);
    expect(revealedLineIds(null, []).size).toBe(0);
  });
});

describe('computeMapHiddenLines', () => {
  it('hides the lines of hidden Sections, keeps ungrouped and revealed ones', () => {
    expect([...computeMapHiddenLines(lines, [7], new Set()).ids]).toEqual([1, 2]);
    expect(computeMapHiddenLines(lines, [7], new Set()).key).toBe('1,2');
    expect([...computeMapHiddenLines(lines, [7, 8], new Set([2])).ids]).toEqual([1, 4]);
  });

  it('is empty (key "") when nothing is hidden', () => {
    expect(computeMapHiddenLines(lines, [], new Set()).key).toBe('');
    // A hidden Section with no lines hides nothing.
    expect(computeMapHiddenLines(lines, [99], new Set()).key).toBe('');
  });
});

describe('mapHiddenLines', () => {
  it('returns the same object while the contents do not change', () => {
    const ui = { hiddenSections: [7], selection: null, multiSelection: [] as Selection[] };
    const first = mapHiddenLines(lines, ui);
    // A selection elsewhere (an ungrouped line) changes nothing.
    const other: Selection = { type: 'line', lineId: 3 };
    expect(mapHiddenLines(lines, { ...ui, selection: other, multiSelection: [other] })).toBe(first);
    // Selecting a hidden line reveals it.
    const hidden: Selection = { type: 'line', lineId: 1 };
    const next = mapHiddenLines(lines, { ...ui, selection: hidden, multiSelection: [hidden] });
    expect(next.key).toBe('2');
  });
});

describe('selectMapSurveyData', () => {
  function data(): SurveyData {
    const d = emptySurveyData();
    d.lines = [1, 2, 3].map((id, i) => ({
      id, name: null, color: '#000', direction: 'in' as const, quality: 1, next_shot_id: 1, shotOffset: i, shotCount: 1,
    }));
    d.shotCount = 3;
    d.shotIds = Uint32Array.from([100, 200, 300]);
    d.shotLineIds = Uint32Array.from([1, 2, 3]);
    for (const k of ['shotAzimuthIn', 'shotAzimuthOut', 'shotLength', 'shotDepthIn', 'shotDepthOut', 'shotPitchIn',
      'shotPitchOut', 'shotLeft', 'shotRight', 'shotUp', 'shotDown', 'shotTemperature'] as const) {
      d[k] = new Float32Array(3);
    }
    d.shotFlags = new Uint8Array(3);
    d.shotMarkerIndex = new Int32Array(3);
    d.stationCount = 6;
    d.stationLineIds = Uint32Array.from([1, 1, 2, 2, 3, 3]);
    d.stationIndices = Uint32Array.from([0, 1, 0, 1, 0, 1]);
    d.stationCoords = new Float64Array(18);
    d.stationFlags = new Uint8Array(6);
    d.stationIds = Uint32Array.from([1, 2, 3, 4, 5, 6]);
    return d;
  }

  it('picks the active geometry and leaves hidden lines out of it', () => {
    const measured = data();
    const adjusted = data();
    const project = {
      project: { lines },
      stations: [],
      surveyData: measured,
      stationsAdjusted: [],
      surveyDataAdjusted: adjusted,
    };
    const ui = { geometryMode: 'measured' as const, hiddenSections: [] as number[], selection: null, multiSelection: [] };
    expect(selectMapSurveyData(project, ui)).toBe(measured);
    const hidden = selectMapSurveyData(project, { ...ui, hiddenSections: [7] })!;
    expect(hidden.lines.map((l) => l.id)).toEqual([3]);
    const adjustedHidden = selectMapSurveyData(project, { ...ui, geometryMode: 'closureAdjusted', hiddenSections: [7] })!;
    expect(adjustedHidden.lines.map((l) => l.id)).toEqual([3]);
    expect(adjustedHidden).not.toBe(hidden);
    // The same inputs give the same object (the map keeps its cheap paths).
    expect(selectMapSurveyData(project, { ...ui, hiddenSections: [7] })).toBe(hidden);
  });
});
