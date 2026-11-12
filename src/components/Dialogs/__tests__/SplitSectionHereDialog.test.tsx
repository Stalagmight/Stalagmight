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
    dialogData = { sectionId: 3, at: { type: 'station', station_index: 2 }, label: '3:2' };
  });

  it('defaults the names and colours from the section', () => {
    render(<SplitSectionHereDialog />);
    expect(screen.getByTestId('split-here-name-a')).toHaveValue('Main-A');
    expect(screen.getByTestId('split-here-name-b')).toHaveValue('Main-B');
    expect(screen.getByTestId('split-here-color-a')).toHaveValue('#123456');
    expect(screen.getByTestId('split-here-summary')).toHaveTextContent('Splitting Main at station 3:2');
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
});
