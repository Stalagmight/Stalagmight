/**
 * [#576 F32-EDIT] The edit paths that rewrite a saved shot from a whole row -
 * Edit Section's Save, the DMP wizard's overlap treatment of an existing section
 * (and, through the same helper, orphan placement) - send only the readings that
 * change. The store holds every reading rounded through an f32 (the binary
 * envelope); re-sending an unchanged one wrote that rounding over the backend's
 * exact value.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { changedShotFields } from '../shotUpdatesDiff';
import { editedShotUpdates } from '../../components/Dialogs/EditSectionDialog';
import { applyAllOverlapTreatments } from '../overlapTreatment';
import { useProjectStore } from '../../store/projectStore';
import type { ConfirmedOverlapInfo, SectionEditState } from '../batchImportHelpers';
import type { CrossSectionSuggestion, DmpFile } from '../../types/commands';
import type { EditableShot } from '../../components/common/ShotTableEditor';
import type { Project, Shot } from '../../types/project';
import { makeDmpFile } from '../../test/factories/dmp';

const f32 = (v: number) => Math.fround(v);

/** A saved shot as the store holds it after a binary load or refresh. */
const storeShot = (id: number): Shot => ({
  id,
  azimuth_in: f32(123.4),
  azimuth_out: f32(303.3),
  length: f32(1.2),
  depth_in: f32(10.1),
  depth_out: f32(11.7),
  pitch_in: f32(-3.3),
  pitch_out: f32(3.1),
  left: f32(0.7),
  right: f32(1.3),
  up: f32(0.2),
  down: f32(2.9),
  comment: 'StationA1 lead',
  kind: 'Normal',
  marker_index: 4,
  temperature: f32(21.3),
});

/** The same shot as an editor's local copy (no device readings). */
const editable = (shot: Shot): EditableShot => {
  const { marker_index: _m, temperature: _t, ...rest } = shot;
  return rest;
};

describe('changedShotFields', () => {
  it('keeps only the entries that differ, treating unset and null alike', () => {
    expect(
      changedShotFields(
        { length: 2, azimuth_in: NaN, comment: null },
        { length: 2, left: null, azimuth_in: NaN, comment: null, right: 3 },
      ),
    ).toEqual({ right: 3 });
  });

  it('sends a clear (null) of a recorded reading', () => {
    expect(changedShotFields({ up: 1.5 }, { up: null })).toEqual({ up: null });
  });
});

describe('Edit Section - one edited row', () => {
  it('sends the edited reading and none of the untouched ones', () => {
    const original = storeShot(3);
    const patch = editedShotUpdates(original, { ...editable(original), depth_out: 12 });
    console.log('Edit Section update_shot patch:', JSON.stringify(patch));
    expect(patch).toEqual({ depth_out: 12 });
  });

  it('never sends the device readings it has no column for', () => {
    const original = storeShot(3);
    const patch = editedShotUpdates(original, { ...editable(original), kind: 'Jump' });
    expect(patch).toEqual({ kind: 'Jump' });
  });
});

describe('DMP wizard - overlap treatment of an existing section', () => {
  const EXISTING_ID = 5;
  const project = { sections: [{ id: EXISTING_ID, shots: [storeShot(0), storeShot(1)] }] } as unknown as Project;

  /** The import section: same readings as the existing overlap, except length. */
  const importShots = (): EditableShot[] =>
    [0, 1, 2].map((id) => ({ ...editable(storeShot(id)), length: 2 + id }));
  const sectionEdits = () =>
    new Map<number, SectionEditState>([
      [0, { name: 'S', surveyor: '', date: '', color: '', direction: 'in', quality: 1, reverse: false, shots: importShots() }],
    ]);
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
    existingSectionName: 'CaveA',
    importSectionName: 'import',
    targetType: 'existing',
    existingSectionId: EXISTING_ID,
    targetImportIndex: null,
  };
  const confirmed = new Map<number, ConfirmedOverlapInfo>([
    [
      0,
      {
        importStationIndex: 0,
        matchedShots: 2,
        existingMatchedShots: 2,
        direction: 'forward',
        existingStationIndex: 0,
        targetType: 'existing',
        existingSectionId: EXISTING_ID,
        targetImportIndex: null,
        mergeImportStation: 2,
      },
    ],
  ]);
  const dmpFile: DmpFile = makeDmpFile();

  beforeEach(() => {
    useProjectStore.setState({ project });
  });

  it('import_data sends only the readings the import changes', () => {
    const updates = applyAllOverlapTreatments([suggestion], confirmed, sectionEdits(), dmpFile, 'import_data');
    console.log('existing-shot updates:', JSON.stringify(updates.map((u) => u.updates)));
    expect(updates.map((u) => [u.shot_id, u.updates])).toEqual([
      [0, { length: 2 }],
      [1, { length: 3 }],
    ]);
  });

  it('interpolate sends only the readings the blend changes', () => {
    const updates = applyAllOverlapTreatments([suggestion], confirmed, sectionEdits(), dmpFile, 'interpolate');
    expect(updates).toHaveLength(2);
    for (const { shot_id, updates: u } of updates) {
      // Linear readings equal on both sides average to themselves and stay out.
      expect(Object.keys(u)).not.toContain('depth_in');
      expect(Object.keys(u)).not.toContain('left');
      expect(u.length).toBeCloseTo((f32(1.2) + (shot_id === 0 ? 2 : 3)) / 2, 12);
    }
  });
});
