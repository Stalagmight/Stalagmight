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
  useProjectStore.setState({ project: null, loadRepairs: null, loadRepairsDismissed: false });
});

describe('LoadRepairsBanner (#576 B35(h))', () => {
  it('lists what the open reset or dropped, per kind, with plural forms', () => {
    openWith({ shots: 1, sectionOffsets: 0, stationOffsets: 2, gpsPoints: 1 });
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
    openWith({ shots: 0, sectionOffsets: 3, stationOffsets: 0, gpsPoints: 0 });
    render(<LoadRepairsBanner />);
    expect(screen.getByTestId('load-repairs-banner')).toHaveTextContent('3 section moves reset');
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
