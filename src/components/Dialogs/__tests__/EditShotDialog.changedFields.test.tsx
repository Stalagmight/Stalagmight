/**
 * [#576 F32-EDIT] Edit Shot sends only the fields the user changed.
 *
 * The store's shots come from the binary envelope, which carries every reading
 * as an f32: a length of 1.2 arrives as 1.2000000476837158. The dialog used to
 * re-send all fifteen fields on Save, so editing one wrote the rounded value of
 * every other one back over the backend's exact copy - and the noise was saved.
 * A Save with nothing changed did the same to the whole shot.
 *
 * `.tsx` so vitest runs it under jsdom - the module under test is a component.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryWith());

import EditShotDialog from '../EditShotDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { Project, SectionId, Shot, ShotId } from '../../../types/project';
import type { CommandResult } from '../../../types/commands';

const SECTION_ID = 1 as SectionId;
const SHOT_ID = 7 as ShotId;

/** Every reading as the binary envelope delivers it: rounded through an f32. */
const f32 = (v: number) => Math.fround(v);
const SHOT: Shot = {
  id: SHOT_ID,
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
};

const PROJECT = {
  sections: [{ id: SECTION_ID, name: 'CaveA', shots: [SHOT] }],
} as unknown as Project;

let updateShot: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateShot = vi.fn(async (): Promise<CommandResult> => ({ success: true }) as CommandResult);
  closeDialog = vi.fn();
  useProjectStore.setState({
    project: PROJECT,
    updateShot,
    isShotFrozen: () => false,
    frozenShots: [],
  });
  useUIStore.setState({
    activeDialog: 'edit-shot',
    closeDialog,
    dialogData: { sectionId: SECTION_ID, shotId: SHOT_ID },
  });
});

describe('EditShotDialog - a Save sends only the changed fields', () => {
  it('editing the length sends the length and nothing else', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-length-input'), { target: { value: '5' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(updateShot).toHaveBeenCalledTimes(1);
    const [sectionId, shotId, updates] = updateShot.mock.calls[0]!;
    console.log('update_shot patch:', JSON.stringify(updates));
    expect(sectionId).toBe(SECTION_ID);
    expect(shotId).toBe(SHOT_ID);
    expect(updates).toEqual({ length: 5 });
  });

  it('clearing a splay and editing the comment sends exactly those two', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-left-input'), { target: { value: '' } });
    fireEvent.change(screen.getByTestId('edit-shot-comment-input'), { target: { value: '  ' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(updateShot.mock.calls[0]![2]).toEqual({ left: null, comment: null });
  });

  it('a Save with nothing changed sends no command and closes', async () => {
    render(<EditShotDialog />);
    fireEvent.click(screen.getByTestId('edit-shot-save'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    console.log('update_shot calls on an unchanged Save:', updateShot.mock.calls.length);
    expect(updateShot).not.toHaveBeenCalled();
  });
});
