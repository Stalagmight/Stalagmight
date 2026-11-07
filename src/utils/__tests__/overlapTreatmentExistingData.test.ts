/**
 * #576 NF-W7F-2: a DMP row that takes an EXISTING section's readings ("existing
 * data" overlap mode) is written with the readings the user sees, not with
 * their f32 rounding.
 *
 * The project the frontend holds is decoded from the binary envelope, which
 * carries every shot reading as an f32: a length the backend stores as 1.2
 * reaches the store as 1.2000000476837158. The import-side treatment copied
 * those store values into the new import rows, so the new section was created
 * with the rounding noise in every overlap reading. The treatment now takes the
 * shortest decimal that is the same f32 - the value the backend stored, for any
 * reading of up to six significant digits.
 *
 * All data is synthetic.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { applyAllOverlapTreatments } from '../overlapTreatment';
import type { SectionEditState } from '../batchImportHelpers';
import type { CrossSectionSuggestion, DmpFile } from '../../types/commands';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { Project } from '../../types/project';
import { useProjectStore } from '../../store/projectStore';

/** The readings the backend stores for the existing shot (exact f64). */
const STORED = {
  azimuth_in: 12.3,
  azimuth_out: 14.7,
  length: 1.2,
  depth_in: 3.7,
  depth_out: 4.1,
  pitch_in: null,
  pitch_out: null,
  left: 0.3,
  right: 0.6,
  up: 1.1,
  down: 2.2,
};

/** The same readings as the store holds them: each one through an f32. */
const asStoreHoldsThem = Object.fromEntries(
  Object.entries(STORED).map(([k, v]) => [k, v == null ? v : Math.fround(v)]),
) as typeof STORED;

const importRow = (id: number): EditableShot =>
  ({
    id,
    azimuth_in: 200,
    azimuth_out: 200,
    length: 9,
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

const edits = (shots: EditableShot[]): SectionEditState => ({
  name: 'CaveA import',
  surveyor: '',
  date: '',
  color: '',
  direction: 'in',
  quality: 1,
  reverse: false,
  shots,
});

/** Import section 0 overlapping existing section 5 by one shot, front. Not
 *  confirmed by the user, so the import side keeps the row and takes the
 *  existing section's readings (a confirmed one would delete it). */
const suggestion = (direction: 'forward' | 'reverse'): CrossSectionSuggestion => ({
  importSectionIndex: 0,
  importStationIndex: 0,
  existingStationIndex: direction === 'forward' ? 0 : 1,
  matchedShots: 1,
  existingMatchedShots: 1,
  preferredMergeOffset: 0,
  score: 100,
  direction,
  isPartial: false,
  confidence: 'high',
  existingSectionName: 'StationA section',
  importSectionName: 'CaveA import',
  targetType: 'existing',
  existingSectionId: 5,
  targetImportIndex: null,
});

const dmpFile = { version: 5, sections: [] } as unknown as DmpFile;

beforeEach(() => {
  useProjectStore.setState({
    project: {
      sections: [{ id: 5, name: 'StationA section', shots: [{ id: 50, kind: 'Normal', comment: null, ...asStoreHoldsThem }] }],
      station_merges: [],
      zero_datums: [],
      loop_closures: [],
    } as unknown as Project,
  });
});

describe('overlap treatment, existing data: the readings the backend stored (#576 NF-W7F-2)', () => {
  it('writes the stored readings into the import row, not their f32 rounding', () => {
    // Sanity: the store really holds noise for these.
    expect(asStoreHoldsThem.length).not.toBe(1.2);

    const sectionEdits = new Map([[0, edits([importRow(0), importRow(1)])]]);
    applyAllOverlapTreatments([suggestion('forward')], new Map(), sectionEdits, dmpFile, 'existing_data');

    const row = sectionEdits.get(0)!.shots[0]!;
    const { azimuth_in, azimuth_out, length, depth_in, depth_out, pitch_in, pitch_out, left, right, up, down } = row;
    expect({ azimuth_in, azimuth_out, length, depth_in, depth_out, pitch_in, pitch_out, left, right, up, down }).toEqual(STORED);
    // The row past the overlap is the import's own.
    expect(sectionEdits.get(0)!.shots[1]!.length).toBe(9);
  });

  it('reverses the stored readings, not the rounded ones, when the overlap runs the other way', () => {
    const sectionEdits = new Map([[0, edits([importRow(0)])]]);
    applyAllOverlapTreatments([suggestion('reverse')], new Map(), sectionEdits, dmpFile, 'existing_data');

    const row = sectionEdits.get(0)!.shots[0]!;
    // reverseTargetShot: azimuths turned 180 and swapped, depths swapped, L/R swapped.
    expect([row.azimuth_in, row.azimuth_out, row.length, row.depth_in, row.depth_out, row.left, row.right]).toEqual([
      194.7, 192.3, 1.2, 4.1, 3.7, 0.6, 0.3,
    ]);
  });

  it('leaves a reading the store holds at full precision alone', () => {
    // Not an f32 value, so it did not come through the envelope's rounding:
    // there is nothing to undo.
    const precise = 1.23456789012;
    useProjectStore.setState({
      project: {
        sections: [{ id: 5, name: 'StationA section', shots: [{ id: 50, kind: 'Normal', comment: null, ...STORED, length: precise }] }],
        station_merges: [],
        zero_datums: [],
        loop_closures: [],
      } as unknown as Project,
    });
    const sectionEdits = new Map([[0, edits([importRow(0)])]]);
    applyAllOverlapTreatments([suggestion('forward')], new Map(), sectionEdits, dmpFile, 'existing_data');

    expect(sectionEdits.get(0)!.shots[0]!.length).toBe(precise);
  });
});
