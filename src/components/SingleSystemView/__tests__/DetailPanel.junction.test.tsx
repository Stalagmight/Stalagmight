/**
 * A junction's station details (UX-7, H-frontend-wire-5): a junction is ONE
 * station several lines pass, so "Merged with" names the other lines through
 * it, never the station itself again ("3a | Side" read as merged with
 * itself); and the ids and names merged into it (its aliases) show as "Also
 * known as", so a surveyor can still find a retired name from the notes.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import DetailPanel from '../DetailPanel';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Project, Line, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

function shot(id: number): Shot {
  return {
    id, azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 1, depth_out: 2,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}

function line(id: number, name: string, shots: number[]): Line {
  return { id, name, color: '#336699', direction: 'in', quality: 1, next_shot_id: 9, shots: shots.map(shot) } as unknown as Line;
}

const MAIN = 1;
const SIDE = 2;
const ref = (line_id: number, station_index: number) => ({ line_id, station_index });

/** Main 0-1-2 and Side 0-1; Main:1 and Side:0 are one station, named "3a",
 *  into which Side's former "0" (Station ID 20) was merged. */
function stations(): Station[] {
  const plain = (line_id: number, i: number, id: number, name: string): Station => ({
    station_ref: ref(line_id, i), coordinates: { x: i, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id, name,
  });
  const junction = (at: ReturnType<typeof ref>, other: ReturnType<typeof ref>): Station => ({
    ...plain(at.line_id, at.station_index, 11, '3a'),
    merged_with: [other],
    aliases: [{ id: 20, name: '0' }, { id: 21, name: null }],
  });
  return [
    plain(MAIN, 0, 10, '2'),
    junction(ref(MAIN, 1), ref(SIDE, 0)),
    plain(MAIN, 2, 12, '4'),
    junction(ref(SIDE, 0), ref(MAIN, 1)),
    plain(SIDE, 1, 22, '1'),
  ];
}

beforeEach(() => {
  const lines = [line(MAIN, 'Main', [101, 102]), line(SIDE, 'Side', [201])];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = stations();
  useProjectStore.setState({
    project, stations: list, surveyData: surveyDataFromLegacy(lines, list),
    warnings: [], stats: null, config: null,
  });
  useUIStore.setState({ detailPanelTab: 'details', appMode: 'survey', geometryMode: 'measured', multiSelection: [] });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 1) });
});

/**
 * I-3: a line merged onto a station keeps its own name for it. Side shows the
 * junction (Station ID 11) as its own "0" (the alias naming Side), Main as
 * "3a"; the details say which name is on which line.
 */
function perLineStations(): Station[] {
  const list = stations();
  const aliases = [{ id: 20, name: '0', line: SIDE }];
  list[1] = { ...list[1]!, aliases };
  list[3] = { ...list[3]!, name: '0', aliases };
  return list;
}

describe('a junction each line shows by its own name', () => {
  beforeEach(() => {
    const { project } = useProjectStore.getState();
    const list = perLineStations();
    useProjectStore.setState({ stations: list, surveyData: surveyDataFromLegacy(project!.lines, list) });
  });

  it("names the slot by its line's name, with the line, and lists the other lines' names", () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(SIDE, 0) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent('Name:0 (on Side)');
    expect(screen.getByTestId('station-detail-junction')).toHaveTextContent('Junction:3a on Main');
    expect(screen.getByTestId('station-detail-id')).toHaveTextContent('11');
  });

  it('reads the same from the other line', () => {
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent('Name:3a (on Main)');
    expect(screen.getByTestId('station-detail-junction')).toHaveTextContent('Junction:0 on Side');
  });

  it('says which line knew it by an alias', () => {
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-alias-20')).toHaveTextContent('0 on Side (Station ID 20)');
  });

  it('shows no "(on line)" when every line shows the junction by one name', () => {
    useProjectStore.setState({ stations: stations(), surveyData: surveyDataFromLegacy(useProjectStore.getState().project!.lines, stations()) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:3a$/);
    expect(screen.getByTestId('station-detail-junction')).toHaveTextContent('Junction:3a on Side');
  });

  it('shows no Junction row for a station on one line', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 2) });
    render(<DetailPanel />);
    expect(screen.queryByTestId('station-detail-junction')).toBeNull();
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:4$/);
  });
});

describe('junction station details', () => {
  it('"Merged with" names the other line, never the station itself', () => {
    render(<DetailPanel />);
    const merged = screen.getByTestId('station-detail-merged-with');
    const link = screen.getByTestId(`station-${SIDE}-0-link`);
    expect(link).toHaveTextContent('Side');
    expect(link.textContent).not.toContain('3a');
    expect(merged.textContent).not.toContain('3a');
    // The accessible name still says which slot it selects.
    expect(link).toHaveAccessibleName(/Side/);
  });

  it('shows the names and Station IDs merged into it as "Also known as"', () => {
    render(<DetailPanel />);
    const aliases = screen.getByTestId('station-detail-aliases');
    expect(aliases).toHaveTextContent('Also known as');
    expect(screen.getByTestId('station-detail-alias-20')).toHaveTextContent('0 (Station ID 20)');
    expect(screen.getByTestId('station-detail-alias-21')).toHaveTextContent('Station ID 21');
  });

  it('a station with no aliases shows no "Also known as"', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 2) });
    render(<DetailPanel />);
    expect(screen.queryByTestId('station-detail-aliases')).toBeNull();
  });

  it('a binary-loaded station carries its aliases through the SoA', () => {
    const data = useProjectStore.getState().surveyData!;
    expect(data.stationAliases.get(1)).toEqual([{ id: 20, name: '0' }, { id: 21, name: null }]);
    expect(data.stationAliases.get(3)).toEqual([{ id: 20, name: '0' }, { id: 21, name: null }]);
    expect(data.stationAliases.has(0)).toBe(false);
  });
});
