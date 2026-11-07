/**
 * #357 items 4 & 5 - the debounced data-table VIEW save (column order/hidden/
 * widths/angle modes), not to be confused with survey-data edits.
 *
 * Item 4: the effect's cleanup was `clearTimeout(timer)` only, and that runs on
 * every dep change as well as on unmount, so it could not tell the two apart -
 * closing the data table (or the project) inside the 500ms debounce window
 * dropped the pending write silently. The fix is a mount-only effect whose
 * cleanup runs exactly once, at unmount, and flushes `pendingViewSaveRef`.
 *
 * Item 5: a failed save only rolled its snapshot back (so a later change
 * retries) and logged to `console.error` - invisible to the user. The fix adds
 * an error toast alongside the existing rollback.
 *
 * Rendered at the full-component tier (real DataTable, real projectStore /
 * uiStore, mocked `invokeIpc`) even though the debounce state has since been
 * extracted into `useDataTableViewPersistence`: what these guards exercise is
 * the mount/unmount lifecycle and the store round trip around that hook, and
 * DataTable is what owns the view state the hook reads. `sections={[]}` keeps the grid itself
 * (SpreadsheetGrid) out of the render - these guards only exercise the toolbar's
 * column-visibility checkbox, which is enough to change the view snapshot and
 * schedule a save. NOT covered at this tier: actual column-resize drag
 * (`onColumnWidthsChange`) and the angle-mode tri-state control - both feed the
 * same debounced effect and snapshot, so a defect isolated to just those paths
 * would not be caught here.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { NoopResizeObserver } from '../../../test/factories/dom';
import { microtasks } from '../../../test/factories/async';

function freshConfig(): Config {
  return structuredClone(defaultConfig) as Config;
}

beforeEach(() => {
  // jsdom has no ResizeObserver; DataTable's chrome-offset measurement effect
  // needs the constructor to exist, but this suite never depends on it firing.
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) => {
    if (command === 'get_config') return Promise.resolve(freshConfig());
    return Promise.resolve(undefined);
  });
  useProjectStore.setState({ config: freshConfig() });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

/** Opens the eye menu and returns the 'comment' column's visibility checkbox -
 *  a plain single-key hide/show toggle, unaffected by the angle-mode tri-state
 *  logic, so toggling it twice is a clean round trip back to the seeded state. */
function commentCheckbox(): HTMLInputElement {
  fireEvent.click(screen.getByTestId('data-table-columns-menu'));
  return screen.getByTestId('col-vis-comment') as HTMLInputElement;
}

function updateViewCalls() {
  return invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view');
}

describe('DataTable debounced view-save guards (#357 items 4 & 5)', () => {
  it('flushes a pending view save on unmount instead of dropping it (item 4)', async () => {
    const { unmount } = render(<DataTable sections={[]} />);

    // Hide the comment column - a real, savable change to the view snapshot.
    fireEvent.click(commentCheckbox());

    // Still inside the 500ms debounce window: nothing written yet.
    expect(updateViewCalls()).toHaveLength(0);

    // Unmounting inside the window must flush the pending save rather than
    // dropping it (the old bug: cleanup was `clearTimeout` only).
    unmount();
    await microtasks(2);

    const calls = updateViewCalls();
    expect(calls).toHaveLength(1);
    // Seeded default hidden columns (section_id, shot_id) plus the just-hidden
    // 'comment', sorted (see `stableWidths`'s sibling sort in the component).
    expect(calls[0]?.[1]).toMatchObject({ hiddenColumns: ['comment', 'section_id', 'shot_id'] });
  });

  it('does NOT flush a stale value when a change is undone back to the last-saved state before unmount (item 4, the subtle half)', async () => {
    const { unmount } = render(<DataTable sections={[]} />);

    const checkbox = commentCheckbox();
    // Hide comment (schedules a save of the new state)...
    fireEvent.click(checkbox);
    // ...then undo it inside the same debounce window, back to what is
    // actually on disk. Without clearing `pendingViewSaveRef` on this
    // no-change path, the OLD closure (hiding comment) would still be armed
    // and would overwrite the state the user actually settled on.
    fireEvent.click(checkbox);

    unmount();
    await microtasks(2);

    expect(updateViewCalls()).toHaveLength(0);
  });

  it('shows an error toast AND rolls back the snapshot so a later identical change retries, on IPC rejection (item 5)', async () => {
    render(<DataTable sections={[]} />);

    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'update_data_table_view') return Promise.reject(new Error('disk full'));
      if (command === 'get_config') return Promise.resolve(freshConfig());
      return Promise.resolve(undefined);
    });

    const checkbox = commentCheckbox();
    fireEvent.click(checkbox); // hide comment -> V1

    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    await microtasks(2);

    // Half 1: the failure is visible.
    expect(updateViewCalls()).toHaveLength(1);
    const toasts = useUIStore.getState().toasts;
    expect(toasts).toHaveLength(1);
    expect(toasts[0]).toMatchObject({ key: 'data-table-view-save', kind: 'error' });

    // Half 2: the rollback. Undo back to the last-saved (V0) state - each
    // toggle's pending timer is cleared by the next effect's cleanup before it
    // can fire, so no write lands from these two clicks themselves - then
    // redo it (back to V1, the exact value that just failed).
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'get_config') return Promise.resolve(freshConfig());
      return Promise.resolve(undefined);
    });
    fireEvent.click(checkbox); // V0 (back to baseline)
    fireEvent.click(checkbox); // V1 again (the value that failed to save)

    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });
    await microtasks(2);

    // If the rollback had not happened, the ref would still read "V1 already
    // saved" and this identical V1 would be silently treated as a no-op -
    // exactly the regression #357 item 5 fixes.
    const callsAfterRetry = updateViewCalls();
    expect(callsAfterRetry).toHaveLength(2);
    expect(callsAfterRetry[1]?.[1]).toMatchObject({ hiddenColumns: ['comment', 'section_id', 'shot_id'] });
  });
});
