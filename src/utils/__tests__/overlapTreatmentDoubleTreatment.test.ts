/**
 * #576 RV5-RL-2: an overlap row already treated by one accepted overlap is not
 * treated again by a second overlap on the same rows.
 *
 * The kb5 parser regression used to reach this through real data (004's back
 * against existing 002 while 004 was also the target of an import-to-import
 * overlap - "the source of the double-treatment bug"); the tuned matcher no
 * longer emits that pair, so nothing exercised the treated-shots guard in
 * `applyAllOverlapTreatments`. These tests drive it directly: import section B
 * is the TARGET of an import-to-import overlap (A -> B) and the IMPORT side of
 * an import-to-existing overlap (B -> existing E) on the same rows, in
 * interpolate mode. Without the guard the second pass averages the already
 * averaged rows again: avg(avg(B, A), E) instead of avg(B, A).
 *
 * All data is synthetic; every length is an exact f32, so the existing side's
 * stored-reading rounding is a no-op here.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { applyAllOverlapTreatments } from '../overlapTreatment';
import type { SectionEditState, ConfirmedOverlapInfo } from '../batchImportHelpers';
import type { CrossSectionSuggestion, DmpFile } from '../../types/commands';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { Project } from '../../types/project';
import { useProjectStore } from '../../store/projectStore';
import { makeDmpFile } from '../../test/factories/dmp';

const SECTION_A = 0;
const SECTION_B = 1;
const EXISTING_E = 5;

const measured = (id: number, length: number): EditableShot =>
  ({
    id,
    azimuth_in: 90,
    azimuth_out: 90,
    length,
    depth_in: 0,
    depth_out: 1,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
  }) as EditableShot;

const edits = (name: string, shots: EditableShot[]): SectionEditState => ({
  name,
  surveyor: '',
  date: '',
  color: '',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots,
});

/** Import A (lengths 10, 12, 14) and import B (lengths 20, 22, 24). */
const freshEdits = () =>
  new Map<number, SectionEditState>([
    [SECTION_A, edits('CaveA import A', [measured(0, 10), measured(1, 12), measured(2, 14)])],
    [SECTION_B, edits('CaveA import B', [measured(0, 20), measured(1, 22), measured(2, 24)])],
  ]);

/** A's front overlaps B's first two rows (import-to-import, B is the target). */
const aOntoB: CrossSectionSuggestion = {
  importSectionIndex: SECTION_A,
  importStationIndex: 0,
  existingStationIndex: 0,
  matchedShots: 2,
  existingMatchedShots: 2,
  preferredMergeOffset: 0,
  score: 100,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'CaveA import B',
  importSectionName: 'CaveA import A',
  targetType: 'importing',
  existingSectionId: null,
  targetImportIndex: SECTION_B,
};

/** B's front overlaps existing E by `matchedShots` rows (import-to-existing,
 *  not confirmed: the import rows are interpolated, not deleted). */
const bOntoExisting = (matchedShots: number): CrossSectionSuggestion => ({
  importSectionIndex: SECTION_B,
  importStationIndex: 0,
  existingStationIndex: 0,
  matchedShots,
  existingMatchedShots: matchedShots,
  preferredMergeOffset: 0,
  score: 90,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'StationA section',
  importSectionName: 'CaveA import B',
  targetType: 'existing',
  existingSectionId: EXISTING_E,
  targetImportIndex: null,
});

const dmpFile: DmpFile = makeDmpFile();
const noConfirmed = new Map<number, ConfirmedOverlapInfo>();

/** Existing E: lengths 40, 42, 44 (ids 50..52). */
beforeEach(() => {
  useProjectStore.setState({
    project: {
      sections: [
        {
          id: EXISTING_E,
          name: 'StationA section',
          shots: [50, 51, 52].map((id, i) => ({ ...measured(id, 40 + 2 * i), id })),
        },
      ],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
  });
});

const lengths = (sectionEdits: Map<number, SectionEditState>, section: number) =>
  sectionEdits.get(section)!.shots.map((s) => (s.isDeleted ? 'deleted' : s.length));

describe('applyAllOverlapTreatments: a treated overlap row is not treated twice (#576 RV5-RL-2)', () => {
  it('an import-to-existing overlap on rows an import-to-import overlap already treated leaves them alone', () => {
    const sectionEdits = freshEdits();

    applyAllOverlapTreatments([aOntoB, bOntoExisting(2)], noConfirmed, sectionEdits, dmpFile, 'interpolate');

    // A -> B: A's overlap rows are deleted, B's are averaged with A's once.
    expect(lengths(sectionEdits, SECTION_A)).toEqual(['deleted', 'deleted', 14]);
    // B -> E on the same rows: already treated, so they stay avg(B, A) = 15, 17
    // (a second pass would make them avg(15, 40) = 27.5 and avg(17, 42) = 29.5).
    expect(lengths(sectionEdits, SECTION_B)).toEqual([15, 17, 24]);
  });

  it('a wider import-to-existing overlap treats only the row the first overlap did not reach', () => {
    const sectionEdits = freshEdits();

    applyAllOverlapTreatments([aOntoB, bOntoExisting(3)], noConfirmed, sectionEdits, dmpFile, 'interpolate');

    // Rows 0 and 1 keep the import-to-import treatment; row 2, untreated so far,
    // is averaged with E's third row once: avg(24, 44) = 34.
    expect(lengths(sectionEdits, SECTION_B)).toEqual([15, 17, 34]);
  });

  it('an import-to-import overlap onto rows an import-to-existing overlap already treated leaves them alone', () => {
    const sectionEdits = freshEdits();

    applyAllOverlapTreatments([bOntoExisting(2), aOntoB], noConfirmed, sectionEdits, dmpFile, 'interpolate');

    // B -> E first: B's rows are averaged with E's once, avg(20, 40) = 30 and
    // avg(22, 42) = 32. A -> B then finds them treated and keeps them (a second
    // pass would make them avg(30, 10) = 20 and avg(32, 12) = 22).
    expect(lengths(sectionEdits, SECTION_B)).toEqual([30, 32, 24]);
    // A's own overlap rows are still deleted: B keeps the overlap.
    expect(lengths(sectionEdits, SECTION_A)).toEqual(['deleted', 'deleted', 14]);
  });
});
