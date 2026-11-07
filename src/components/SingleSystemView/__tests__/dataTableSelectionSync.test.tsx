/**
 * D21 (#576): an external selection (map, sections panel) always moves the data
 * table's cursor, however soon it follows a keyboard move in the table.
 *
 * The table handed the grid a level-triggered `selectedRow` derived from a
 * scroll-key state that two 100 ms timers reset to null. The grid reacted only
 * when that prop CHANGED, so re-selecting a shot on the map within 100 ms of
 * the first selection - after the user had moved the cursor off it with the
 * keyboard - set the same key again: React bailed out of the same-value
 * update, the prop never changed, and the table stayed on the wrong row while
 * the map and detail panel showed the selected shot. Each external selection
 * is now a request with its own nonce, which the grid honours once.
 *
 * Fake timers hold the old 100 ms timers shut, which is the race window.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', async () =>
  (await import('../../../test/factories/virtualizer')).fakeVirtualizer({ headerOffset: 0 }),
);

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { Section, Shot, Station } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';
import { makeSection, makeShot, makeStation } from '../../../test/factories/survey';

const shot = (id: number): Shot =>
  makeShot({ id, azimuth_in: 10, azimuth_out: 10, length: id * 10, depth_out: 1 });

const section: Section = makeSection({ name: 'CaveA', shots: [shot(1), shot(2), shot(3)] });

const station = (index: number): Station =>
  makeStation({ station_ref: { section_id: 1, station_index: index }, coordinates: { x: index, y: 0, z: 0 } });

/** The grid row holding the cursor, as its row index (-1 when none). */
const activeRow = () => {
  const row = document.querySelector('[role="row"][data-active="true"]');
  return row ? Number(row.getAttribute('data-testid')?.replace('grid-row-', '')) : -1;
};

const scroller = () => document.querySelector('[data-grid-id="grid"] > div[tabindex="0"]')!;

function selectExternally(selection: Parameters<ReturnType<typeof useUIStore.getState>['setSelection']>[0]) {
  act(() => useUIStore.getState().setSelection(selection));
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? structuredClone(defaultConfig) : undefined)
  );
  useProjectStore.setState({
    config: structuredClone(defaultConfig) as Config,
    stations: [station(0), station(1), station(2), station(3)],
  });
  useUIStore.setState({ toasts: [], selection: null });
  useSpeleoStore.setState({ projectLinkKind: 'local', link: null });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe('DataTable selection sync with the map (D21)', () => {
  it('moves the cursor back to a shot re-selected right after a keyboard move', () => {
    render(<DataTable sections={[section]} />);

    selectExternally({ type: 'shot', sectionId: 1, shotId: 2 });
    expect(activeRow()).toBe(1);

    // Keyboard move inside the table: cursor and store selection on shot 3.
    act(() => {
      fireEvent.keyDown(scroller(), { key: 'ArrowDown' });
    });
    expect(activeRow()).toBe(2);
    expect(useUIStore.getState().selection).toMatchObject({ type: 'shot', shotId: 3 });

    // The map selects shot 2 again, well inside 100 ms of the first selection.
    selectExternally({ type: 'shot', sectionId: 1, shotId: 2 });
    expect(activeRow()).toBe(1);

    // Still true once any timer has had its chance to run.
    act(() => {
      vi.advanceTimersByTime(500);
    });
    expect(activeRow()).toBe(1);
  });

  // #576 NF-GRID-1: a draft row is not a shot, so moving the cursor onto one
  // leaves the store selection on the shot the cursor came from. Clicking that
  // same shot on the map again is then a same-value selection - which the table
  // ignored, leaving the cursor on the draft row while the map showed the shot.
  it('moves the cursor back from a draft row when the same shot is selected again', () => {
    render(<DataTable sections={[section]} />);

    selectExternally({ type: 'shot', sectionId: 1, shotId: 3 });
    expect(activeRow()).toBe(2);

    // Down onto the end draft row (row 3): the selection stays on shot 3.
    act(() => {
      fireEvent.keyDown(scroller(), { key: 'ArrowDown' });
    });
    expect(activeRow()).toBe(3);
    expect(useUIStore.getState().selection).toMatchObject({ type: 'shot', shotId: 3 });

    // The map selects shot 3 again.
    selectExternally({ type: 'shot', sectionId: 1, shotId: 3 });
    expect(activeRow()).toBe(2);
  });

  it('does the same in the stations view', () => {
    render(<DataTable sections={[section]} />);

    selectExternally({ type: 'station', stationRef: { section_id: 1, station_index: 1 } });
    expect(screen.getByTestId('data-table-view-stations')).toBeTruthy();
    expect(activeRow()).toBe(1);

    act(() => {
      fireEvent.keyDown(scroller(), { key: 'ArrowDown' });
    });
    expect(activeRow()).toBe(2);

    selectExternally({ type: 'station', stationRef: { section_id: 1, station_index: 1 } });
    expect(activeRow()).toBe(1);
  });

  it('a later row shift does not pull the cursor back to an old external selection', () => {
    const { rerender } = render(<DataTable sections={[section]} />);

    selectExternally({ type: 'shot', sectionId: 1, shotId: 3 });
    expect(activeRow()).toBe(2);
    act(() => {
      fireEvent.keyDown(scroller(), { key: 'ArrowUp' });
    });
    expect(activeRow()).toBe(1);

    // A shot appears above the old selection (e.g. an undo restoring one),
    // still inside the old 100 ms window: shot 3 moves from row 2 to row 3.
    rerender(<DataTable sections={[{ ...section, shots: [shot(9), ...section.shots] }]} />);
    expect(activeRow()).toBe(1);
  });
});
