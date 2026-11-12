/**
 * Overlap treatment must land on the right target-import row even after the
 * user has edited that section in the wizard.
 *
 * `computeTargetShotIndex` answers in the target section's ORIGINAL DMP
 * ordering, and `convertToEditableShots` stamps each original shot's id with
 * that same index. Indexing the edit array by that number went wrong the moment
 * a row was inserted before the overlap: the inserted row shifted every position
 * after it, so the treatment landed on the inserted row and left the real
 * overlap row untreated - the emitted target section came out with the wrong
 * geometry. The fix resolves the row by identity (the non-new row whose id is
 * the DMP index), which follows the shot through inserts and deletions.
 */
import { describe, it, expect } from 'vitest';
import { applyAllOverlapTreatments } from '../overlapTreatment';
import type { SectionEditState, ConfirmedOverlapInfo } from '../batchImportHelpers';
import type { CrossSectionSuggestion, DmpFile } from '../../types/commands';
import type { EditableShot } from '../../components/common/ShotTableEditor';

const measured = (id: number, length: number, overrides: Partial<EditableShot> = {}): EditableShot =>
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
    ...overrides,
  }) as EditableShot;

/** A row added in the wizard and left wholly blank. */
const blankNew = (tempId: number): EditableShot =>
  ({
    id: tempId,
    tempId,
    isNew: true,
    azimuth_in: NaN,
    azimuth_out: NaN,
    length: NaN,
    depth_in: NaN,
    depth_out: NaN,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
  }) as EditableShot;

const edits = (shots: EditableShot[]): SectionEditState => ({
  name: 'S',
  date: '',
  color: '',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots,
});

/** import section 0 (deleted on its side) overlapping target import section 1,
 *  Front + forward, two matched shots against target stations 0 and 1. */
const suggestion: CrossSectionSuggestion = {
  importSectionIndex: 0,
  importStationIndex: 0,
  existingStationIndex: 0,
  matchedShots: 2,
  existingMatchedShots: 2,
  preferredMergeOffset: 0,
  score: 100,
  direction: 'forward',
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'target',
  importSectionName: 'import',
  targetType: 'importing',
  existingSectionId: null,
  targetImportIndex: 1,
};

const dmpFile: DmpFile = { version: 1, sections: [] };
const noConfirmed = new Map<number, ConfirmedOverlapInfo>();

/** The target section's original shots: ids 0..2 mirror the DMP ordering, with
 *  lengths distinct enough to tell a correct pairing from a shifted one. */
const targetOriginals = (): EditableShot[] => [
  measured(0, 20),
  measured(1, 22),
  measured(2, 24),
];

/** The import shots that pair with the overlap (ids 0,1), lengths 10 and 12. */
const importShots = (): EditableShot[] => [measured(0, 10), measured(1, 12), measured(2, 14)];

describe('applyAllOverlapTreatments target-import row resolution', () => {
  it('treats the correct originals when a blank row is inserted before the overlap', () => {
    const sectionEdits = new Map<number, SectionEditState>([
      [0, edits(importShots())],
      [1, edits([blankNew(-1), ...targetOriginals()])],
    ]);

    applyAllOverlapTreatments(
      [suggestion],
      noConfirmed,
      sectionEdits,
      dmpFile,
      'interpolate',
    );

    const target = sectionEdits.get(1)!.shots;
    // The inserted blank row is still an invitation - untreated.
    expect(target[0]!.isNew).toBe(true);
    expect(Number.isNaN(target[0]!.length)).toBe(true);
    // Overlap rows interpolate with their OWN import partner: id0 with import
    // id0 (avg 15), id1 with import id1 (avg 17). A positional index would pair
    // id0 with import id1 (16) and leave id1 untreated (22).
    const byId = (id: number) => target.find((s) => s.isNew !== true && s.id === id)!;
    expect(byId(0).length).toBe(15);
    expect(byId(1).length).toBe(17);
    // Shot past the overlap is untouched.
    expect(byId(2).length).toBe(24);
  });

  it('leaves a filled-in inserted row alone and still treats the originals', () => {
    const inserted = measured(-5, 99, { tempId: 5, isNew: true, azimuth_in: 45, azimuth_out: 45 });
    const sectionEdits = new Map<number, SectionEditState>([
      [0, edits(importShots())],
      [1, edits([inserted, ...targetOriginals()])],
    ]);

    applyAllOverlapTreatments(
      [suggestion],
      noConfirmed,
      sectionEdits,
      dmpFile,
      'interpolate',
    );

    const target = sectionEdits.get(1)!.shots;
    // The inserted new row keeps its own geometry - a positional index would
    // have interpolated it (avg of 99 and import id0's 10 = 54.5).
    expect(target[0]!.isNew).toBe(true);
    expect(target[0]!.length).toBe(99);
    const byId = (id: number) => target.find((s) => s.isNew !== true && s.id === id)!;
    expect(byId(0).length).toBe(15);
    expect(byId(1).length).toBe(17);
    expect(byId(2).length).toBe(24);
  });

  it('still treats the correct row on an unedited target (no inserts)', () => {
    const sectionEdits = new Map<number, SectionEditState>([
      [0, edits(importShots())],
      [1, edits(targetOriginals())],
    ]);

    applyAllOverlapTreatments(
      [suggestion],
      noConfirmed,
      sectionEdits,
      dmpFile,
      'interpolate',
    );

    const target = sectionEdits.get(1)!.shots;
    expect(target[0]!.length).toBe(15);
    expect(target[1]!.length).toBe(17);
    expect(target[2]!.length).toBe(24);
  });
});
