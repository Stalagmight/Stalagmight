import { describe, expect, it } from 'vitest';
import { buildLabelDescriptors, type LabelBuildContext, type LabelDescriptor } from '../labelTessellate';
import { LABEL_FLAG, LABEL_KIND } from '../labelShaders';
import { emptySurveyData, shotKindToFlags, type SurveyData } from '../../../../../types/survey-data';
import type { StationRef } from '../../../../../types/project';

/**
 * One station label per merge group (SPEC 4.2, Labels): the group's
 * representative is a station that is not its line's first, then not its
 * last, ties broken by the line's order in the project and the station index.
 * Both label modes (Station IDs, Station names) draw it; unmerged stations
 * are untouched.
 */

interface LineSpec {
  id: number;
  name: string;
  shots: number;
  kinds?: Array<'Normal' | 'Jump' | 'Reference'>;
}

/** Lines laid out in `lines` order (the project order), stations named by
 *  index unless `names` says otherwise, Station IDs `100 + global`. `merges`
 *  are junctions: their slots share the first slot's Station ID and each
 *  member's `merged_with` lists the rest. */
function survey(
  lines: LineSpec[],
  merges: StationRef[][],
  names: Record<string, string> = {},
): SurveyData {
  const d = emptySurveyData();
  let shotOffset = 0;
  const lineIds: number[] = [];
  const indices: number[] = [];
  const shotLines: number[] = [];
  const flags: number[] = [];
  d.lines = lines.map((l) => {
    const meta = {
      id: l.id, name: l.name, color: '#4ecdc4', direction: 'in' as never, quality: 0,
      next_shot_id: l.shots, shotOffset, shotCount: l.shots,
    };
    for (let i = 0; i < l.shots; i++) {
      shotLines.push(l.id);
      flags.push(shotKindToFlags(l.kinds?.[i] ?? 'Normal'));
    }
    for (let i = 0; i <= l.shots; i++) {
      lineIds.push(l.id);
      indices.push(i);
    }
    shotOffset += l.shots;
    return meta;
  });
  d.shotCount = shotOffset;
  d.shotIds = new Uint32Array(shotLines.map((_, i) => i));
  d.shotLineIds = new Uint32Array(shotLines);
  d.shotFlags = new Uint8Array(flags);
  d.shotAzimuthIn = new Float32Array(shotOffset);
  d.shotLength = new Float32Array(shotOffset).fill(5);
  d.shotDepthIn = new Float32Array(shotOffset);
  d.shotDepthOut = new Float32Array(shotOffset);
  d.stationCount = lineIds.length;
  d.stationLineIds = new Uint32Array(lineIds);
  d.stationIndices = new Uint32Array(indices);
  d.stationIds = new Uint32Array(lineIds.map((_, g) => 100 + g));
  d.stationCoords = new Float64Array(lineIds.length * 3);
  d.stationFlags = new Uint8Array(lineIds.length);
  d.stationNames = new Map(
    lineIds.map((line, g) => [g, names[`${line}:${indices[g]}`] ?? String(indices[g])]),
  );
  const globalOf = (ref: StationRef) =>
    lineIds.findIndex((line, g) => line === ref.line_id && indices[g] === ref.station_index);
  // A junction is one station: its slots share the first listed slot's
  // Station ID (the wire's shared id) and list each other in `merged_with`.
  for (const group of merges) {
    const shared = d.stationIds[globalOf(group[0]!)]!;
    for (const ref of group) {
      d.stationMergedWith.set(globalOf(ref), group.filter((other) => other !== ref));
      d.stationIds[globalOf(ref)] = shared;
    }
  }
  return d;
}

const ref = (line_id: number, station_index: number): StationRef => ({ line_id, station_index });

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

function stationLabels(d: SurveyData, mode: 'id' | 'name' = 'name'): LabelDescriptor[] {
  return buildLabelDescriptors(d, null, 0, 0, { ...CTX, stationLabelText: mode }).filter(
    (x) => x.kind === LABEL_KIND.STATION_LABEL,
  );
}

const texts = (labels: LabelDescriptor[]) => labels.map((l) => `${l.lineId}:${l.stationIndex}=${l.text}`);

describe('one station label per merge group', () => {
  it("draws the main line's junction name, not the branch's start", () => {
    // Main 0..4 with station 2 renamed Junction; Side starts there.
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2 }],
      [[ref(1, 2), ref(2, 0)]],
      { '1:2': 'Junction' },
    );
    expect(texts(stationLabels(d))).toEqual([
      '1:0=0', '1:1=1', '1:2=Junction', '1:3=3', '1:4=4', '2:1=1', '2:2=2',
    ]);
    // Station IDs mode draws the same representative's ID, once.
    const ids = stationLabels(d, 'id');
    expect(ids.map((l) => l.text)).toEqual(['100', '101', '102', '103', '104', '106', '107']);
    // Order does not decide it: the branch listed first in the project still
    // labels the junction with the main line's station.
    const swapped = survey(
      [{ id: 2, name: 'Side', shots: 2 }, { id: 1, name: 'Main', shots: 4 }],
      [[ref(2, 0), ref(1, 2)]],
      { '1:2': 'Junction' },
    );
    expect(texts(stationLabels(swapped)).filter((t) => t.includes('Junction') || t.startsWith('2:0'))).toEqual([
      '1:2=Junction',
    ]);
  });

  it('prefers a station that is not its line start, then one that is not its line end', () => {
    // End to end: Main's last station and Side's first. Main's end wins (it
    // is not a line start).
    const ends = survey(
      [{ id: 2, name: 'Side', shots: 1 }, { id: 1, name: 'Main', shots: 2 }],
      [[ref(1, 2), ref(2, 0)]],
    );
    const at = (labels: LabelDescriptor[], lineId: number, index: number) =>
      labels.find((l) => l.lineId === lineId && l.stationIndex === index);
    expect(at(stationLabels(ends), 1, 2)?.text).toBe('2');
    expect(at(stationLabels(ends), 2, 0)).toBeUndefined();

    // A middle station beats a line end, whatever the line order.
    const middle = survey(
      [{ id: 1, name: 'Main', shots: 2 }, { id: 2, name: 'Side', shots: 2 }],
      [[ref(1, 2), ref(2, 1)]],
    );
    expect(at(stationLabels(middle), 2, 1)?.text).toBe('1');
    expect(at(stationLabels(middle), 1, 2)).toBeUndefined();
  });

  it("breaks a tie by the line's order in the project, then by station index", () => {
    // Two middle stations: the line listed first in the project wins, not the
    // lower line id.
    const d = survey(
      [{ id: 5, name: 'Listed first', shots: 4 }, { id: 1, name: 'Listed second', shots: 4 }],
      [[ref(1, 1), ref(5, 3)]],
    );
    const labels = stationLabels(d);
    expect(labels.filter((l) => l.stationGroup !== undefined).map((l) => `${l.lineId}:${l.stationIndex}`)).toEqual([
      '5:3',
    ]);
    // A loop merged onto itself: the lower station index of the line.
    const loop = survey([{ id: 1, name: 'Loop', shots: 5 }], [[ref(1, 4), ref(1, 2)]]);
    const own = stationLabels(loop).filter((l) => l.stationGroup !== undefined);
    expect(own.map((l) => l.stationIndex)).toEqual([2]);
  });

  it('keeps the first-in-line rule: the group label always shows when a member starts its line', () => {
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2 }],
      [[ref(1, 2), ref(2, 0)]],
    );
    const junction = stationLabels(d).find((l) => l.lineId === 1 && l.stationIndex === 2)!;
    expect(junction.flags & LABEL_FLAG.FIRST_IN_LINE).not.toBe(0);
    expect(junction.ordinal).toBe(2);
    expect(junction.stationGroup).toEqual([2, 5]);
    // A station that is not merged keeps its own flags.
    const plain = stationLabels(d).find((l) => l.lineId === 1 && l.stationIndex === 1)!;
    expect(plain.flags & LABEL_FLAG.FIRST_IN_LINE).toBe(0);
    expect(plain.stationGroup).toBeUndefined();
    expect(stationLabels(d).find((l) => l.lineId === 1 && l.stationIndex === 0)!.flags & LABEL_FLAG.FIRST_IN_LINE).not.toBe(0);
  });

  it('hides the group label with the shots only when every member would hide', () => {
    // Side is all jump: its start alone would carry ALL_JUMP, but the
    // junction is on Main's normal shots, so the group label never hides.
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2, kinds: ['Jump', 'Jump'] }],
      [[ref(1, 2), ref(2, 0)]],
    );
    const junction = stationLabels(d).find((l) => l.lineId === 1 && l.stationIndex === 2)!;
    expect(junction.flags & (LABEL_FLAG.ALL_JUMP | LABEL_FLAG.ALL_REFERENCE)).toBe(0);
    // Two virtual members of different kinds: drawn while either kind is.
    const virtual = survey(
      [
        { id: 1, name: 'Jumps', shots: 2, kinds: ['Jump', 'Jump'] },
        { id: 2, name: 'Refs', shots: 2, kinds: ['Reference', 'Reference'] },
      ],
      [[ref(1, 1), ref(2, 1)]],
    );
    const shared = stationLabels(virtual).find((l) => l.stationGroup !== undefined)!;
    expect(shared.flags & LABEL_FLAG.ALL_JUMP).not.toBe(0);
    expect(shared.flags & LABEL_FLAG.ALL_REFERENCE).not.toBe(0);
  });

  it('labels the visible part of a group whose representative is on a hidden line', () => {
    // Main is hidden (not in the SoA); Side's start still gets its label.
    const d = survey(
      [{ id: 2, name: 'Side', shots: 2 }],
      [],
    );
    d.stationMergedWith.set(0, [ref(1, 2)]);
    expect(texts(stationLabels(d))).toEqual(['2:0=0', '2:1=1', '2:2=2']);
  });

  it('skips a member whose name shows as nothing when choosing the representative', () => {
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2 }],
      [[ref(1, 2), ref(2, 0)]],
      { '1:2': '⁦⁩' },
    );
    const labels = stationLabels(d);
    expect(labels.find((l) => l.lineId === 2 && l.stationIndex === 0)?.text).toBe('0');
    expect(labels.find((l) => l.lineId === 1 && l.stationIndex === 2)).toBeUndefined();
  });

  it('leaves the station comments of every member alone', () => {
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2 }],
      [[ref(1, 2), ref(2, 0)]],
    );
    d.stationComments = new Map([[2, 'main note'], [5, 'side note']]);
    const comments = buildLabelDescriptors(d, null, 0, 0, CTX).filter((x) => x.kind === LABEL_KIND.STATION_COMMENT);
    expect(comments.map((c) => c.text)).toEqual(['main note', 'side note']);
  });

  it('groups the slots that share a Station ID, whatever merged_with says', () => {
    // The station graph's wire: a junction's slots carry ONE Station ID. The
    // label groups by it, so a slot whose merged_with list is missing (or
    // stale) still draws no second label at the junction.
    const d = survey(
      [{ id: 1, name: 'Main', shots: 4 }, { id: 2, name: 'Side', shots: 2 }],
      [],
    );
    d.stationIds[5] = d.stationIds[2]!;
    expect(texts(stationLabels(d))).toEqual([
      '1:0=0', '1:1=1', '1:2=2', '1:3=3', '1:4=4', '2:1=1', '2:2=2',
    ]);
    expect(stationLabels(d, 'id').filter((l) => l.text === '102')).toHaveLength(1);
  });
});
