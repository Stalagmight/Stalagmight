/**
 * Demo issue I-4: Merge must say what it does before it does it. The Merge
 * dialog names the piece that moves, how far and onto which station, and, when
 * the two stations' depths differ by more than the backend's threshold, the
 * depth the joined station takes - all from the backend's `merge_preview` on
 * the merge check. Before, the dialog said nothing: a free piece jumped 24 m
 * and depths 15.0 m and 4.0 m silently became 9.5 m. Injection: drop
 * `MergePreviewNotice` from the dialog and every test here fails.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { MergeDatumShift } from '../../../types/ipc/MergeDatumShift';
import type { MergePreview } from '../../../types/ipc/MergePreview';

let dialogData: Record<string, unknown> = {};
const uiState = () => ({ closeDialog: vi.fn(), dialogData, clearMergeStations: vi.fn() });
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign(() => uiState(), { getState: () => uiState() }),
}));

const projectState = {
  mergeStations: vi.fn(),
  executeBatch: vi.fn(),
  project: {
    lines: [
      { id: 1, name: 'Upstream' },
      { id: 2, name: 'Second entrance' },
      { id: 3, name: 'Side passage' },
      { id: 4, name: 'Sump' },
      { id: 5, name: 'Chimney' },
      { id: 6, name: 'Far room' },
    ],
  },
  // Names that differ from the positional indices (both stations sit at index 5).
  stations: [
    { station_ref: { line_id: 1, station_index: 5 }, name: '17' },
    { station_ref: { line_id: 2, station_index: 5 }, name: 'B4' },
  ],
  surveyData: null,
};
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: Object.assign(
    (selector?: (s: typeof projectState) => unknown) => (selector ? selector(projectState) : projectState),
    { getState: () => projectState }
  ),
}));

vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children }: { children: React.ReactNode }) => <div>{children}</div>,
}));

import '../../../i18n';
import MergeDialog from '../MergeDialog';

const A = { line_id: 1, station_index: 5 };
const B = { line_id: 2, station_index: 5 };
const preview = (extra: Partial<MergePreview> = {}): MergePreview => ({
  moving_line_ids: [1],
  shift_m: 24.31,
  joins: B,
  ...extra,
});

describe('MergeDialog says what the merge does (I-4)', () => {
  beforeEach(() => {
    dialogData = { stationA: A, stationB: B };
  });

  it('names the piece that moves, how far, and the station it joins', () => {
    dialogData = { ...dialogData, mergePreview: preview() };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-move')).toHaveTextContent('Upstream moves 24 m to join Second entrance / B4.');
    expect(screen.queryByTestId('merge-preview-depths')).not.toBeInTheDocument();
  });

  it('says the depths when the two stations disagree', () => {
    dialogData = { ...dialogData, mergePreview: preview({ depths: { a_m: 15, b_m: 4, joined_m: 9.5 } }) };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent(
      'Depths 15.0 m and 4.0 m become 9.5 m: the joined station takes the mean of its depth readings.'
    );
  });

  // N-13: "Upstream and 1 more line move 24 m" did not say which line; the
  // lines are named, up to three, then counted. Injection: listing only the
  // first name and a count fails both.
  it('names the other lines of a moving piece', () => {
    dialogData = { ...dialogData, mergePreview: preview({ moving_line_ids: [1, 3], shift_m: 4.26 }) };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-move')).toHaveTextContent(
      'Upstream and Side passage move 4.3 m to join Second entrance / B4.'
    );
  });

  it('names three lines, then counts the rest', () => {
    dialogData = { ...dialogData, mergePreview: preview({ moving_line_ids: [1, 3, 4], shift_m: 4.26 }) };
    const { unmount } = render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-move')).toHaveTextContent(
      'Upstream, Side passage and Sump move 4.3 m'
    );
    unmount();
    dialogData = { ...dialogData, mergePreview: preview({ moving_line_ids: [1, 3, 4, 5, 6], shift_m: 4.26 }) };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-move')).toHaveTextContent(
      'Upstream, Side passage, Sump and 2 more move 4.3 m'
    );
  });

  it('says no move for stations already on one point', () => {
    dialogData = { ...dialogData, mergePreview: preview({ shift_m: 0.001 }) };
    render(<MergeDialog />);
    expect(screen.queryByTestId('merge-preview-move')).not.toBeInTheDocument();
  });

  it('leaves the move to the datum notice when both pieces are placed by GPS', () => {
    const datumShift: MergeDatumShift = {
      moving_line_ids: [2],
      shift_m: 12.047,
      datum_station: { line_id: 2, station_index: 0 },
    };
    dialogData = {
      ...dialogData,
      datumShift,
      mergePreview: preview({ moving_line_ids: [2], joins: A, depths: { a_m: 3, b_m: 5, joined_m: 4 } }),
    };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-datum-shift')).toBeInTheDocument();
    expect(screen.queryByTestId('merge-preview-move')).not.toBeInTheDocument();
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent('Depths 3.0 m and 5.0 m become 4.0 m');
  });

  it('gives no distance after a snap-to-merge drag, which moved the piece first', () => {
    dialogData = { ...dialogData, mergePreview: preview({ depths: { a_m: 15, b_m: 4, joined_m: 9.5 } }), preCommands: [] };
    render(<MergeDialog />);
    expect(screen.queryByTestId('merge-preview-move')).not.toBeInTheDocument();
    expect(screen.getByTestId('merge-preview-depths')).toBeInTheDocument();
  });
});
