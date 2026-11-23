/**
 * RS-PROSE: the range delete names the stations its warnings are about (the
 * zero datums and station comments the delete drops, the datum that blocks
 * closing the gap) as prose pills, and each sentence reads, to a screen
 * reader, exactly as plain text wrote it. The backend's shift-blocked reasons
 * hold no names and stay plain.
 *
 * Injection: rendering `removedDatums.map(fmt).join(', ')` as a plain label
 * again fails "draws a pill"; rendering the token string raw fails "reads".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

const invokeIpc = vi.fn();
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: (...args: unknown[]) => invokeIpc(...args),
}));

import '../../../i18n';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import BulkDeleteDialog from '../BulkDeleteDialog';
import { accessibleText } from '../../../test/accessibleText';

const MAIN = 3;
const ref = (station_index: number) => ({ line_id: MAIN, station_index });
const shot = (id: number) => ({ id, length: 5 });

beforeEach(() => {
  vi.clearAllMocks();
  useProjectStore.setState({
    project: {
      lines: [{ id: MAIN, name: 'Annex', shots: [shot(30), shot(31), shot(32), shot(33), shot(34)] }],
      station_merges: [],
    },
    surveyData: null,
    // Named unlike their indices: a label built from the handles never passes.
    stations: [0, 1, 2, 3, 4, 5].map((i) => ({
      station_ref: ref(i),
      name: `N${i}`,
      coordinates: { x: 0, y: 0, z: 0 },
      merged_with: [],
    })),
  } as never);
  useUIStore.setState({ syntacticLevel: 'full' } as never);
});

const impact = (over: Record<string, unknown>) => ({
  line_id: MAIN,
  start_pos: 1,
  end_pos: 2,
  kind: 'interior',
  removed_stations: [],
  removed_merges: [],
  removed_closures: [],
  closure_to_merge: [],
  shift_blocked: false,
  shift_blocked_reasons: [],
  removed_datums: [],
  removed_comments: [],
  ...over,
});

const open = (kind: 'interior' | 'prefix', start: number, end: number) =>
  useUIStore.setState({
    dialogData: {
      lineId: MAIN,
      startShotId: 30 + start,
      endShotId: 30 + end - 1,
      shotCount: end - start,
      kind,
      startStationIndex: start,
      endStationIndex: end,
    },
  } as never);

/** The pill kinds in `testId`, and its words as a screen reader reads them. */
const said = (testId: string) => {
  const el = screen.getByTestId(testId);
  return {
    pills: [...el.querySelectorAll('.sh-ppill')].map((pill) => pill.getAttribute('data-prose-kind')),
    text: accessibleText(el).replace(/\s+/g, ' ').trim(),
  };
};

describe('Bulk Delete names its stations as prose pills (RS-PROSE)', () => {
  it('an interior range: the summary, the datums, and the comments of each choice', async () => {
    open('interior', 1, 4);
    invokeIpc.mockResolvedValue(
      impact({ end_pos: 2, removed_datums: [ref(2)], removed_comments: [ref(1), ref(2), ref(3)] }),
    );
    render(<BulkDeleteDialog />);
    expect(said('bulk-delete-summary')).toEqual({
      pills: ['station', 'station', 'line'],
      text: 'Delete 3 shots (stations N1 to N4 in Annex).',
    });
    expect(await screen.findByTestId('bulk-delete-datums')).toBeInTheDocument();
    expect(said('bulk-delete-datums')).toEqual({
      pills: ['station'],
      text: "Zero datums (anchor the survey's real-world position) on N2.",
    });
    expect(said('bulk-delete-split-comments')).toEqual({
      pills: ['station', 'station'],
      text: 'Station comments on N1, N2.',
    });
    expect(said('bulk-delete-gap-comments')).toEqual({
      pills: ['station'],
      text: 'Closing the gap also removes comments on N3 (splitting keeps them).',
    });
    // The drawing is the bare name: the summary names the line.
    const pill = screen.getByTestId('bulk-delete-split-comments').querySelector('.sh-ppill')!;
    expect(pill.querySelector('[aria-hidden="true"]')!.textContent).toBe('N1');
  });

  it('a prefix range: the blocking datum and the comments the delete drops', async () => {
    open('prefix', 0, 2);
    invokeIpc.mockResolvedValue(
      impact({
        kind: 'prefix',
        start_pos: 0,
        end_pos: 1,
        shift_blocked: true,
        shift_blocked_reasons: ['A datum holds this range.'],
        removed_datums: [ref(1)],
        removed_comments: [ref(0)],
      }),
    );
    render(<BulkDeleteDialog />);
    expect(await screen.findByTestId('bulk-delete-blocking-datums')).toBeInTheDocument();
    expect(said('bulk-delete-blocking-datums')).toEqual({ pills: ['station'], text: 'Blocking zero datum on N1.' });
    // Blocked: closing the gap drops no comment, so no comment paragraph.
    expect(screen.queryByTestId('bulk-delete-gap-comments')).toBeNull();
    // The backend's reason holds no name: plain text.
    expect(screen.getByText('A datum holds this range.').querySelector('.sh-ppill')).toBeNull();
  });

  it('a prefix range that closes the gap names the comments it drops', async () => {
    open('prefix', 0, 2);
    invokeIpc.mockResolvedValue(impact({ kind: 'prefix', start_pos: 0, end_pos: 1, removed_comments: [ref(0), ref(1)] }));
    render(<BulkDeleteDialog />);
    expect(await screen.findByTestId('bulk-delete-gap-comments')).toBeInTheDocument();
    expect(said('bulk-delete-gap-comments')).toEqual({ pills: ['station', 'station'], text: 'Station comments on N0, N1.' });
  });
});
