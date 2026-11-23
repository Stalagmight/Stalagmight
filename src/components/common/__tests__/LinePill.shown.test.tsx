/**
 * RS 6: pills show the identity a slot shows. A station pill on a foreign
 * slot (its line knew another station there; the junction shows the kept
 * one) ends with the shown Station ID and carries the home line's dot; a loop
 * end carries the loop mark, even with no dot. Shot pills carry each foreign
 * end's dot. The plain text below Full says whose station it is.
 * Injections: reading the node id for the superscript fails "ends with the
 * shown Station ID"; dropping the default home mark fails "carries the home
 * line's dot".
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));

import '../../../i18n';
import { DetailLink } from '../DetailLink';
import { StationBlob } from '../EntityBlob';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import type { Line, Project, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';

const MAIN = 1;
const SIDE = 2;

function shot(id: number): Shot {
  return {
    id, azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 1, depth_out: 2,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}

function line(id: number, name: string, color: string, shots: number[]): Line {
  return { id, name, color, direction: 'in', quality: 1, next_shot_id: 9, shots: shots.map(shot) } as unknown as Line;
}

function station(line_id: number, station_index: number, id: number, name: string, extra: Partial<Station> = {}): Station {
  return {
    station_ref: { line_id, station_index }, coordinates: { x: station_index, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id, name, ...extra,
  };
}

beforeEach(() => {
  const lines = [line(MAIN, 'Main', '#ffff00', [101, 102]), line(SIDE, 'Side', 'navy', [201, 202])];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = [
    station(MAIN, 0, 3, '3'),
    station(MAIN, 1, 11, '12', { merged_with: [{ line_id: SIDE, station_index: 0 }] }),
    station(MAIN, 2, 13, '13'),
    // Side 0 shows the junction's kept station Main / 12 (node 11).
    station(SIDE, 0, 11, '12', {
      merged_with: [{ line_id: MAIN, station_index: 1 }],
      shown: { id: 11, home_line: MAIN, foreign: true, loop_end: false },
    }),
    station(SIDE, 1, 21, '1'),
    // Side 2 (node 22) is a loop end shown as Main / 3.
    station(SIDE, 2, 22, '3', { shown: { id: 3, home_line: MAIN, foreign: true, loop_end: true, loop_closure: 9n } }),
  ];
  useProjectStore.setState({ project, stations: list, surveyData: surveyDataFromLegacy(lines, list) });
  useUIStore.setState({ syntacticLevel: 'full', resolvedTheme: 'dark' });
});

describe('station pills of foreign slots and loop ends (RS 6)', () => {
  it("carries the home line's dot on a foreign slot, none on a native one", () => {
    render(
      <>
        <StationBlob station={{ line_id: SIDE, station_index: 0 }} testId="foreign" />
        <StationBlob station={{ line_id: MAIN, station_index: 1 }} testId="native" />
      </>,
    );
    expect(screen.getByTestId('foreign-home-dot')).toBeInTheDocument();
    expect(screen.queryByTestId('native-home-dot')).toBeNull();
    expect(screen.getByTestId('foreign-station-id')).toHaveTextContent('11');
  });

  it('ends a loop end with the shown Station ID and the loop mark', () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 2 }} testId="loop" />);
    expect(screen.getByTestId('loop-station-id')).toHaveTextContent('3');
    expect(screen.getByTestId('loop-station-id')).not.toHaveTextContent('22');
    expect(screen.getByTestId('loop-loop-end')).toHaveTextContent('loop end');
    expect(screen.getByTestId('loop-home-dot')).toBeInTheDocument();
  });

  it('lets a caller say no dot', () => {
    render(<StationBlob station={{ line_id: SIDE, station_index: 0 }} homeLineId={null} testId="none" />);
    expect(screen.queryByTestId('none-home-dot')).toBeNull();
  });

  it('says whose station it is in plain text below Full (U-P3-8)', () => {
    useUIStore.setState({ syntacticLevel: 'off' });
    render(
      <>
        <span data-testid="plain-foreign"><StationBlob station={{ line_id: SIDE, station_index: 0 }} withLine /></span>
        <span data-testid="plain-loop"><StationBlob station={{ line_id: SIDE, station_index: 2 }} withLine /></span>
      </>,
    );
    expect(screen.getByTestId('plain-foreign')).toHaveTextContent('Side / 12 (Main)');
    expect(screen.getByTestId('plain-loop')).toHaveTextContent('Main / 3 (loop end on Side)');
  });

  it('gives a station link the same: shown ID, dot, loop mark, and a spoken whose', () => {
    render(
      <DetailLink
        type="station"
        target={{ lineId: SIDE, position: 2 }}
        displayText="3"
        fullText="Main / 3 (loop end on Side)"
        onSelect={() => {}}
      />,
    );
    const id = `station-${SIDE}-2-link`;
    expect(screen.getByTestId(`${id}-station-id`)).toHaveTextContent('3');
    expect(screen.getByTestId(`${id}-loop-end`)).toBeInTheDocument();
    expect(screen.getByTestId(`${id}-home-dot`)).toBeInTheDocument();
    expect(screen.getByTestId(id)).toHaveAccessibleName('Select station Main / 3 (loop end on Side), Station ID 3');
  });

  it("gives a shot link each foreign end's dot", () => {
    // Side's first shot runs from the foreign junction slot to Side 1.
    render(<DetailLink type="shot" target={{ lineId: SIDE, position: 1 }} displayText="12 → 1" onSelect={() => {}} />);
    const id = `shot-${SIDE}-1-link`;
    expect(screen.getByTestId(`${id}-from-home-dot`)).toBeInTheDocument();
    expect(screen.queryByTestId(`${id}-to-home-dot`)).toBeNull();
  });
});
