/**
 * #741: merging two pieces that are each placed by their own GPS moves one of
 * them. The Merge dialog says which, by how much, and that its datum becomes a
 * GPS fix - all from the backend's `datum_shift` on the merge check.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { MergeDatumShift } from '../../../types/ipc/MergeDatumShift';

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
const shift = (extra: Partial<MergeDatumShift> = {}): MergeDatumShift => ({
  moving_line_ids: [2],
  shift_m: 12.047,
  datum_station: { line_id: 2, station_index: 0 },
  ...extra,
});

describe('MergeDialog datum shift (#741)', () => {
  beforeEach(() => {
    dialogData = { stationA: A, stationB: B };
  });

  it('names both stations with their lines, never the positional handles', () => {
    render(<MergeDialog />);
    // Full highlighting: line pills, the line as the muted context.
    expect(document.body.textContent).toContain('Upstream: 17');
    expect(document.body.textContent).toContain('Second entrance: B4');
    expect(document.body.textContent).not.toMatch(/\d+:\d+|Line \d/);
  });

  it('says nothing about datums when the merge moves no placed piece', () => {
    render(<MergeDialog />);
    expect(screen.queryByTestId('merge-datum-shift')).not.toBeInTheDocument();
  });

  it('names the piece that moves, how far, and the datum that becomes a GPS fix', () => {
    dialogData = { ...dialogData, datumShift: shift() };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-datum-shift')).toHaveTextContent(
      'Second entrance is placed by its own GPS. It moves 12 m to meet the other piece, and its zero datum at Second entrance / 0 becomes a GPS fix.'
    );
  });

  it('names the other lines of a moving piece (N-13)', () => {
    dialogData = { ...dialogData, datumShift: shift({ moving_line_ids: [2, 3], shift_m: 4.26 }) };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-datum-shift')).toHaveTextContent(
      'Second entrance and Side passage are placed by their own GPS. They move 4.3 m'
    );
  });

  it('gives no distance after a snap-to-merge drag, which moved stations first', () => {
    dialogData = { ...dialogData, datumShift: shift(), preCommands: [] };
    render(<MergeDialog />);
    const notice = screen.getByTestId('merge-datum-shift');
    expect(notice).toHaveTextContent('The merge moves it to meet the other piece');
    expect(notice).not.toHaveTextContent('12 m');
  });
});
