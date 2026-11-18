/**
 * #411 grid-unparseable-draft-spec sibling: the create-project screen and the
 * TML import dialog must quote the SAME line/shot counts for the same file.
 * Both now read `computeTmlImportStats` (src/utils/tmlImportStats.ts) instead
 * of each hand-rolling its own reduce, so a fixture with excluded shots, a
 * closure row, and a line that empties out under default filtering has to
 * land on identical numbers on both screens.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, screen } from '@testing-library/react';

// The TML dialog gained a map column in #425; LinePreview and the panel pull
// maplibre-gl (WebGL) at module load, which jsdom cannot execute. This suite is
// about the two screens' NUMBERS, nothing drawn.
vi.mock('../LinePreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../LinePreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 480,
  MAP_PANEL_MIN_WIDTH: 280,
  MAP_PANEL_MAX_WIDTH: 900,
}));

import NewProjectForImportDialog from '../NewProjectForImportDialog';
import TmlImportDialog from '../TmlImportDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { TmlFile, TmlSection, TmlShot } from '../../../types/commands';

const shot = (overrides: Partial<TmlShot> = {}): TmlShot =>
  ({
    id: 1n,
    from_id: 0n,
    shot_type: 'Real',
    azimuth: 0,
    length: 10,
    depth: 2,
    depth_in: 1,
    inclination: 0,
    left: 1,
    right: 1,
    up: 1,
    down: 1,
    comment: null,
    explorer: null,
    date: null,
    color: '#3b82f6',
    excluded: false,
    latitude: 0,
    longitude: 0,
    closure_to_id: 0n,
    line: 'S',
    ...overrides,
  }) as TmlShot;

const line = (name: string, shots: TmlShot[]): TmlSection => ({
  name,
  shots,
  surveyor: null,
  date: null,
  color: '#3b82f6',
  is_branch_split: false,
  start_ariane_id: 0n,
  end_ariane_id: 0n,
  start_depth: 0,
});

// Line A: two ordinary shots -> both count.
// Line B: a single EXCLUDED shot -> empties out under the default (exclude
// excluded shots), so it must not be counted as a landed line.
// Line C: a CLOSURE row (never imported) plus one ordinary shot -> the
// closure is dropped, the shot counts.
const tmlFile: TmlFile = {
  cave_name: 'Test Cave',
  unit: 'Metric',
  use_magnetic_azimuth: false,
  first_start_absolute_elevation: 0,
  geo_coding: null,
  lines: [
    line('A', [shot({ id: 1n }), shot({ id: 2n })]),
    line('B', [shot({ id: 3n, excluded: true })]),
    line('C', [shot({ id: 4n, shot_type: 'Closure' }), shot({ id: 5n })]),
  ],
  closures: [{ from_line_index: 0, from_station_index: 0, to_line_index: 2, to_station_index: 1 }],
};

// Expected under the DEFAULT toggles (includeExcluded: false): line B
// empties out, so 2 lines / 3 shots land.
const EXPECTED_LINES = 2;
const EXPECTED_SHOTS = 3;

describe('TML import summary consistency (#411)', () => {
  beforeEach(() => {
    useUIStore.setState({
      dialogData: { tmlFile, filePath: '/tmp/cave.tml', importType: 'tml', defaultName: 'Cave' },
      openDialog: () => {},
      closeDialog: () => {},
      setImportContext: () => {},
    });
    useProjectStore.setState({
      createProject: async () => ({ success: true, error: null, warnings: [], line_ids: [] }),
      calculateFileHash: async () => 'hash',
      startLoading: () => 0,
      setLoadingProgress: () => {},
      resetLoading: () => {},
      loading: useProjectStore.getState().loading,
    });
  });

  it('create-project screen quotes the same counts the import dialog lands on', () => {
    const { unmount } = render(<NewProjectForImportDialog />);
    const summary = screen.getByText(new RegExp(`^${EXPECTED_LINES} lines - ${EXPECTED_SHOTS} shots`));
    expect(summary).toBeInTheDocument();
    unmount();

    render(<TmlImportDialog tmlFile={tmlFile} filePath="/tmp/cave.tml" />);
    expect(screen.getByTestId('tml-section-count')).toHaveTextContent(String(EXPECTED_LINES));
    expect(screen.getByTestId('tml-shot-count')).toHaveTextContent(String(EXPECTED_SHOTS));
  });
});
