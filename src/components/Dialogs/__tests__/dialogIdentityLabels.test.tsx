/**
 * The small station/shot dialogs name what they act on by its names (SPEC 4.2):
 * a station `Main Line / 17`, a shot `Main Line: 4 → 5`, a line by its name -
 * never the positional `line:index` handle, a shot id or `Line <id>`.
 *
 * The station names differ from their indices on purpose (station i of Main
 * Line is `M<i>`, of Side is `S<i>`), so a label built from the handles can
 * never pass for one built from the names.
 *
 * A sentence draws each name as a prose pill (RS-PROSE): `said` checks the
 * pills' kinds and reads the sentence as a screen reader does, which is the
 * plain sentence word for word. Dialog titles stay plain text.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  // The delete preview never resolves: these tests read only the static text.
  invokeIpc: vi.fn(() => new Promise(() => {})),
}));

import '../../../i18n';
import { invokeIpc } from '../../../utils/ipc';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import DrawShotDialog from '../DrawShotDialog';
import EditShotDialog from '../EditShotDialog';
import RemoveClosureDialog from '../RemoveClosureDialog';
import UnmergeStationsDialog from '../UnmergeStationsDialog';
import DeleteShotChoiceDialog from '../DeleteShotChoiceDialog';
import SwapCommentsDialog from '../SwapCommentsDialog';
import ReverseLineDialog from '../ReverseLineDialog';
import BulkDeleteDialog from '../BulkDeleteDialog';
import { said } from './proseSentence';
import { accessibleText } from '../../../test/accessibleText';

const MAIN = 3;
const SIDE = 8;
const shot = (id: number) => ({
  id,
  azimuth_in: 0,
  azimuth_out: 180,
  length: 5,
  depth_in: 0,
  depth_out: 0,
  pitch_in: null,
  pitch_out: null,
  left: null,
  right: null,
  up: null,
  down: null,
  comment: null,
  kind: 'Normal',
  marker_index: null,
  temperature: null,
});

const station = (lineId: number, index: number, prefix: string) => ({
  station_ref: { line_id: lineId, station_index: index },
  name: `${prefix}${index}`,
  coordinates: { x: 0, y: 0, z: 0 },
  merged_with: [],
  closures_with: [],
});

beforeEach(() => {
  useProjectStore.setState({
    project: {
      lines: [
        { id: MAIN, name: 'Main Line', shots: [shot(30), shot(31), shot(32)] },
        { id: SIDE, name: 'Side', shots: [shot(80), shot(81)] },
      ],
      station_merges: [],
    },
    surveyData: null,
    stations: [
      ...[0, 1, 2, 3].map((i) => station(MAIN, i, 'M')),
      ...[0, 1, 2].map((i) => station(SIDE, i, 'S')),
    ],
    isShotFrozen: () => false,
  } as never);
});

const open = (dialogData: Record<string, unknown>) =>
  useUIStore.setState({ dialogData } as never);

/** The pill kinds in the element `testId`, and its text as a screen reader reads it. */
const said = (testId: string) => {
  const el = screen.getByTestId(testId);
  return {
    pills: [...el.querySelectorAll('.sh-ppill')].map((pill) => pill.getAttribute('data-prose-kind')),
    text: accessibleText(el).replace(/\s+/g, ' ').trim(),
  };
};

/** No positional `3:1` handle, `Line 3` id fallback or `#2` shot number. */
const expectNoHandles = () => expect(document.body.textContent).not.toMatch(/\b\d+:\d+\b|Line \d|Shot ID/);

describe('dialog identity labels', () => {
  it('Draw Shot names the station it starts from', () => {
    open({ fromStation: { line_id: MAIN, station_index: 2 }, appendMode: true });
    render(<DrawShotDialog />);
    expect(document.body.textContent).toContain('Main Line: M2');
    expect(document.body.textContent).toContain('Add Shot After Station M2');
    expectNoHandles();
  });

  it('Edit Shot names the shot by its line and stations', () => {
    open({ lineId: MAIN, shotId: 31 });
    render(<EditShotDialog />);
    expect(said('edit-shot-identity')).toEqual({ pills: ['shot'], text: 'Main Line: M1 → M2' });
    expectNoHandles();
  });

  it('Remove Closure names the station at the far end of each closure', () => {
    open({
      anchorStation: { line_id: MAIN, station_index: 1 },
      closures: [{ closureId: 4n, stationRef: { line_id: SIDE, station_index: 2 } }],
    });
    render(<RemoveClosureDialog />);
    expect(document.body.textContent).toContain('Side: S2');
    expectNoHandles();
  });

  it('Detach names the station and each line through it', async () => {
    // A two-line junction lists its lines once the previews have answered
    // (here they fail, so the choice cannot be ruled out, N-2).
    vi.mocked(invokeIpc).mockRejectedValueOnce(new Error('No project open')).mockRejectedValueOnce(new Error('No project open'));
    open({
      stationRef: { line_id: MAIN, station_index: 0 },
      mergedStations: [{ line_id: SIDE, station_index: 1 }],
    });
    render(<UnmergeStationsDialog />);
    expect(await screen.findByText('Main Line (the line you clicked)')).toBeInTheDocument();
    expect(screen.getByTestId('unmerge-stations-lead')).toHaveTextContent('M0 is where 2 lines meet');
    expect(screen.getByText('Side')).toBeInTheDocument();
    expectNoHandles();
  });

  it('Detach with no choice to offer names the line that comes off and the one that keeps the station', async () => {
    const preview = (kept: boolean) => ({ name: 'S1', station_id: 9, restored: true, kept, lines: [] });
    vi.mocked(invokeIpc)
      .mockImplementationOnce(async () => preview(true))
      .mockImplementationOnce(async () => preview(false));
    open({
      stationRef: { line_id: MAIN, station_index: 0 },
      mergedStations: [{ line_id: SIDE, station_index: 1 }],
    });
    render(<UnmergeStationsDialog />);
    // The sentence draws its names as prose pills; it reads as it was written.
    expect(said(await screen.findByTestId('unmerge-stations-naming'))).toBe(
      'Side comes off: it gets back S1 (Station ID 9); Main Line keeps station M0.',
    );
    expectNoHandles();
  });

  it('Delete Shot names the shot it deletes', () => {
    open({ lineId: MAIN, shotId: 31, lineName: 'Main Line' });
    render(<DeleteShotChoiceDialog />);
    expect(said('delete-shot-choice-lead')).toEqual({
      pills: ['shot'],
      text: 'Shot Main Line: M1 → M2 is in the middle of the line. Choose whether to split the line or shift geometry.',
    });
    expectNoHandles();
  });

  it('Bulk Delete names the range by its station names', () => {
    open({
      lineId: MAIN,
      startShotId: 30,
      endShotId: 31,
      shotCount: 2,
      kind: 'interior',
      lineName: 'Main Line',
      startStationIndex: 0,
      endStationIndex: 2,
    });
    render(<BulkDeleteDialog />);
    expect(said('bulk-delete-summary')).toEqual({
      pills: ['station', 'station', 'line'],
      text: 'Delete 2 shots (stations M0 to M2 in Main Line).',
    });
    expectNoHandles();
  });

  it('Move comment names the station and the shot that hold the comments', () => {
    open({
      source: { type: 'station', stationRef: { line_id: SIDE, station_index: 1 }, comment: 'Tight' },
      target: { type: 'shot', lineId: MAIN, shotId: 32, comment: 'Silt' },
    });
    render(<SwapCommentsDialog />);
    expect(said('swap-comments-source')).toEqual({ pills: ['station'], text: 'Comment being moved, from station Side / S1:' });
    expect(said('swap-comments-target')).toEqual({
      pills: ['shot'],
      text: 'The shot Main Line: M2 → M3 already has a comment:',
    });
    expectNoHandles();
  });

  it('Reverse names the line and where it will run, by station names', () => {
    open({ lineIds: [SIDE] });
    render(<ReverseLineDialog />);
    expect(said('reverse-line-list')).toEqual({
      pills: ['line', 'station', 'station'],
      text: 'Side (will run from S2 to S0)',
    });
    expectNoHandles();
  });
});
