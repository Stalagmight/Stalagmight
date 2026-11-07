/**
 * [#576 RV6b-3] The data-table view save re-reads the config in the config chain.
 *
 * A view save (`update_data_table_view`) is followed by a re-read of the whole
 * config, so a remount seeds from what reached disk. That re-read ran in the
 * view's own write chain, beside the config chain every settings save runs in
 * (save, then its own re-read). Two re-reads in flight answer in either order,
 * so the view's - issued before a settings save landed - could answer after the
 * save's own re-read and put a config WITHOUT that save back into the store,
 * until the next re-read. It now waits in the config chain behind the saves
 * issued before it, and so reads a config that has them.
 *
 * Full-component tier like dataTableViewSave.persist.test.tsx (real DataTable and
 * stores, mocked IPC); every config answer is held and released by the test, in
 * the losing order. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';

import { invokeIpcMock } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');

import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

/** The backend's config: what each get_config snapshots WHEN IT IS CALLED. */
let backend: Config;
/** Held IPC answers, by command, in call order. */
let held: Record<string, Array<() => void>>;

function hold(command: string, answer: () => unknown): Promise<unknown> {
  return new Promise((resolve) => {
    (held[command] ??= []).push(() => resolve(answer()));
  });
}

async function until(predicate: () => boolean): Promise<void> {
  await vi.waitFor(() => expect(predicate()).toBe(true));
}

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  backend = structuredClone(defaultConfig) as Config;
  held = {};
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case 'get_config': {
        const snapshot = structuredClone(backend);
        return hold('get_config', () => snapshot);
      }
      case 'patch_config':
        return hold('patch_config', () => {
          backend = { ...backend, general: { ...backend.general, auto_save_interval_seconds: 45 } };
          return { keychain_residue: null, persistent_residue: null };
        });
      case 'update_data_table_view':
        backend = { ...backend, panels: { ...backend.panels, data_table_shot_hidden_columns: args?.hiddenColumns as string[] } };
        return Promise.resolve(undefined);
      default:
        return Promise.resolve(undefined);
    }
  });
  useProjectStore.setState({ config: structuredClone(defaultConfig) as Config });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('data-table view save vs. a settings save (#576 RV6b-3)', () => {
  it("the view's config re-read cannot land a config without a settings save made before it", async () => {
    expect(useProjectStore.getState().config?.general.auto_save_interval_seconds).toBe(30);

    // A settings save is in flight (its patch_config not answered yet)...
    const saved = useProjectStore.getState().patchConfig([{ path: ['general', 'auto_save_interval_seconds'], value: 45 }]);
    await until(() => (held.patch_config?.length ?? 0) === 1);

    // ...when a data-table view change is flushed (unmount flushes the debounce).
    const { unmount } = render(<DataTable sections={[]} />);
    fireEvent.click(screen.getByTestId('data-table-columns-menu'));
    fireEvent.click(screen.getByTestId('col-vis-comment'));
    unmount();
    await until(() => invokeIpcMock.mock.calls.some(([command]) => command === 'update_data_table_view'));
    await new Promise((resolve) => setTimeout(resolve, 10));
    const viewReadsBeforeSaveLanded = held.get_config?.length ?? 0;

    // The settings save lands, then its own re-read answers...
    held.patch_config![0]!();
    await until(() => (held.get_config?.length ?? 0) >= 1 + viewReadsBeforeSaveLanded);
    held.get_config![viewReadsBeforeSaveLanded]!();
    await saved;
    // ...and only then does the view's re-read answer (on the base it was issued
    // before the save landed; with the fix it is issued only now).
    await until(() => (held.get_config?.length ?? 0) === 2);
    const viewRead = viewReadsBeforeSaveLanded === 1 ? 0 : 1;
    held.get_config![viewRead]!();
    await new Promise((resolve) => setTimeout(resolve, 10));

    const after = useProjectStore.getState().config;
    console.log(
      `[RV6b-3] view re-read issued before the settings save landed: ${viewReadsBeforeSaveLanded === 1}; ` +
        `store auto_save_interval_seconds after both re-reads: ${String(after?.general.auto_save_interval_seconds)} (expected 45), ` +
        `hidden columns: ${JSON.stringify(after?.panels.data_table_shot_hidden_columns)}`,
    );
    expect(viewReadsBeforeSaveLanded).toBe(0);
    expect(after?.general.auto_save_interval_seconds).toBe(45);
    expect(after?.panels.data_table_shot_hidden_columns).toContain('comment');
  });
});
