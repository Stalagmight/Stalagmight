/**
 * RS 6 (M-P2-10, U-P2-3): a loop end (a station a loop closure shows as its
 * other end) is never labelled on the map, by name, Station ID or comment;
 * its kept end's label says it, and its point stays drawn. In Station IDs
 * mode a foreign slot's label reads the Station ID it shows. A junction's
 * representative is never a loop end.
 * Injections: dropping the loop-end test in `labelText` fails "never labels
 * a loop end"; reading `stationIds` in IDs mode fails "reads the shown ID".
 */
import { describe, expect, it } from 'vitest';
import { buildLabelDescriptors, type LabelBuildContext, type LabelDescriptor } from '../labelTessellate';
import { LABEL_KIND } from '../labelShaders';
import { emptySurveyData, shotKindToFlags, type SurveyData } from '../../../../../types/survey-data';

const MAIN = 1;
const SIDE = 2;

/** Main 0..2 then Side 0..2 (global 0..5), Station IDs 100 + global. Side 2
 *  is a loop end shown as Main / 0 (Station ID 100), with a comment. */
function survey(): SurveyData {
  const d = emptySurveyData();
  const lineIds = [MAIN, MAIN, MAIN, SIDE, SIDE, SIDE];
  const indices = [0, 1, 2, 0, 1, 2];
  d.lines = [
    { id: MAIN, name: 'Main', color: '#4ecdc4', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 0, shotCount: 2 },
    { id: SIDE, name: 'Side', color: '#4ecdc4', direction: 'in' as never, quality: 0, next_shot_id: 2, shotOffset: 2, shotCount: 2 },
  ];
  d.shotCount = 4;
  d.shotIds = new Uint32Array([0, 1, 2, 3]);
  d.shotLineIds = new Uint32Array([MAIN, MAIN, SIDE, SIDE]);
  d.shotFlags = new Uint8Array([0, 0, 0, 0].map(() => shotKindToFlags('Normal')));
  d.shotAzimuthIn = new Float32Array(4);
  d.shotLength = new Float32Array(4).fill(5);
  d.shotDepthIn = new Float32Array(4);
  d.shotDepthOut = new Float32Array(4);
  d.stationCount = 6;
  d.stationLineIds = new Uint32Array(lineIds);
  d.stationIndices = new Uint32Array(indices);
  d.stationIds = new Uint32Array(lineIds.map((_, g) => 100 + g));
  d.stationCoords = new Float64Array(18);
  d.stationFlags = new Uint8Array(6);
  d.stationNames = new Map([
    [0, 'A'],
    [1, 'B'],
    [2, 'C'],
    [3, 'S0'],
    [4, 'S1'],
    [5, 'A'],
  ]);
  d.stationComments = new Map([[5, 'Arch']]);
  d.stationShown = new Map([[5, { id: 100, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 1n }]]);
  return d;
}

const CTX: LabelBuildContext = {
  colorCtx: {
    showDepthGradient: false,
    depthShallow: '#00bfff',
    depthDeep: '#ff4444',
    depthMin: 0,
    depthMax: 1,
    labelColor: '#e0e0e0',
  },
};

function labels(d: SurveyData, mode: 'id' | 'name', kind: number): LabelDescriptor[] {
  return buildLabelDescriptors(d, null, 0, 0, { ...CTX, stationLabelText: mode }).filter((x) => x.kind === kind);
}

const texts = (list: LabelDescriptor[]) => list.map((l) => `${l.lineId}:${l.stationIndex}=${l.text}`);

describe('map labels of loop ends and foreign slots (RS 6)', () => {
  it('never labels a loop end, by name or Station ID', () => {
    const d = survey();
    expect(texts(labels(d, 'name', LABEL_KIND.STATION_LABEL))).toEqual(['1:0=A', '1:1=B', '1:2=C', '2:0=S0', '2:1=S1']);
    expect(texts(labels(d, 'id', LABEL_KIND.STATION_LABEL))).toEqual([
      '1:0=100', '1:1=101', '1:2=102', '2:0=103', '2:1=104',
    ]);
  });

  it("never draws a loop end's comment", () => {
    expect(labels(survey(), 'name', LABEL_KIND.STATION_COMMENT)).toEqual([]);
  });

  it('reads the Station ID a foreign slot shows in IDs mode', () => {
    const d = survey();
    // Side 0 shows a junction kept as Main's station 7 (node 103).
    d.stationShown = new Map([[3, { id: 7, home_line: MAIN, foreign: true, loop_end: false }]]);
    expect(texts(labels(d, 'id', LABEL_KIND.STATION_LABEL))).toContain('2:0=7');
  });
});
