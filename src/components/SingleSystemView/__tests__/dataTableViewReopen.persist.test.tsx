/**
 * #357 item 4, the reopen half: closing the data table with a pending debounced
 * view save and reopening it INSIDE the flush window used to revert the write
 * that just went out.
 *
 * The unmount flush fires the IPC and only re-syncs `config` in the promise's
 * `.then`. A remount seeds its view state from `config.panels`, and the seed
 * effect is one-shot (`lastSavedViewRef.current !== null` bails on every later
 * run), so a mount that lands before the re-sync pins the PRE-save values as
 * "last saved" and never adopts the fresher config that arrives moments later.
 * The next edit then persists those stale values over the flushed ones.
 *
 * Rendered at the full-component tier (real DataTable, real projectStore,
 * mocked `invokeIpc`) because the ordering only exists across a real unmount +
 * remount of the component that owns the view state. `sections={[]}` keeps
 * SpreadsheetGrid out of the render; the column-visibility checkbox is enough to
 * change the view snapshot, and it stands in for the width edits that share the
 * same snapshot and the same debounced effect.
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

/** Backend state the mock persists across calls, so `get_config` answers with
 *  what `update_data_table_view` was last told - the property under test is
 *  precisely who wins between that answer and the in-memory config. */
let backendView: Record<string, unknown> = {};
/** Resolver for the in-flight `update_data_table_view`, so the test can hold the
 *  IPC open across the remount and land its `loadConfig` afterwards - the exact
 *  ordering the bug needs. */
let releaseSave: (() => void) | null = null;

beforeEach(() => {
  // jsdom has no ResizeObserver; DataTable's chrome-offset measurement effect
  // needs the constructor to exist, but this suite never depends on it firing.
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  vi.useFakeTimers();
  backendView = {};
  releaseSave = null;
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
    if (command === 'get_config') {
      const config = freshConfig();
      return Promise.resolve({ ...config, panels: { ...config.panels, ...backendView } });
    }
    if (command === 'update_data_table_view') {
      // Mirror the Rust command's partial-update semantics for the one field
      // this suite drives.
      const hidden = args?.hiddenColumns as string[] | undefined;
      return new Promise<void>((resolve) => {
        releaseSave = () => {
          if (hidden != null) backendView.data_table_shot_hidden_columns = hidden;
          resolve();
        };
      });
    }
    return Promise.resolve(undefined);
  });
  useProjectStore.setState({ config: freshConfig() });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

function toggleColumn(key: string) {
  fireEvent.click(screen.getByTestId('data-table-columns-menu'));
  fireEvent.click(screen.getByTestId(`col-vis-${key}`));
}

function updateViewCalls() {
  return invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view');
}

describe('DataTable view save across a close/reopen inside the flush window (#357 item 4)', () => {
  it('does not revert the flushed write when the table is reopened before the re-sync lands', async () => {
    const first = render(<DataTable sections={[]} />);

    // Hide `comment`, then close the table inside the 500ms debounce window so
    // the unmount flush - not the timer - is what fires the IPC.
    toggleColumn('comment');
    expect(updateViewCalls()).toHaveLength(0);
    first.unmount();
    await microtasks(2);
    expect(updateViewCalls()).toHaveLength(1);
    expect(updateViewCalls()[0]?.[1]).toMatchObject({
      hiddenColumns: ['comment', 'section_id', 'shot_id'],
    });

    // The IPC has NOT settled yet, so its `loadConfig` re-sync has not run.
    // Reopening here is the race: the new mount can only seed from the
    // in-memory config.
    render(<DataTable sections={[]} />);

    // Now let the write land and the re-sync arrive - after the seed, which is
    // the whole point: the one-shot seed will refuse to re-read it.
    releaseSave?.();
    await act(async () => {
      await microtasks(2);
    });

    // A further edit in the reopened table. Its payload is built from the state
    // the remount seeded, so it shows whether the flushed value survived.
    toggleColumn('_warnings');
    await act(async () => {
      await vi.advanceTimersByTimeAsync(500);
    });

    const calls = updateViewCalls();
    expect(calls).toHaveLength(2);
    // `comment` must still be hidden. Without the optimistic publish the second
    // payload drops it, silently undoing the write the unmount flush just made.
    expect(calls[1]?.[1]).toMatchObject({
      hiddenColumns: ['_warnings', 'comment', 'section_id', 'shot_id'],
    });
  });
});
