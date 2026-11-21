/**
 * Demo finding I-10: in a file a newer Stalagmight made (open read-only), the
 * station menu's edit rows are disabled but looked almost exactly like enabled
 * ones, and nothing said why. A disabled row now fades its icon and label
 * (`opacity-50`, as a disabled `Button`), and a row the read-only gate
 * disabled says why in its tooltip. Injection: dropping `faded` from
 * `MenuRow`, or `reason` from the gate, fails the first case.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import { MenuRow } from '../MenuRow';

vi.mock('../../../utils/combineLines', () => ({
  queryCombineCandidates: () => new Promise(() => {}),
  queryCombineCandidateForLines: () => new Promise(() => {}),
  combinePreview: () => ({ lines: [], stations: [] }),
}));

let dialogData: Record<string, unknown> = {};
let fileReadOnlyReason: string | null = null;
const openDialog = vi.fn();
const uiState = () => ({
  closeDialog: vi.fn(),
  dialogData,
  openDialog,
  setActiveTool: vi.fn(),
  setMergeStationA: vi.fn(),
  triggerZoomToLine: vi.fn(),
  multiSelection: [],
  geometryMode: 'measured',
  appMode: 'survey',
  syntacticLevel: 'icons',
});
vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign((selector: (s: ReturnType<typeof uiState>) => unknown) => selector(uiState()), {
    getState: () => uiState(),
  }),
}));

const shot = (id: number) => ({ id, length: 5, azimuth_in: 0, azimuth_out: 0, depth_in: 0, depth_out: 0 });
const slot = (line_id: number, station_index: number, merged: Array<{ line_id: number; station_index: number }>) => ({
  station_ref: { line_id, station_index },
  coordinates: { x: 0, y: 0, z: 0 },
  is_zero_datum: false,
  gps_coordinates: null,
  merged_with: merged,
  closures_with: [],
  id: 7,
  name: '2',
});
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: () => ({
    project: {
      lines: [
        { id: 1, name: 'Main', shots: [shot(1), shot(2), shot(3)] },
        { id: 2, name: 'Side', shots: [shot(4)] },
      ],
      station_merges: [{ station_a: { line_id: 1, station_index: 2 }, station_b: { line_id: 2, station_index: 0 } }],
    },
    // Main's station 2 is Side's start: one station, two slots.
    stations: [slot(1, 2, [{ line_id: 2, station_index: 0 }]), slot(2, 0, [{ line_id: 1, station_index: 2 }])],
    setShotKind: vi.fn(),
    unmergeStations: vi.fn(),
    unmergeStationPair: vi.fn(),
    removeLoopClosure: vi.fn(),
    fileReadOnlyReason,
  }),
}));
vi.mock('../../../store/gasPlanStore', () => ({
  useGasPlanStore: { getState: () => ({ reachField: null, lastReachField: null }) },
}));
vi.mock('../../../store/speleoStore', () => ({
  useSpeleoStore: () => false,
  selectSpeleoReadOnly: () => false,
}));
vi.mock('../../../hooks/useDismissableLayer', () => ({ useDismissableLayer: () => {} }));
vi.mock('../../../utils/telemetry', () => ({
  setSource: vi.fn(),
  trackContextMenuAction: vi.fn(),
  trackDialogComplete: vi.fn(),
}));
vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (key: string) => key }),
}));

import ContextMenu from '../ContextMenu';

function openOn(lineId: number, stationIndex: number) {
  dialogData = {
    target: { type: 'station', ref: { line_id: lineId, station_index: stationIndex } },
    position: { x: 10, y: 10 },
    survivesScroll: true,
  };
  render(<ContextMenu />);
}

const row = (name: string) => screen.getByRole('button', { name });
const label = (button: HTMLElement, name: string) =>
  Array.from(button.querySelectorAll('span')).find((s) => s.textContent === name)!;

describe('ContextMenu rows in a read-only file', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    fileReadOnlyReason = null;
  });

  it('fades the disabled edit rows and says why on hover', () => {
    fileReadOnlyReason = 'requires zz-future';
    openOn(1, 2);
    const detach = row('Detach Line from Station...');
    expect(detach).toBeDisabled();
    expect(label(detach, 'Detach Line from Station...').className).toContain('opacity-50');
    expect(detach.getAttribute('title')).toBe('editGate.readOnlyFileNotice');
  });

  it('leaves an editable file\'s rows as they were', () => {
    openOn(1, 2);
    const detach = row('Detach Line from Station...');
    expect(detach).toBeEnabled();
    expect(label(detach, 'Detach Line from Station...').className).not.toContain('opacity-50');
    expect(detach.getAttribute('title')).toBeNull();
  });
});

describe('MenuRow disabled look', () => {
  it('fades the icon and label but keeps the hint that says why', () => {
    render(
      <MenuRow label="Combine" hint="Not at a line end" icon={<svg data-testid="icon" />} showIcons disabled />,
    );
    const button = screen.getByRole('button', { name: 'Combine' });
    expect(label(button, 'Combine').className).toContain('opacity-50');
    expect(screen.getByTestId('icon').parentElement!.className).toContain('opacity-50');
    expect(label(button, 'Not at a line end').className).not.toContain('opacity-50');
  });
});
