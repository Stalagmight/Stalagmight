/**
 * #335: the "Split Section Here" dialog.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';

const closeDialog = vi.fn();
const clearSelection = vi.fn();
const splitSection = vi.fn();

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog, clearSelection, dialogData });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

const shot = (id: number) => ({ id, length: 10 });
let stations: Array<Record<string, unknown>> = [];
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    splitSection,
    config: null,
    stations,
    project: {
      sections: [{ id: 3, name: 'Main', color: '#123456', shots: [shot(31), shot(32), shot(33), shot(34)] }],
    },
  }),
}));

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  isEditingBlockedNow: () => blocked,
}));

vi.mock('../../../utils/telemetry', () => ({ trackDialogComplete: vi.fn() }));

const previewSplitSection = vi.fn();
vi.mock('../../../utils/splitSection', () => ({
  previewSplitSection: (...args: unknown[]) => previewSplitSection(...args),
}));
const NO_MOVE = { closures_to_merges: 0, moved_side: null, moved_m: 0 };

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import SplitSectionHereDialog from '../SplitSectionHereDialog';

const station = (index: number, extra: Record<string, unknown>) => ({
  station_ref: { section_id: 3, station_index: index },
  is_zero_datum: false,
  ...extra,
});

describe('SplitSectionHereDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    stations = [];
    splitSection.mockResolvedValue({ success: true });
    previewSplitSection.mockResolvedValue(NO_MOVE);
    dialogData = { sectionId: 3, at: { type: 'station', station_index: 2 }, label: '3:2' };
  });

  it('defaults the names and colours from the section', () => {
    render(<SplitSectionHereDialog />);
    expect(screen.getByTestId('split-here-name-a')).toHaveValue('Main-A');
    expect(screen.getByTestId('split-here-name-b')).toHaveValue('Main-B');
    expect(screen.getByTestId('split-here-color-a')).toHaveValue('#123456');
    expect(screen.getByTestId('split-here-summary')).toHaveTextContent('Splitting Main at station 3:2');
    expect(screen.getByTestId('split-here-credit-note')).toHaveTextContent(
      'Each shot keeps its own surveyors, explorers and survey date.'
    );
  });

  it('sends a station split and clears the selection', async () => {
    render(<SplitSectionHereDialog />);
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(splitSection).toHaveBeenCalledWith(3, { type: 'station', station_index: 2 }, {
      name_a: 'Main-A',
      name_b: 'Main-B',
      color_a: '#123456',
      color_b: '#123456',
      comment_side: 'a',
      datum_side: 'a',
    });
    expect(clearSelection).toHaveBeenCalled();
  });

  it('sends a shot split with its distance', async () => {
    dialogData = { sectionId: 3, at: { type: 'shot', shot_id: 32, distance: 4 }, label: '3:2' };
    render(<SplitSectionHereDialog />);
    expect(screen.getByTestId('split-here-summary')).toHaveTextContent('(40%) along shot 3:2');
    fireEvent.change(screen.getByTestId('split-here-name-b'), { target: { value: 'Tail' } });
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitSection).toHaveBeenCalled());
    expect(splitSection.mock.calls[0]?.[1]).toEqual({ type: 'shot', shot_id: 32, distance: 4 });
    expect(splitSection.mock.calls[0]?.[2]).toMatchObject({ name_b: 'Tail' });
  });

  it('hides the comment and datum choices on a plain station', () => {
    stations = [station(2, {})];
    render(<SplitSectionHereDialog />);
    expect(screen.queryByTestId('split-here-comment-side')).toBeNull();
    expect(screen.queryByTestId('split-here-datum-side')).toBeNull();
  });

  it('offers and sends the comment and datum choices when the station has them', async () => {
    stations = [station(2, { comment: 'restriction', is_zero_datum: true })];
    render(<SplitSectionHereDialog />);
    expect(screen.getByTestId('split-here-comment-side-both')).toBeInTheDocument();
    expect(screen.queryByTestId('split-here-datum-side-both')).toBeNull();
    fireEvent.click(screen.getByTestId('split-here-comment-side-both'));
    fireEvent.click(screen.getByTestId('split-here-datum-side-b'));
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    await waitFor(() => expect(splitSection).toHaveBeenCalled());
    expect(splitSection.mock.calls[0]?.[2]).toMatchObject({ comment_side: 'both', datum_side: 'b' });
  });

  it('hides the choices for a mid-shot cut, which makes a fresh station', () => {
    stations = [station(1, { comment: 'x', is_zero_datum: true }), station(2, { comment: 'y' })];
    dialogData = { sectionId: 3, at: { type: 'shot', shot_id: 32, distance: 5 }, label: '3:2' };
    render(<SplitSectionHereDialog />);
    expect(screen.queryByTestId('split-here-comment-side')).toBeNull();
    expect(screen.queryByTestId('split-here-datum-side')).toBeNull();
  });

  it('refuses to split in a read-only view', () => {
    blocked = true;
    render(<SplitSectionHereDialog />);
    fireEvent.click(screen.getByTestId('split-here-confirm'));
    expect(splitSection).not.toHaveBeenCalled();
    expect(screen.getByTestId('split-here-error')).toHaveTextContent('Editing is disabled');
  });

  it('warns, before the split, that a loop closure across the cut will move a half', async () => {
    previewSplitSection.mockResolvedValue({ closures_to_merges: 1, moved_side: 'b', moved_m: 0.5 });
    render(<SplitSectionHereDialog />);
    const warning = await screen.findByTestId('split-here-closure-warning');
    expect(warning).toHaveTextContent(
      'Main-B is tied to Main-A only by a loop closure, so the split turns it into a merge and moves Main-B'
    );
    expect(warning).toHaveTextContent('0.50');
    expect(previewSplitSection).toHaveBeenCalledWith(
      3,
      { type: 'station', station_index: 2 },
      expect.objectContaining({ datum_side: 'a' })
    );
    // Still the user's call: the split goes ahead.
    expect(screen.getByTestId('split-here-confirm')).toBeEnabled();
  });

  it('names the first half and counts the closures when it is the one that moves', async () => {
    previewSplitSection.mockResolvedValue({ closures_to_merges: 2, moved_side: 'a', moved_m: 1.25 });
    render(<SplitSectionHereDialog />);
    expect(await screen.findByTestId('split-here-closure-warning')).toHaveTextContent(
      'Main-A is tied to Main-B only by 2 loop closures, so the split turns them into merges and moves Main-A'
    );
  });

  it('shows no warning when nothing moves, or the preview is refused', async () => {
    render(<SplitSectionHereDialog />);
    await waitFor(() => expect(previewSplitSection).toHaveBeenCalled());
    expect(screen.queryByTestId('split-here-closure-warning')).toBeNull();

    previewSplitSection.mockRejectedValue('Station 2 is the first or last station');
    render(<SplitSectionHereDialog />);
    await waitFor(() => expect(previewSplitSection).toHaveBeenCalledTimes(2));
    expect(screen.queryByTestId('split-here-closure-warning')).toBeNull();
  });

  it('asks again when the zero datum changes halves, which decides the half that moves', async () => {
    stations = [station(2, { is_zero_datum: true })];
    render(<SplitSectionHereDialog />);
    await waitFor(() => expect(previewSplitSection).toHaveBeenCalledTimes(1));
    fireEvent.click(screen.getByTestId('split-here-datum-side-b'));
    await waitFor(() =>
      expect(previewSplitSection).toHaveBeenLastCalledWith(
        3,
        { type: 'station', station_index: 2 },
        expect.objectContaining({ datum_side: 'b' })
      )
    );
  });
});
