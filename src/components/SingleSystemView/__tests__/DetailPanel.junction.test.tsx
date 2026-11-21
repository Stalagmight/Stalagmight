/**
 * A junction's station details (UX-7, H-frontend-wire-5): a junction is ONE
 * station several lines pass, so "Also on" names the other lines through
 * it, never the station itself again ("3a | Side" read as merged with
 * itself); and the ids and names merged into it (its aliases) show as "Also
 * known as", so a surveyor can still find a retired name from the notes.
 *
 * Demo finding N-3: each fact once. The details said one fact three times
 * ("Junction: 0 on North Passage", "Merged with: North Passage", "Also known
 * as: 0 on North Passage (Station ID 9)"). Now each "Also on" row names the
 * other line, the name it shows the station by and the Station ID it had
 * there when that differs (`North Passage 0 (Station ID 9)`); there is no
 * Junction row, and "Also known as" lists only the aliases no row shows.
 * I-9: the heading reads the same for every junction ("Merged with" was
 * wrong on a Split Line Here junction). Injections: putting the Junction row
 * back fails "says each fact once"; dropping the `shown` filter from
 * `StationAliases` fails it too; dropping the Station ID from a row fails
 * "names each other line with its own name and Station ID".
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

  it("names the slot by its line's name, and each other line by its own name", () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(SIDE, 0) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:0$/);
    expect(screen.getByTestId('station-detail-line')).toHaveTextContent(/^Line:Side$/);
    expect(screen.getByTestId('station-detail-id')).toHaveTextContent('11');
    // Main shows the junction by its own name and Station ID: nothing more to say.
    expect(screen.getByTestId(`station-detail-other-name-${MAIN}-1`)).toHaveTextContent(/^3a$/);
    // Side's own former Station ID: its name is on the Name row already.
    expect(screen.getByTestId('station-detail-alias-20')).toHaveTextContent(/^Station ID 20 on Side$/);
  });

  it('names each other line with its own name and Station ID (N-3)', () => {
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:3a$/);
    const block = screen.getByTestId('station-detail-merged-with');
    expect(block).toHaveTextContent(/^Also on:/);
    expect(screen.getByTestId(`station-${SIDE}-0-link`)).toHaveTextContent('Side');
    expect(screen.getByTestId(`station-detail-other-name-${SIDE}-0`)).toHaveTextContent(/^0 \(Station ID 20\)$/);
    expect(screen.getByTestId(`detail-unmerge-${SIDE}-0`)).toHaveAccessibleName('Detach Side from this station');
  });

  it('says each fact once: no Junction row, no alias a row already shows (N-3)', () => {
    render(<DetailPanel />);
    const panel = screen.getByTestId('station-detail');
    expect(screen.queryByTestId('station-detail-junction')).toBeNull();
    expect(panel.textContent).not.toMatch(/Junction:|Merged with/);
    // Side's alias is on its "Also on" row, so "Also known as" has nothing left.
    expect(screen.queryByTestId('station-detail-aliases')).toBeNull();
    expect(panel.textContent?.match(/Station ID 20/g)).toHaveLength(1);
  });

  // Demo finding V-8: the line was named three times, "3 (on Main Line)",
  // "Line: Main Line" and a Connected "Line: Main Line" link. It is named
  // once, by the Line row, which is the link. Injection: put "(on line)"
  // back on the Name row, or the Line row back under Connected, and this
  // fails.
  it('names the station\'s line once, in the Line row, which links to it (V-8)', () => {
    render(<DetailPanel />);
    const panel = screen.getByTestId('station-detail');
    expect(panel.textContent?.match(/Main(?! Line)/g)).toHaveLength(1);
    const line = screen.getByTestId('station-detail-line');
    expect(line).toHaveTextContent(/^Line:Main$/);
    expect(line.querySelector(`[data-testid="line-${MAIN}-link"]`)).not.toBeNull();
    expect(screen.getAllByTestId(`line-${MAIN}-link`)).toHaveLength(1);
  });

  it('shows no "(on line)" when every line shows the junction by one name', () => {
    useProjectStore.setState({ stations: stations(), surveyData: surveyDataFromLegacy(useProjectStore.getState().project!.lines, stations()) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:3a$/);
    // Side shows it as the Name row says: its row names the line only.
    expect(screen.queryByTestId(`station-detail-other-name-${SIDE}-0`)).toBeNull();
  });

  it('shows no other lines for a station on one line', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 2) });
    render(<DetailPanel />);
    expect(screen.queryByTestId('station-detail-merged-with')).toBeNull();
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:4$/);
  });
});

describe('a junction made by Split Line Here (I-9)', () => {
  it('reads like every other junction: "Also on", never "Merged with"', () => {
    // Both halves show the split station by one name and one Station ID: no
    // alias, nothing merged.
    const list = stations().map((s) => {
      const plain = { ...s };
      delete plain.aliases;
      return plain;
    });
    useProjectStore.setState({ stations: list, surveyData: surveyDataFromLegacy(useProjectStore.getState().project!.lines, list) });
    render(<DetailPanel />);
    const block = screen.getByTestId('station-detail-merged-with');
    expect(block).toHaveTextContent(/^Also on:/);
    expect(block.textContent).not.toContain('Merged');
    expect(block).toHaveTextContent(/^Also on:Side$/);
    expect(screen.queryByTestId(`station-detail-other-name-${SIDE}-0`)).toBeNull();
    expect(screen.queryByTestId('station-detail-aliases')).toBeNull();
  });
});

describe('a joined comment in the station details (N-9)', () => {
  it('keeps its line breaks', () => {
    const list = stations().map((s) => ({ ...s, comment: 'T-junction, line arrow points out\n\nSilt bank, low visibility' }));
    useProjectStore.setState({ stations: list, surveyData: surveyDataFromLegacy(useProjectStore.getState().project!.lines, list) });
    render(<DetailPanel />);
    const comment = screen.getByTestId('station-detail-comment');
    expect(comment.textContent).toBe('T-junction, line arrow points out\n\nSilt bank, low visibility');
    expect(comment).toHaveClass('whitespace-pre-wrap');
  });
});

describe('junction station details', () => {
  it('"Also on" names the other line, never the station itself', () => {
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
    // No line knows the station by them, so no "Also on" row shows them.
    expect(screen.queryByTestId(`station-detail-other-name-${SIDE}-0`)).toBeNull();
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
