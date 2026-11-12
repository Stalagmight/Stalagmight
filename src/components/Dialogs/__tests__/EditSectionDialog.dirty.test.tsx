/**
 * When the Edit Section dialog counts as "dirty".
 *
 * Save skips an `isNew` row that was materialized and then cleared back to blank
 * (the `isBlankNewShot` / `plannedShotInserts` predicate emits nothing for it),
 * so closing the dialog in that state must NOT prompt "discard changes?" - there
 * is nothing to discard. `isDirty` used to treat every `isNew` row as a pending
 * change, so the prompt fired over a row Save would have written nothing for.
 *
 * A real (filled-in) new row, a soft-deleted existing row, an edited existing
 * row, and changed metadata all still count.
 *
 * `.tsx` so vitest runs it under jsdom - the module under test is a component.
 */
import { describe, it, expect } from 'vitest';

import { isEditSectionDirty, type EditSectionDirtyState } from '../EditSectionDialog';
import { blankEditableShot } from '../../common/editableShot';
import type { EditableShot } from '../../common/editableShot';
import type { Section, Shot } from '../../../types/project';

const shotBase = (id: number, length: number) => ({
  id,
  azimuth_in: 90,
  azimuth_out: 90,
  length,
  depth_in: 1,
  depth_out: 2,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal' as const,
});

/** A saved shot as the section holds it. */
const savedShot = (id: number, length = 10): Shot => ({
  ...shotBase(id, length),
  marker_index: null,
  temperature: null,
});

/** The same shot as the dialog's local editable copy (no isNew flag). */
const editableSaved = (id: number, length = 10): EditableShot => ({ ...shotBase(id, length) });

/** A new row the user actually filled in. */
const filledNew = (tempId: number): EditableShot => ({
  ...shotBase(tempId, 7),
  tempId,
  isNew: true,
});

const section = (shots: Shot[], over: Partial<Section> = {}): Section => ({
  id: 1,
  name: null,
  color: '#ffffff',
  direction: 'in',
  quality: 1,
  shots,
  next_shot_id: 100,
  reverse_parity: false,
  gauge_shot_uuid: null,
  gauge_neighbour_uuid: null,
  ...over,
});

/** The form state that mirrors `section`'s saved values, i.e. nothing edited. */
const cleanState = (
  shots: EditableShot[],
  over: Partial<EditSectionDirtyState> = {},
): EditSectionDirtyState => ({
  name: '',
  creditChanged: false,
  color: '#ffffff',
  direction: 'in',
  quality: 1,
  shots,
  ...over,
});

describe('isEditSectionDirty', () => {
  it('is clean when nothing has changed', () => {
    const sec = section([savedShot(0), savedShot(1)]);
    expect(isEditSectionDirty(cleanState([editableSaved(0), editableSaved(1)]), sec)).toBe(false);
  });

  it('stays clean when a new row was materialized then cleared back to blank', () => {
    // The blank invitation row Save writes nothing for is not a pending change.
    const sec = section([savedShot(0)]);
    const state = cleanState([editableSaved(0), blankEditableShot(-1)]);
    expect(isEditSectionDirty(state, sec)).toBe(false);
  });

  it('is dirty once that new row holds a real measurement', () => {
    const sec = section([savedShot(0)]);
    const state = cleanState([editableSaved(0), filledNew(-1)]);
    expect(isEditSectionDirty(state, sec)).toBe(true);
  });

  it('is dirty when an existing row is soft-deleted', () => {
    const sec = section([savedShot(0), savedShot(1)]);
    const state = cleanState([editableSaved(0), { ...editableSaved(1), isDeleted: true }]);
    expect(isEditSectionDirty(state, sec)).toBe(true);
  });

  it('is dirty when an existing row was edited', () => {
    const sec = section([savedShot(0)]);
    const state = cleanState([{ ...editableSaved(0), length: 999 }]);
    expect(isEditSectionDirty(state, sec)).toBe(true);
  });

  it('is dirty when the survey credit changed (or a name is still being typed)', () => {
    const sec = section([savedShot(0)]);
    expect(isEditSectionDirty(cleanState([editableSaved(0)], { creditChanged: true }), sec)).toBe(true);
  });

  it('is dirty when metadata changed, even over a blank new row', () => {
    const sec = section([savedShot(0)]);
    const state = cleanState([editableSaved(0), blankEditableShot(-1)], { name: 'Renamed' });
    expect(isEditSectionDirty(state, sec)).toBe(true);
  });
});
