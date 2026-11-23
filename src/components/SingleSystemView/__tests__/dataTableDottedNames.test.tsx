/**
 * Dotted slots don't collide (RS-FX5, the user's decision of 2026-11-23):
 * Main / 12 is kept at a junction with Branch, so Branch's start shows "12"
 * with Main's dot, and Branch's own 13th station keeps its surveyed name "12";
 * no letter. The data table's stations view shows both "12"s on Branch: the
 * Name column reads the shown name of each, the dotted one followed by a
 * muted "(Main)" (RS-FX6, P2-5), the gutter pill of the foreign slot carries
 * Main's home dot (which reads "(Main)" to a screen reader) and the native
 * one none, and the Station ID column reads the two different shown Station
 * IDs. Below Full, the gutter's plain text says whose station the foreign
 * slot shows.
 *
 * Injection: a gutter pill named by the qualified label ("12 (Main)") fails
 * "shows both 12s"; dropping the default home mark fails it too.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../utils/ipc')>()),
  invokeIpc: invokeIpcMock,
}));

// Every row rendered: jsdom has no layout for the virtualizer to measure.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));

import '../../../i18n';
import DataTable from '../DataTable';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { Line, Project, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const MAIN = 1;
const BRANCH = 3;
/** Main / 12's Station ID: the junction node Branch's start shows. */
const MAIN12 = 112;
/** Branch's own station 12 (its 13th). */
const BRANCH12 = 312;

function shot(id: number): Shot {
  return {
    id, azimuth_in: 0, azimuth_out: 0, length: 2, depth_in: 0, depth_out: 0,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}

const line = (id: number, name: string, color: string, shotIds: number[]): Line =>
  ({ id, name, color, direction: 'in', quality: 1, next_shot_id: 99, shots: shotIds.map(shot) }) as unknown as Line;

function station(line_id: number, station_index: number, id: number, name: string, extra: Partial<Station> = {}): Station {
  return {
    station_ref: { line_id, station_index }, coordinates: { x: station_index, y: line_id, z: 0 },
    is_zero_datum: false, gps_coordinates: null, merged_with: [], id, name, ...extra,
  };
}

/** Main: 11, 12 (the junction). Branch: 0 is the foreign slot of Main / 12,
 *  1..12 its own stations, 12 named "12" as surveyed. */
const LINES = [
  line(MAIN, 'Main', '#ffff00', [1]),
  line(BRANCH, 'Branch', 'navy', Array.from({ length: 12 }, (_, k) => 300 + k)),
];
const STATIONS: Station[] = [
  station(MAIN, 0, 111, '11'),
  station(MAIN, 1, MAIN12, '12', { merged_with: [{ line_id: BRANCH, station_index: 0 }] }),
  station(BRANCH, 0, MAIN12, '12', {
    merged_with: [{ line_id: MAIN, station_index: 1 }],
    shown: { id: MAIN12, home_line: MAIN, foreign: true, loop_end: false },
  }),
  ...Array.from({ length: 12 }, (_, k) =>
    station(BRANCH, k + 1, k + 1 === 12 ? BRANCH12 : 300 + k + 1, k + 1 === 12 ? '12' : `B${k + 1}`),
  ),
];

const freshConfig = (): Config => structuredClone(defaultConfig) as Config;

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? freshConfig() : undefined),
  );
  const project = { lines: LINES, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  useProjectStore.setState({
    config: freshConfig(),
    project,
    stations: STATIONS,
    surveyData: surveyDataFromLegacy(LINES, STATIONS),
    projectOpenGeneration: 1,
  });
  useUIStore.setState({ toasts: [], selection: null, syntacticLevel: 'full', resolvedTheme: 'dark', geometryMode: 'measured' });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

/** The data table on Branch's stations, with the (default hidden) Name column shown. */
function openBranchStations() {
  render(<DataTable lines={LINES} />);
  act(() => useUIStore.setState({ selection: { type: 'station', stationRef: { line_id: BRANCH, station_index: 0 } } }));
  fireEvent.click(screen.getByTestId('data-table-columns-menu'));
  fireEvent.click(screen.getByTestId('col-vis-name'));
}

const cell = (index: number, key: string) => screen.getByTestId(`station-cell-${BRANCH}-${index}-${key}`);
const pill = (index: number) => `data-table-station-pill-${BRANCH}-${index}`;

describe('two "12"s on one line, one of them dotted (RS-FX5)', () => {
  it('shows both 12s: the Name cells, the dot on the foreign slot only, two Station IDs', () => {
    openBranchStations();
    expect(cell(0, 'name')).toHaveTextContent(/^12 \(Main\)$/);
    expect(cell(12, 'name')).toHaveTextContent(/^12$/);

    // The gutter pills name both bare; the foreign slot's carries Main's dot.
    expect(screen.getByTestId(pill(0))).toHaveTextContent(/^12\s*\(Main\)\s*Station ID\s*112$/);
    expect(screen.getByTestId(pill(12))).toHaveTextContent(/^12\s*Station ID\s*312$/);
    expect(screen.getByTestId(`${pill(0)}-home-dot`)).toBeInTheDocument();
    expect(screen.queryByTestId(`${pill(12)}-home-dot`)).toBeNull();

    // The Station ID column reads each slot's shown Station ID.
    expect(cell(0, '_stationId')).toHaveTextContent(/^112$/);
    expect(cell(12, '_stationId')).toHaveTextContent(/^312$/);
  });

  it("tells them apart in the gutter's tooltip and in plain text below Full", () => {
    openBranchStations();
    const label = (index: number) => screen.getByTestId(`data-table-station-row-label-${BRANCH}-${index}`);
    expect(label(0)).toHaveAttribute('title', 'Branch / 12 (Main)');
    expect(label(12)).toHaveAttribute('title', 'Branch / 12');

    act(() => useUIStore.setState({ syntacticLevel: 'off' }));
    expect(within(label(0)).getByText('12 (Main)')).toBeInTheDocument();
    expect(label(12)).toHaveTextContent(/^12$/);
  });
});
