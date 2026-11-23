/**
 * A junction's station details (UX-7, H-frontend-wire-5, RS 6): a junction
 * is ONE station several lines pass, and under RS every line shows the
 * station the merge kept. "Also on" names the other lines through it, never
 * the station itself again and never a name of their own; the stations the
 * merges archived in it are listed under "Archived here", each with the line
 * whose own station it was, so a surveyor can still find it from the notes.
 * On a line whose own station was archived (a foreign slot) the details say
 * whose station it shows; a loop end opens with what it is shown as.
 *
 * Demo finding N-3: each fact once, no Junction row. I-9: the heading reads
 * the same for every junction. Injections: putting a per-line name back on an
 * "Also on" row fails "names the other lines only"; dropping the foreign
 * sentence fails "says whose station a foreign slot shows"; dropping the
 * loop-end lead fails the loop-end case.
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
    // Records (RS 3): history ones here, naming no line.
    archived: [{ id: 20, name: '0', lines: [] }, { id: 21, name: '', lines: [] }],
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
 * RS 4.1: Side's own "0" (Station ID 20) was archived by the merge, so Side
 * shows the kept station "3a" (Station ID 11) too, with Main's dot: its slot
 * is foreign.
 */
function keptStations(): Station[] {
  const list = stations();
  const archived = [{ id: 20, name: '0', lines: [SIDE] }];
  list[1] = { ...list[1]!, archived };
  list[3] = { ...list[3]!, archived, shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false } };
  return list;
}

describe('a junction that shows its kept station on every line', () => {
  beforeEach(() => {
    const { project } = useProjectStore.getState();
    const list = keptStations();
    useProjectStore.setState({ stations: list, surveyData: surveyDataFromLegacy(project!.lines, list) });
  });

  it('says whose station a foreign slot shows, and the archived station of its line', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(SIDE, 0) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:3a$/);
    expect(screen.getByTestId('station-detail-belongs')).toHaveTextContent('3a belongs to Main.');
    expect(screen.getByTestId('station-detail-line')).toHaveTextContent(/^Line:Side$/);
    expect(screen.getByTestId('station-detail-id')).toHaveTextContent('11');
    expect(screen.getByTestId('station-detail-record-20')).toHaveTextContent("0 (Station ID 20), Side's own station");
  });

  it('names the other lines only, each with its detach button (RS 6)', () => {
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:3a$/);
    expect(screen.queryByTestId('station-detail-belongs')).toBeNull();
    const block = screen.getByTestId('station-detail-merged-with');
    expect(block).toHaveTextContent(/^Also on:Side$/);
    expect(screen.getByTestId(`station-${SIDE}-0-link`)).toHaveTextContent('Side');
    expect(screen.getByTestId(`detail-unmerge-${SIDE}-0`)).toHaveAccessibleName('Detach Side from this station');
  });

  it('says each fact once: no Junction row, the archived station listed once (N-3)', () => {
    render(<DetailPanel />);
    const panel = screen.getByTestId('station-detail');
    expect(screen.queryByTestId('station-detail-junction')).toBeNull();
    expect(panel.textContent).not.toMatch(/Junction:|Merged with/);
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

  it('shows no other lines for a station on one line', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 2) });
    render(<DetailPanel />);
    expect(screen.queryByTestId('station-detail-merged-with')).toBeNull();
    expect(screen.getByTestId('station-detail-name')).toHaveTextContent(/^Name:4$/);
  });
});

describe('a loop end in the station details (RS 6, U-P2-15)', () => {
  it('opens with what it is shown as, and its own station hidden', () => {
    // Side 1 (Station ID 22, "1") is a loop end: closure 5 shows it as Main / 4.
    const { project } = useProjectStore.getState();
    const list = stations();
    list[4] = { ...list[4]!, name: '4', shown: { id: 12, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 5n } };
    const loop_closures = [
      {
        id: 5n, station_a: ref(MAIN, 2), station_b: ref(SIDE, 1), path_length: 10, misclosure_distance: 0.5,
        error_percentage: 5, horizontal_misclosure: 0.5, vertical_misclosure: 0, strategy: 'Batch', is_aligned: false,
        archived: { end: 'b', station: { id: 22, name: '1', home_line: SIDE }, shown_as: { id: 12, name: '4', home_line: MAIN } },
      },
    ];
    useProjectStore.setState({
      project: { ...project!, loop_closures } as unknown as Project,
      stations: list,
      surveyData: surveyDataFromLegacy(project!.lines, list),
    });
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(SIDE, 1) });
    render(<DetailPanel />);
    expect(screen.getByTestId('station-detail-loop-end')).toHaveTextContent('Loop closure end, shown as Main / 4');
    expect(screen.getByTestId('station-detail-loop-end-own')).toHaveTextContent(
      'Its own station 1 (Station ID 22) is hidden. Removing the closure brings it back.',
    );
    expect(screen.getByTestId('station-detail-id')).toHaveTextContent('12');
    expect(screen.queryByTestId('station-detail-belongs')).toBeNull();
  });
});

describe('a junction made by Split Line Here (I-9)', () => {
  it('reads like every other junction: "Also on", never "Merged with"', () => {
    // Both halves show the split station by one name and one Station ID:
    // nothing archived.
    const list = stations().map((s) => {
      const plain = { ...s };
      delete plain.archived;
      return plain;
    });
    useProjectStore.setState({ stations: list, surveyData: surveyDataFromLegacy(useProjectStore.getState().project!.lines, list) });
    render(<DetailPanel />);
    const block = screen.getByTestId('station-detail-merged-with');
    expect(block).toHaveTextContent(/^Also on:/);
    expect(block.textContent).not.toContain('Merged');
    expect(block).toHaveTextContent(/^Also on:Side$/);
    expect(screen.queryByTestId('station-detail-records')).toBeNull();
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

  it('lists the stations archived in it under "Archived here"', () => {
    render(<DetailPanel />);
    const records = screen.getByTestId('station-detail-records');
    expect(records).toHaveTextContent('Archived here');
    expect(screen.getByTestId('station-detail-record-20')).toHaveTextContent('0 (Station ID 20), kept for its Station ID');
    expect(screen.getByTestId('station-detail-record-21')).toHaveTextContent('21 (Station ID 21), kept for its Station ID');
  });

  it('a station with nothing archived shows no "Archived here"', () => {
    useUIStore.getState().setSelection({ type: 'station', stationRef: ref(MAIN, 2) });
    render(<DetailPanel />);
    expect(screen.queryByTestId('station-detail-records')).toBeNull();
  });

  it('a binary-loaded station carries its records through the SoA', () => {
    const data = useProjectStore.getState().surveyData!;
    const records = [{ id: 20, name: '0', lines: [] }, { id: 21, name: '', lines: [] }];
    expect(data.stationArchived.get(1)).toEqual(records);
    expect(data.stationArchived.get(3)).toEqual(records);
    expect(data.stationArchived.has(0)).toBe(false);
  });
});
