/**
 * The Data workspace's view persistence (`panels.data_workspace_*`): apply from
 * config, debounced save, optimistic publish, rollback + toast on failure,
 * flush on unmount, and the per-field merge that never clobbers an unsaved
 * local edit when the config arrives again.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import {
  MAP_PANE_MAX_WIDTH,
  normalizeDataWorkspaceView,
  resetDataWorkspaceViewPersistenceForTests,
  useDataWorkspaceViewPersistence,
} from '../useDataWorkspaceViewPersistence';
import { DEFAULT_VIEW, MAP_PANE_MIN_WIDTH, useDataWorkspaceStore } from '../store';
import { DATA_DEFAULT_HIDDEN } from '../columnsMeta';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig, type Config } from '../../../types/config';
import { TOAST_KEYS } from '../../../utils/toastKeys';

type Panels = Config['panels'];

function configWith(panels: Partial<Panels> = {}): Config {
  const config = structuredClone(defaultConfig) as Config;
  config.panels = { ...config.panels, ...panels };
  return config;
}

/** What `get_config` answers: the backend's current view of config.toml. */
let diskConfig: Config;

/** A backend that stores what `update_data_workspace_view` sends, like the real
 *  partial write does, so the post-save `loadConfig` re-sync reads it back. */
function persistingBackend(command: string, args?: Record<string, unknown>) {
  if (command === 'get_config') return Promise.resolve(structuredClone(diskConfig));
  if (command === 'update_data_workspace_view' && args) {
    const p = diskConfig.panels;
    if (args.columnOrder) p.data_workspace_column_order = args.columnOrder as string[];
    if (args.hiddenColumns) p.data_workspace_hidden_columns = args.hiddenColumns as string[];
    if (args.columnWidths) p.data_workspace_column_widths = args.columnWidths as Record<string, number>;
    if (args.tintMode) p.data_workspace_tint_mode = args.tintMode as string;
    if (args.mapPaneOpen !== undefined) p.data_workspace_map_pane_open = args.mapPaneOpen as boolean;
    if (args.mapPaneWidth !== undefined) p.data_workspace_map_pane_width = args.mapPaneWidth as number;
    if (args.sectionHeaders !== undefined) p.data_workspace_section_headers = args.sectionHeaders as boolean;
  }
  return Promise.resolve(undefined);
}

async function flushMicrotasks() {
  for (let i = 0; i < 5; i += 1) await Promise.resolve();
}

async function advance(ms: number) {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms);
  });
  await flushMicrotasks();
}

const saves = () => invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_workspace_view');
const view = () => useDataWorkspaceStore.getState();

beforeEach(() => {
  vi.useFakeTimers();
  resetDataWorkspaceViewPersistenceForTests();
  useDataWorkspaceStore.setState({ ...DEFAULT_VIEW, hiddenColumns: [...DATA_DEFAULT_HIDDEN], viewLoaded: false });
  diskConfig = configWith();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(persistingBackend);
  useProjectStore.setState({ config: null });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('useDataWorkspaceViewPersistence', () => {
  it('applies the persisted view when the config arrives, without saving it back', async () => {
    useProjectStore.setState({
      config: configWith({
        data_workspace_column_order: ['comment', 'length'],
        data_workspace_hidden_columns: ['x'],
        data_workspace_column_widths: { comment: 300 },
        data_workspace_tint_mode: 'depth',
        data_workspace_map_pane_open: true,
        data_workspace_map_pane_width: 600,
        data_workspace_section_headers: false,
      }),
    });
    renderHook(() => useDataWorkspaceViewPersistence());

    expect(view()).toMatchObject({
      viewLoaded: true,
      columnOrder: ['comment', 'length'],
      hiddenColumns: ['x'],
      columnWidths: { comment: 300 },
      tintMode: 'depth',
      mapPaneOpen: true,
      mapPaneWidth: 600,
      showSectionHeaders: false,
    });
    await advance(1000);
    expect(saves()).toHaveLength(0);
  });

  it('waits for the config before applying anything, then applies it', async () => {
    renderHook(() => useDataWorkspaceViewPersistence());
    expect(view().viewLoaded).toBe(false);

    act(() => {
      useProjectStore.setState({ config: configWith({ data_workspace_tint_mode: 'issues' }) });
    });
    expect(view().viewLoaded).toBe(true);
    expect(view().tintMode).toBe('issues');
    await advance(1000);
    expect(saves()).toHaveLength(0);
  });

  it('repairs a hostile or legacy config instead of trusting it', () => {
    const legacy = configWith({
      data_workspace_tint_mode: 'rainbow',
      data_workspace_map_pane_width: 1_000_000,
      data_workspace_column_order: ['length', 'not_a_column', 'length'] as string[],
      data_workspace_column_widths: { length: Number.NaN, comment: 250, runaway: 99_999, sliver: 3 },
    });
    // A backend that predates the hidden list sends no such field at all.
    delete (legacy.panels as Partial<Panels>).data_workspace_hidden_columns;
    useProjectStore.setState({ config: legacy });
    renderHook(() => useDataWorkspaceViewPersistence());

    expect(view().tintMode).toBe('none');
    expect(view().mapPaneWidth).toBe(MAP_PANE_MAX_WIDTH);
    expect(view().columnOrder).toEqual(['length']);
    expect(view().columnWidths).toEqual({ comment: 250, runaway: 2000, sliver: 40 });
    expect(view().hiddenColumns).toEqual([...DATA_DEFAULT_HIDDEN]);
  });

  it('drops the retired Links column from a saved view (its chips sit over the shot cells now)', () => {
    useProjectStore.setState({
      config: configWith({
        data_workspace_hidden_columns: ['links', 'x'],
        data_workspace_column_widths: { comment: 250 },
      }),
    });
    renderHook(() => useDataWorkspaceViewPersistence());
    expect(view().hiddenColumns).toEqual(['x']);
    expect(normalizeDataWorkspaceView({ columnOrder: ['links'] }).columnOrder).not.toContain('links');
  });

  it('keeps an empty hidden list as "show all"', () => {
    useProjectStore.setState({ config: configWith({ data_workspace_hidden_columns: [] }) });
    renderHook(() => useDataWorkspaceViewPersistence());
    expect(view().hiddenColumns).toEqual([]);
  });

  it('debounces changes into one save of the whole view, published optimistically', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());

    act(() => {
      view().setTintMode('depth');
      view().setMapPaneOpen(true);
    });
    await advance(300);
    act(() => view().setMapPaneWidth(700));
    await advance(300);
    expect(saves()).toHaveLength(0);

    await advance(250);
    expect(saves()).toHaveLength(1);
    expect(saves()[0]![1]).toEqual({
      columnOrder: [],
      hiddenColumns: [...DATA_DEFAULT_HIDDEN].sort(),
      columnWidths: {},
      tintMode: 'depth',
      mapPaneOpen: true,
      mapPaneWidth: 700,
      sectionHeaders: true,
    });
    // Published into the cached config before the IPC settles...
    const panels = useProjectStore.getState().config!.panels;
    expect(panels.data_workspace_tint_mode).toBe('depth');
    expect(panels.data_workspace_map_pane_width).toBe(700);
    // ...and re-synced from the backend afterwards, which changes nothing.
    expect(invokeIpcMock.mock.calls.some(([command]) => command === 'get_config')).toBe(true);
    expect(view()).toMatchObject({ tintMode: 'depth', mapPaneOpen: true, mapPaneWidth: 700 });
  });

  it('does not save a change that is undone inside the debounce window', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());

    act(() => view().setShowSectionHeaders(false));
    act(() => view().setShowSectionHeaders(true));
    await advance(1000);
    expect(saves()).toHaveLength(0);
  });

  it('rolls back and toasts when the save fails, so a later change retries it', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());
    invokeIpcMock.mockImplementation((command: string) => {
      if (command === 'update_data_workspace_view') return Promise.reject(new Error('disk full'));
      if (command === 'get_config') return Promise.resolve(structuredClone(diskConfig));
      return Promise.resolve(undefined);
    });

    act(() => view().setTintMode('distance'));
    await advance(500);
    expect(saves()).toHaveLength(1);
    expect(useUIStore.getState().toasts).toHaveLength(1);
    expect(useUIStore.getState().toasts[0]).toMatchObject({
      key: TOAST_KEYS.DATA_WORKSPACE_VIEW_SAVE,
      kind: 'error',
    });
    // The optimistic publish is undone from disk, but the user's choice stays.
    expect(useProjectStore.getState().config!.panels.data_workspace_tint_mode).toBe('none');
    expect(view().tintMode).toBe('distance');

    // The next change saves the whole view again, the failed tint included.
    invokeIpcMock.mockImplementation(persistingBackend);
    act(() => view().setMapPaneOpen(true));
    await advance(500);
    expect(saves()).toHaveLength(2);
    expect(saves()[1]![1]).toMatchObject({ tintMode: 'distance', mapPaneOpen: true });
  });

  it('flushes a pending save on unmount', async () => {
    useProjectStore.setState({ config: configWith() });
    const { unmount } = renderHook(() => useDataWorkspaceViewPersistence());

    act(() => view().setColumnWidths({ comment: 420 }));
    expect(saves()).toHaveLength(0);
    unmount();
    await flushMicrotasks();

    expect(saves()).toHaveLength(1);
    expect(saves()[0]![1]).toMatchObject({ columnWidths: { comment: 420 } });
  });

  it('never clobbers an unsaved local edit when the config arrives again', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());

    // A local edit, still inside the debounce window...
    act(() => view().setTintMode('depth'));
    // ...when the config is reloaded from disk with another tint AND another
    // column order (Settings > Reload from disk).
    act(() => {
      useProjectStore.setState({
        config: configWith({ data_workspace_tint_mode: 'type', data_workspace_column_order: ['comment'] }),
      });
    });
    expect(view().tintMode).toBe('depth');
    expect(view().columnOrder).toEqual(['comment']);

    await advance(500);
    expect(saves()).toHaveLength(1);
    expect(saves()[0]![1]).toMatchObject({ tintMode: 'depth', columnOrder: ['comment'] });
  });

  it('applies a later config arrival when there is nothing unsaved', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());

    act(() => {
      useProjectStore.setState({
        config: configWith({ data_workspace_map_pane_open: true, data_workspace_map_pane_width: 900 }),
      });
    });
    expect(view().mapPaneOpen).toBe(true);
    expect(view().mapPaneWidth).toBe(900);
    await advance(1000);
    expect(saves()).toHaveLength(0);
  });

  it('keeps the saved view across a remount (it lives in the global store)', async () => {
    useProjectStore.setState({ config: configWith() });
    const first = renderHook(() => useDataWorkspaceViewPersistence());
    act(() => view().setTintMode('issues'));
    await advance(500);
    first.unmount();
    expect(diskConfig.panels.data_workspace_tint_mode).toBe('issues');

    renderHook(() => useDataWorkspaceViewPersistence());
    expect(view().tintMode).toBe('issues');
    await advance(1000);
    expect(saves()).toHaveLength(1);
  });

  it('does not flick back to an older value when an earlier save re-syncs during a later one', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());
    const pending: Array<() => void> = [];
    invokeIpcMock.mockImplementation((command: string, args?: Record<string, unknown>) => {
      if (command === 'update_data_workspace_view') {
        // Held until the test lets it land; it reaches "disk" only then.
        return new Promise<void>((resolve) => {
          pending.push(() => {
            void persistingBackend(command, args);
            resolve();
          });
        });
      }
      return persistingBackend(command, args);
    });

    act(() => view().setTintMode('depth'));
    await advance(500);
    act(() => view().setTintMode('issues'));
    await advance(500);
    expect(saves()).toHaveLength(2);

    // The first save lands and re-syncs: the disk now says 'depth', which is
    // older than what the user has and what is still in flight.
    pending[0]!();
    await advance(0);
    expect(view().tintMode).toBe('issues');

    pending[1]!();
    await advance(0);
    expect(view().tintMode).toBe('issues');
    expect(useProjectStore.getState().config!.panels.data_workspace_tint_mode).toBe('issues');
  });

  it('saves a narrow map pane at the minimum width the config keeps', async () => {
    useProjectStore.setState({ config: configWith() });
    renderHook(() => useDataWorkspaceViewPersistence());

    act(() => useDataWorkspaceStore.setState({ mapPaneWidth: 10 }));
    await advance(500);
    expect(saves()[0]![1]).toMatchObject({ mapPaneWidth: MAP_PANE_MIN_WIDTH });
  });
});
