/**
 * [#576 B35(h)] One notice after an open that reset or dropped stored values, with
 * a count per kind: the load result's `loadRepairs` is shown once, in plain words,
 * and can be dismissed. Nothing shows for a clean load.
 *
 * Real i18n (not a key-echo mock), so the plural forms the user reads are checked.
 * `.tsx` so vitest runs it under jsdom.
 */
import { afterEach, describe, expect, it } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

import '../../../i18n';
import { LoadRepairsBanner } from '../LoadRepairsBanner';
import { useProjectStore } from '../../../store/projectStore';
import type { Project } from '../../../types/ipc/Project';

const openProject = { name: 'CaveA', file_path: '/tmp/CaveA.stalx' } as unknown as Project;

function openWith(loadRepairs: ReturnType<typeof useProjectStore.getState>['loadRepairs']) {
  useProjectStore.setState({ project: openProject, loadRepairs, loadRepairsDismissed: false });
}

afterEach(() => {
  useProjectStore.setState({ project: null, loadRepairs: null, mergeSourceRepairs: [], loadRepairsDismissed: false });
});

describe('LoadRepairsBanner (#576 B35(h))', () => {
  it('lists what the open reset or dropped, per kind, with plural forms', () => {
    openWith({ shots: 1, sectionOffsets: 0, stationOffsets: 2, gpsPoints: 1, stationComments: 0 });
    render(<LoadRepairsBanner />);
    const banner = screen.getByTestId('load-repairs-banner');
    expect(banner).toHaveTextContent(
      'Some stored values could not be used and were changed when this project opened: ' +
        '1 shot with an invalid value reset, 2 station moves reset, 1 GPS tie point dropped.',
    );
    expect(banner).toHaveTextContent('Saving makes the change permanent.');
    expect(banner).not.toHaveTextContent('section move');
  });

  it('can be dismissed for the open project', () => {
    openWith({ shots: 0, sectionOffsets: 3, stationOffsets: 0, gpsPoints: 0, stationComments: 0 });
    render(<LoadRepairsBanner />);
    expect(screen.getByTestId('load-repairs-banner')).toHaveTextContent('3 section moves reset');
    act(() => {
      fireEvent.click(screen.getByTestId('load-repairs-dismiss-button'));
    });
    expect(screen.queryByTestId('load-repairs-banner')).not.toBeInTheDocument();
  });

  // [#576 Q-W30-3-D1] A comment row past the end of its section is dropped on load
  // and listed here, so the user knows it is gone before a save makes that final.
  it('lists the station comments the open dropped as orphans', () => {
    openWith({ shots: 0, sectionOffsets: 0, stationOffsets: 0, gpsPoints: 0, stationComments: 1 });
    render(<LoadRepairsBanner />);
    expect(screen.getByTestId('load-repairs-banner')).toHaveTextContent(
      'Some stored values could not be used and were changed when this project opened: ' +
        '1 station comment past the end of its section dropped.',
    );
    act(() => {
      openWith({ shots: 0, sectionOffsets: 0, stationOffsets: 0, gpsPoints: 0, stationComments: 2 });
    });
    expect(screen.getByTestId('load-repairs-banner')).toHaveTextContent(
      '2 station comments past the end of their sections dropped.',
    );
  });

  // [#576 W57-NF2] A merge reads each source as an open does; what that read reset
  // or dropped is not in the merged file, so the notice names the source - over the
  // project list too, where a merge that did not open its result leaves the user.
  it('lists what a merge read of each source changed, named by source', () => {
    useProjectStore.setState({ project: null, loadRepairs: null, loadRepairsDismissed: false });
    act(() => {
      useProjectStore.getState().showMergeSourceRepairs([
        {
          path: '/caves/B.stalx',
          name: 'Cave B',
          repairs: { shots: 1, section_offsets: 0, station_offsets: 0, gps_points: 0, station_comments: 2 },
        },
        {
          path: 'C:\\caves\\clean.stalx',
          name: 'Clean',
          repairs: { shots: 0, section_offsets: 0, station_offsets: 0, gps_points: 0, station_comments: 0 },
        },
      ]);
    });
    render(<LoadRepairsBanner />);
    const lines = screen.getAllByTestId(/^load-repairs-merge-source-/);
    expect(lines).toHaveLength(1);
    expect(lines[0]).toHaveAttribute('data-testid', 'load-repairs-merge-source-B.stalx');
    expect(lines[0]).toHaveTextContent(
      'Some stored values in Cave B (B.stalx) could not be used and were changed in the merged project: ' +
        '1 shot with an invalid value reset, 2 station comments past the end of their sections dropped.',
    );
    expect(screen.getByTestId('load-repairs-banner')).toHaveTextContent('The source files are unchanged.');
    expect(screen.getByTestId('load-repairs-banner')).not.toHaveTextContent('when this project opened');
    act(() => {
      fireEvent.click(screen.getByTestId('load-repairs-dismiss-button'));
    });
    expect(screen.queryByTestId('load-repairs-banner')).not.toBeInTheDocument();
  });

  it('shows nothing when the load reset nothing', () => {
    openWith(null);
    render(<LoadRepairsBanner />);
    expect(screen.queryByTestId('load-repairs-banner')).not.toBeInTheDocument();
  });
});
