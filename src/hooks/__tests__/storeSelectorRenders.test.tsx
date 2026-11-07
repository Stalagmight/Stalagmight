/**
 * Render-count guard for the always-mounted store subscribers (#576 D2).
 *
 * A zustand store hook called WITHOUT a selector (`useProjectStore()`)
 * subscribes its component to the whole store, so every `set()` re-renders it:
 * a dirty-flag flip, a loading-progress tick, the undo availability after each
 * edit. The hooks and components below are mounted for the whole life of the
 * survey view (the keyboard shortcuts, the map tools inside MapCanvas, the grid,
 * the undo/redo buttons, the detail panel) or of the Global View, so each one
 * used to re-render on writes to fields it never reads.
 *
 * Each case mounts the subject against the REAL stores, lets it settle, then
 * writes a field the subject does not read and asserts the render count did not
 * move. A write to a field it DOES read is the control: it must still re-render,
 * so a test cannot pass by the subject never re-rendering at all.
 *
 * The lint half of the guard is stalagmight/require-store-selector
 * (scripts/eslint/require-store-selector.mjs): no store hook call without a
 * selector anywhere under src/.
 *
 * `.tsx` so this runs in the jsdom tier (vite.config.ts environmentMatchGlobs).
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { Profiler, type ReactNode } from 'react';
import { act, render, renderHook } from '@testing-library/react';
import type * as maplibregl from 'maplibre-gl';

// maplibre-gl touches `window.URL.createObjectURL` at module init, which jsdom
// lacks. The subjects import it for types only.
vi.mock('maplibre-gl', () => ({ default: {} }));

import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useMapTools } from '../useMapTools';
import { useGlobalViewShortcuts } from '../useGlobalViewShortcuts';
import GridOverlay from '../../components/Map/GridOverlay';
import CommentOverlay from '../../components/Map/CommentOverlay';
import UndoHistoryDropdown from '../../components/SingleSystemView/UndoHistoryDropdown';
import DetailPanel from '../../components/SingleSystemView/DetailPanel';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { useGlobalViewStore } from '../../store/globalViewStore';
import type { Project, Station } from '../../types/project';
import { makeStation as stationOf } from '../../test/factories/survey';

function makeProject(name: string): Project {
  return {
    name,
    location: null,
    created_at: '2026-01-01T00:00:00Z',
    modified_at: '2026-01-01T00:00:00Z',
    sections: [],
    station_merges: [],
    zero_datums: [],
    loop_closures: [],
    file_path: '/mock/a.stalx',
    next_section_id: 1,
    section_offsets: {},
    imported_file_hashes: [],
  } as unknown as Project;
}

function makeStation(station_index: number, x: number): Station {
  return stationOf({
    station_ref: { section_id: 1, station_index },
    coordinates: { x, y: 0, z: 0 },
    closures_with: [],
  });
}

const UNRELATED = 'renders after writes to fields it does not read (count before the writes on the right)';

/** Fields none of the subjects read: the "unrelated" writes. */
function unrelatedProjectWrites() {
  act(() => useProjectStore.setState((s) => ({ isDirty: !s.isDirty })));
  act(() => useProjectStore.setState((s) => ({ integrityDismissed: !s.integrityDismissed })));
}

function makeMap(): maplibregl.Map {
  const layers = new Set<string>();
  const sources = new Set<string>();
  return {
    style: {},
    getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
    addLayer: (layer: { id: string }) => layers.add(layer.id),
    removeLayer: (id: string) => layers.delete(id),
    getSource: (id: string) => (sources.has(id) ? { setData: () => {} } : undefined),
    addSource: (id: string) => sources.add(id),
    removeSource: (id: string) => sources.delete(id),
    setPaintProperty: () => {},
    on: () => {},
    off: () => {},
  } as unknown as maplibregl.Map;
}

/** Mount `node` under a Profiler and return a live commit counter. */
function renderCounted(node: ReactNode) {
  const counter = { commits: 0 };
  const view = render(
    <Profiler id="subject" onRender={() => { counter.commits += 1; }}>
      {node}
    </Profiler>,
  );
  return { counter, view };
}

beforeEach(() => {
  useProjectStore.setState({
    project: makeProject('CaveA'),
    stations: [makeStation(0, 0), makeStation(1, 10)],
    isDirty: false,
    integrityDismissed: false,
    canUndo: false,
    canRedo: false,
  });
});

describe('always-mounted hooks do not re-render on unrelated store writes', () => {
  it('useKeyboardShortcuts', () => {
    let renders = 0;
    renderHook(() => {
      renders += 1;
      useKeyboardShortcuts();
    });
    const settled = renders;
    unrelatedProjectWrites();
    expect(renders, UNRELATED).toBe(settled);
    // Control: `stations` is read, so a write to it must still re-render.
    act(() => useProjectStore.setState({ stations: [makeStation(0, 5)] }));
    expect(renders).toBeGreaterThan(settled);
  });

  it('useMapTools (called by MapCanvas on every frame it renders)', () => {
    let renders = 0;
    renderHook(() => {
      renders += 1;
      useMapTools();
    });
    const settled = renders;
    unrelatedProjectWrites();
    expect(renders, UNRELATED).toBe(settled);
    act(() => useProjectStore.setState({ project: makeProject('CaveB') }));
    expect(renders).toBeGreaterThan(settled);
  });

  it('useGlobalViewShortcuts', () => {
    let renders = 0;
    const noop = () => {};
    renderHook(() => {
      renders += 1;
      useGlobalViewShortcuts({
        selectedProjectId: null,
        multiSelectedIds: new Set(),
        activeTab: 'map',
        recentProjects: [],
        loading: { isLoading: false },
        contextMenu: { isOpen: false, x: 0, y: 0, projectId: null } as never,
        mapRef: { current: null },
        setSelectedProjectId: noop,
        setMultiSelectedIds: noop,
        setContextMenu: noop,
        handleProjectOpen: noop,
        handleRemoveFromRecent: async () => {},
      });
    });
    const settled = renders;
    unrelatedProjectWrites();
    // It reads only actions from the Global View store, so no write there re-renders it.
    act(() => useGlobalViewStore.setState({ selectedSystemIds: new Set(['/mock/b.stalx']) }));
    expect(renders, UNRELATED).toBe(settled);
    // Control: `config` (the shortcut map) is read.
    act(() => useProjectStore.setState({ config: { ...(useProjectStore.getState().config ?? {}) } as never }));
    expect(renders).toBeGreaterThan(settled);
  });
});

describe('always-mounted components do not re-render on unrelated store writes', () => {
  it('GridOverlay', () => {
    const { counter } = renderCounted(<GridOverlay map={makeMap()} gpsOrigin={null} />);
    const settled = counter.commits;
    unrelatedProjectWrites();
    expect(counter.commits, UNRELATED).toBe(settled);
    act(() => useProjectStore.setState({ stations: [makeStation(0, 1)] }));
    expect(counter.commits).toBeGreaterThan(settled);
  });

  it('CommentOverlay', () => {
    const { counter } = renderCounted(<CommentOverlay />);
    const settled = counter.commits;
    unrelatedProjectWrites();
    act(() => useProjectStore.setState({ stations: [makeStation(0, 1)] }));
    expect(counter.commits, UNRELATED).toBe(settled);
    act(() =>
      useUIStore.setState({
        commentTarget: {
          type: 'station',
          stationRef: { section_id: 1, station_index: 0 },
          screenPos: { x: 0, y: 0 },
          currentComment: '',
        },
      }),
    );
    expect(counter.commits).toBeGreaterThan(settled);
  });

  it('UndoHistoryDropdown (the toolbar undo/redo buttons)', () => {
    const { counter } = renderCounted(
      <UndoHistoryDropdown mode="undo" disabled={false} onSingleAction={() => {}} testId="undo-probe" ariaLabel="Undo">
        <span>U</span>
      </UndoHistoryDropdown>,
    );
    const settled = counter.commits;
    unrelatedProjectWrites();
    act(() => useProjectStore.setState({ stations: [makeStation(0, 1)] }));
    expect(counter.commits, UNRELATED).toBe(settled);
    act(() => useProjectStore.setState({ canUndo: true }));
    expect(counter.commits).toBeGreaterThan(settled);
  });

  it('DetailPanel', () => {
    const { counter } = renderCounted(<DetailPanel />);
    const settled = counter.commits;
    unrelatedProjectWrites();
    expect(counter.commits, UNRELATED).toBe(settled);
    act(() => useProjectStore.setState({ warnings: [] }));
    expect(counter.commits).toBeGreaterThan(settled);
  });
});
