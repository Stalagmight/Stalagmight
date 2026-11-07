/**
 * [#576 NF-W7F-1] Edit Shot and Edit Section show a reading as it was entered.
 *
 * The store's shots come from the binary envelope, which carries every reading
 * as an f32: a length of 1.2 arrives as 1.2000000476837158. Edit Shot seeded its
 * inputs with `toString()` and showed those digits; Edit Section's grid editor
 * seeded with twelve fraction digits (1.200000047684). Each now shows the
 * shortest decimal that is the same f32, and a field re-typed as the same f32
 * value (1.20 for 1.2) counts as unchanged, like one left alone - so it is not
 * sent and does not overwrite the backend's exact value.
 *
 * `.tsx` so vitest runs it under jsdom - the modules under test are components.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

vi.mock('../../../utils/telemetry', async () => (await import('../../../test/factories/telemetry')).telemetryWith());
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import EditShotDialog from '../EditShotDialog';
import { editedShotUpdates } from '../EditSectionDialog';
import { SpreadsheetGrid } from '../../common/SpreadsheetGrid/SpreadsheetGrid';
import type { GridColumn } from '../../common/SpreadsheetGrid/types';
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
  pitch_out: null,
  left: f32(0.7),
  right: null,
  up: f32(0.2),
  down: f32(2.9),
  comment: null,
  kind: 'Normal',
  marker_index: null,
  temperature: null,
};

const PROJECT = {
  sections: [{ id: SECTION_ID, name: 'CaveA', shots: [SHOT] }],
} as unknown as Project;

let updateShot: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  updateShot = vi.fn(async (): Promise<CommandResult> => ({ success: true }) as CommandResult);
  closeDialog = vi.fn();
  useProjectStore.setState({ project: PROJECT, updateShot, isShotFrozen: () => false, frozenShots: [] });
  useUIStore.setState({
    activeDialog: 'edit-shot',
    closeDialog,
    dialogData: { sectionId: SECTION_ID, shotId: SHOT_ID },
  });
});

const value = (testId: string) => (screen.getByTestId(testId) as HTMLInputElement).value;

describe('Edit Shot shows each reading as entered (#576 NF-W7F-1)', () => {
  it('seeds every input with the shortest decimal of its f32', () => {
    render(<EditShotDialog />);
    const shown = {
      length: value('edit-shot-length-input'),
      azimuthIn: value('edit-shot-azimuth-in-input'),
      azimuthOut: value('edit-shot-azimuth-out-input'),
      depthIn: value('edit-shot-depth-in-input'),
      depthOut: value('edit-shot-depth-out-input'),
      pitchIn: value('edit-shot-pitch-in-input'),
      pitchOut: value('edit-shot-pitch-out-input'),
      left: value('edit-shot-left-input'),
      right: value('edit-shot-right-input'),
      up: value('edit-shot-up-input'),
      down: value('edit-shot-down-input'),
    };
    console.log('[NF-W7F-1] Edit Shot inputs:', JSON.stringify(shown));
    expect(shown).toEqual({
      length: '1.2',
      azimuthIn: '123.4',
      azimuthOut: '303.3',
      depthIn: '10.1',
      depthOut: '11.7',
      pitchIn: '-3.3',
      pitchOut: '',
      left: '0.7',
      right: '',
      up: '0.2',
      down: '2.9',
    });
  });

  it('a field re-typed as the same f32 value is not sent', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-length-input'), { target: { value: '1.20' } });
    fireEvent.change(screen.getByTestId('edit-shot-depth-in-input'), { target: { value: '10.10000' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    console.log('[NF-W7F-1] update_shot calls after re-typing the same values:', updateShot.mock.calls.length);
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('a real change is still sent, alone', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-length-input'), { target: { value: '1.25' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(updateShot.mock.calls[0]![2]).toEqual({ length: 1.25 });
  });

  // [#576 RV8b-2] By design (shotUpdatesDiff.ts, the `f32` compare): the store holds
  // f32, so a value that differs from it only beyond f32 precision is the same
  // reading, and is not sent; a difference at the 7th significant digit is.
  it('a value that differs only beyond f32 precision is not a change', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-depth-in-input'), { target: { value: '10.1000001' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(updateShot).not.toHaveBeenCalled();
  });

  it('a value that differs at the 7th significant digit is', async () => {
    render(<EditShotDialog />);
    fireEvent.change(screen.getByTestId('edit-shot-depth-in-input'), { target: { value: '10.10001' } });
    fireEvent.click(screen.getByTestId('edit-shot-save'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(updateShot.mock.calls[0]![2]).toEqual({ depth_in: 10.10001 });
  });
});

describe('Edit Section (#576 NF-W7F-1)', () => {
  it('a row re-typed to the same f32 values sends no update', () => {
    const typed = { ...SHOT, length: 1.2, depth_in: 10.1, azimuth_in: 123.4 };
    const patch = editedShotUpdates(SHOT, typed);
    console.log('[NF-W7F-1] Edit Section patch for a re-typed row:', JSON.stringify(patch));
    expect(patch).toEqual({});
    expect(editedShotUpdates(SHOT, { ...SHOT, length: 1.25 })).toEqual({ length: 1.25 });
  });

  it("the grid's cell editor opens on the shortest decimal of an f32 reading", () => {
    interface Row { id: number; length: number }
    const columns: GridColumn<Row>[] = [
      { key: 'length', header: 'Len', kind: 'number', getValue: (r) => r.length, editable: true },
    ];
    render(
      <SpreadsheetGrid<Row>
        columns={columns}
        rows={[{ id: 1, length: f32(1.2) }, { id: 2, length: 1.23456789 }]}
        rowKey={(r) => r.id}
        onEdit={vi.fn()}
        height={200}
      />,
    );
    fireEvent.doubleClick(screen.getByTestId('grid-cell-0-length'));
    const f32Seed = (screen.getByTestId('grid-cell-editor') as HTMLInputElement).value;
    fireEvent.keyDown(screen.getByTestId('grid-cell-editor'), { key: 'Escape' });
    fireEvent.doubleClick(screen.getByTestId('grid-cell-1-length'));
    const f64Seed = (screen.getByTestId('grid-cell-editor') as HTMLInputElement).value;
    console.log(`[NF-W7F-1] grid editor seeds: f32 1.2 -> "${f32Seed}", f64 1.23456789 -> "${f64Seed}"`);
    expect(f32Seed).toBe('1.2');
    // A value that is not an f32 (computed in f64) keeps its full precision.
    expect(f64Seed).toBe('1.23456789');
  });
});
