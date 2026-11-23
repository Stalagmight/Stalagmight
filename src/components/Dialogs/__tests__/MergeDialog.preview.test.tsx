/**
 * Demo issue I-4: Merge must say what it does before it does it. The Merge
 * dialog names the piece that moves, how far and onto which station, and, when
 * the two stations' depths differ by more than the backend's threshold, the
 * depth the joined station takes - all from the backend's `merge_preview` on
 * the merge check. Before, the dialog said nothing: a free piece jumped 24 m
 * and depths 15.0 m and 4.0 m silently became 9.5 m. Injection: drop
 * `MergePreviewNotice` from the dialog and every test here fails.
 *
 * Demo finding V-2: onto a station that is already a junction, "Depths 4.0 m
 * and 12.5 m become 9.7 m" did not add up (12.5 m was read twice, on Main Line
 * and on Far Room). Every reading is listed by its line, and the dialog says
 * the station is already where its lines meet. Injection: list the two
 * station depths again, or drop the junction sentence, and the junction test
 * fails.
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
  junctions: [],
  candidates: [],
  default_keep: 0,
  keeps: [],
  ...extra,
});
/** Two stations' depths and the readings their mean takes, A's line first. */
const depths = (a_m: number, b_m: number, joined_m: number) => ({
  a_m,
  b_m,
  joined_m,
  readings: [
    { line_id: A.line_id, depth_m: a_m },
    { line_id: B.line_id, depth_m: b_m },
  ],
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
    expect(screen.queryByTestId(/^merge-preview-junction/)).not.toBeInTheDocument();
  });

  it('lists every reading by line onto a junction, and says it is one (V-2)', () => {
    // Upstream / 17 (4.0 m) onto Second entrance / B4, where Far room is
    // already merged: three readings, 12.5 m on each of B's lines.
    dialogData = {
      ...dialogData,
      mergePreview: preview({
        junctions: [{ station: B, line_ids: [2, 6] }],
        depths: {
          a_m: 4,
          b_m: 12.5,
          joined_m: 29 / 3,
          readings: [
            { line_id: 1, depth_m: 4 },
            { line_id: 2, depth_m: 12.5 },
            { line_id: 6, depth_m: 12.5 },
          ],
        },
      }),
    };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent(
      'Depths 4.0 m (Upstream), 12.5 m (Second entrance), 12.5 m (Far room) become 9.7 m: the joined station takes the mean of its depth readings.'
    );
    expect(screen.getByTestId('merge-preview-junction-2-5')).toHaveTextContent(
      'Second entrance / B4 is already where Second entrance and Far room meet.'
    );
  });

  it('says the depths when the two stations disagree', () => {
    dialogData = { ...dialogData, mergePreview: preview({ depths: depths(15, 4, 9.5) }) };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent(
      'Depths 15.0 m (Upstream), 4.0 m (Second entrance) become 9.5 m: the joined station takes the mean of its depth readings.'
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
      mergePreview: preview({ moving_line_ids: [2], joins: A, depths: depths(3, 5, 4) }),
    };
    render(<MergeDialog />);
    expect(screen.getByTestId('merge-datum-shift')).toBeInTheDocument();
    expect(screen.queryByTestId('merge-preview-move')).not.toBeInTheDocument();
    expect(screen.getByTestId('merge-preview-depths')).toHaveTextContent('Depths 3.0 m (Upstream), 5.0 m (Second entrance) become 4.0 m');
  });

  it('gives no distance after a snap-to-merge drag, which moved the piece first', () => {
    dialogData = { ...dialogData, mergePreview: preview({ depths: depths(15, 4, 9.5) }), preCommands: [] };
    render(<MergeDialog />);
    expect(screen.queryByTestId('merge-preview-move')).not.toBeInTheDocument();
    expect(screen.getByTestId('merge-preview-depths')).toBeInTheDocument();
  });
});
