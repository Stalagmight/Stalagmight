/**
 * RS-PROSE: the Warnings tab draws every name a warning's message says. A
 * survey message opens with its shot (`Shot Main: 0 → 1 ...`), which the
 * tab has always drawn as its own shot pill; the rest of the message (the
 * body) now draws the stations and lines it names from the message's parts
 * (`ProjectWarning.messageParts`), and a message that does not open with the
 * shot draws all of its names. A screen reader reads the message's words.
 * Below Full, and without parts, the row reads exactly as before.
 * Injections: render `body` as a string again and the body draws no pill;
 * drop the whole-message branch and the unconnected line draws none.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => []) }));
// jsdom lays nothing out: the warnings list's virtualizer renders every row.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () => Array.from({ length: count }, (_, index) => ({ index, key: index, start: index * 56, size: 56 })),
    getTotalSize: () => count * 56,
    measureElement: () => {},
    scrollToIndex: () => {},
  }),
}));

import '../../../i18n';
import DetailPanel from '../DetailPanel';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { surveyDataFromLegacy } from '../../../types/survey-data';
import { accessibleText } from '../../../test/accessibleText';
import { partsToPlain } from '../../../utils/prose';
import { warningMessageTestId, warningOpenShotTestId } from '../../../utils/testId';
import type { Project, Line, Shot } from '../../../types/project';
import type { Station } from '../../../types/ipc/Station';
import type { ProjectWarning } from '../../../types/commands';
import type { ProsePart } from '../../../types/ipc/ProsePart';

const MAIN = 1;

function shot(id: number): Shot {
  return {
    id, azimuth_in: 90, azimuth_out: 90, length: 5, depth_in: 1, depth_out: 2,
    pitch_in: null, pitch_out: null, left: null, right: null, up: null, down: null,
    comment: null, kind: 'Normal', marker_index: null, temperature: null,
  } as Shot;
}

function station(i: number): Station {
  return {
    station_ref: { line_id: MAIN, station_index: i }, coordinates: { x: i, y: 0, z: 0 }, is_zero_datum: false,
    gps_coordinates: null, merged_with: [], id: 10 + i, name: String(i),
  };
}

const end = (i: number): ProsePart => ({
  kind: 'station', text: `Main / ${i}`, line_id: MAIN, context: 'Main', name: String(i), station_id: 10 + i,
});

const acrossParts: ProsePart[] = [
  { kind: 'text', text: 'Shot ' },
  { kind: 'shot', text: 'Main: 0 → 1', line_id: MAIN, context: 'Main', from: { name: '0' }, to: { name: '1' } },
  { kind: 'text', text: ' has both ends joined by loop closure ' },
  end(0),
  { kind: 'text', text: ' to ' },
  end(1),
  { kind: 'text', text: ', which would squeeze the shot to zero length.' },
];

const unconnectedParts: ProsePart[] = [
  { kind: 'text', text: 'Line ' },
  { kind: 'line', text: '"Main"', line_id: MAIN, name: 'Main' },
  { kind: 'text', text: ' is not connected to the rest of the survey and has no zero datum.' },
];

function warning(shotIndex: number, code: string, parts: ProsePart[] | null, message?: string): ProjectWarning {
  const w: ProjectWarning = {
    lineId: MAIN, shotId: 101 + shotIndex, shotIndex, code,
    message: message ?? partsToPlain(parts ?? []),
    severity: 'warning', isResolved: false,
  };
  if (parts) w.messageParts = parts;
  return w;
}

const across = warning(0, 'CLOSURE_ACROSS_ONE_SHOT', acrossParts);
const unconnected = warning(1, 'SECTION_NOT_CONNECTED', unconnectedParts);
const plainShort = warning(1, 'SHORT_SHOT', null, 'Shot Main: 1 → 2 length 0.1m below minimum threshold 0.2m');

beforeEach(() => {
  const lines = [{ id: MAIN, name: 'Main', color: '#336699', direction: 'in', quality: 1, next_shot_id: 9, shots: [101, 102].map(shot) } as unknown as Line];
  const project = { lines, station_merges: [], zero_datums: [], loop_closures: [] } as unknown as Project;
  const list = [station(0), station(1), station(2)];
  useProjectStore.setState({
    project, stations: list, surveyData: surveyDataFromLegacy(lines, list),
    warnings: [across, unconnected, plainShort], stats: null, config: null,
  });
  useUIStore.setState({
    detailPanelTab: 'warnings', appMode: 'survey', geometryMode: 'measured', multiSelection: [],
    syntacticLevel: 'full', resolvedTheme: 'dark',
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

const kinds = (el: Element) => [...el.querySelectorAll('.sh-ppill')].map((p) => p.getAttribute('data-prose-kind'));

describe('Warnings tab prose (RS-PROSE)', () => {
  it('draws the names in the body after the shot pill and reads the message', () => {
    render(<DetailPanel />);
    const row = screen.getByTestId(warningMessageTestId(across));
    // The shot keeps its own pill (a button to its details); the body's two
    // closure ends are prose pills.
    expect(screen.getByTestId(warningOpenShotTestId(across))).toBeInTheDocument();
    expect(kinds(row)).toEqual(['station', 'station']);
    expect(accessibleText(row)).toContain(across.message.slice('Shot '.length + 'Main: 0 → 1'.length));
    expect(accessibleText(row)).toContain('has both ends joined by loop closure Main / 0 to Main / 1, which');
  });

  it('draws every name of a message that does not open with its shot', () => {
    render(<DetailPanel />);
    const row = screen.getByTestId(warningMessageTestId(unconnected));
    expect(kinds(row)).toEqual(['line']);
    expect(accessibleText(row)).toBe(unconnected.message);
  });

  it('reads a message without parts as before', () => {
    render(<DetailPanel />);
    const row = screen.getByTestId(warningMessageTestId(plainShort));
    expect(kinds(row)).toEqual([]);
    expect(row.textContent).toContain('length 0.1m below minimum threshold 0.2m');
    expect(screen.getByTestId(warningOpenShotTestId(plainShort))).toBeInTheDocument();
  });

  it('is the plain message below Full', () => {
    useUIStore.setState({ syntacticLevel: 'off' });
    render(<DetailPanel />);
    for (const w of [across, unconnected]) {
      const row = screen.getByTestId(warningMessageTestId(w));
      expect(row.querySelector('.sh-ppill')).toBeNull();
      expect(row.textContent).toContain(w.message);
    }
  });
});
