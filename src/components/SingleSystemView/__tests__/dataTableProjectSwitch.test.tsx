/**
 * Demo finding I-12: the data table kept the previous project's line, row and
 * column when another project opened. Line ids are small per-project
 * integers, so the kept id opened the next project on whichever of its lines
 * had that id. The table now starts over on every project open
 * (`projectOpenGeneration`), and only then: an edit or refresh of the same
 * project keeps the line. Injection: rendering `ProjectDataTable` without the
 * `key` (or not moving `projectOpenGeneration` on a load) fails the first case.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import '../../../i18n';
import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { Line, Shot } from '../../../types/project';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const freshConfig = (): Config => structuredClone(defaultConfig) as Config;

function shot(id: number): Shot {
  return {
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 1,
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
  } as Shot;
}

const line = (id: number, name: string, shotIds: number[]): Line =>
  ({ id, name, color: '#e53935', shots: shotIds.map(shot) }) as unknown as Line;

const LINES = [line(0, 'Main Line', [1, 2]), line(3, 'Branch 3', [31, 32, 33])];

const pill = () => screen.getByTestId('data-table-line-pill');

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? freshConfig() : undefined),
  );
  useProjectStore.setState({ config: freshConfig(), project: null, projectOpenGeneration: 1 });
  useUIStore.setState({ toasts: [], selection: null });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('the data table across a project switch', () => {
  it('starts on the first line of a newly opened project', () => {
    render(<DataTable lines={LINES} />);
    expect(pill()).toHaveTextContent('Main Line');
    // The map selects a shot on Branch 3: the table follows it there.
    act(() => useUIStore.setState({ selection: { type: 'shot', lineId: 3, shotId: 32 } }));
    expect(pill()).toHaveTextContent('Branch 3');

    // Another project opens; it has a line 3 too.
    act(() => useUIStore.setState({ selection: null }));
    act(() => useProjectStore.setState({ projectOpenGeneration: 2 }));
    expect(pill()).toHaveTextContent('Main Line');
  });

  it('keeps the line across an edit or refresh of the same project', () => {
    render(<DataTable lines={LINES} />);
    act(() => useUIStore.setState({ selection: { type: 'shot', lineId: 3, shotId: 32 } }));
    expect(pill()).toHaveTextContent('Branch 3');
    act(() =>
      useProjectStore.setState((s) => ({ projectTreeGeneration: s.projectTreeGeneration + 1 })),
    );
    expect(pill()).toHaveTextContent('Branch 3');
  });
});
