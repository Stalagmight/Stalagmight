/**
 * Every survey-file import dialog's Cancel names the import its command was sent
 * with (#576 W44-NF3; checked for the Codex review on #630).
 *
 * The claim checked: the TML, Compass, Walls and GPX dialogs call `startLoading`
 * with no token, so `ImportCancelButton` would read none from the loading state and
 * send `cancel_loading` with `loadToken: null`, which the backend applies to no load.
 * The store's import actions name the import as they issue it (`nameImport`,
 * projectStore.ts) and write that name into the loading state, after the dialog's
 * `startLoading` cleared it.
 *
 * Dialog tier with the REAL store actions (`startLoading`, the `execute*Import`
 * actions, `executeCommand`): only the IPC boundary is faked, so the name is traced
 * from the import command's arguments to the Cancel's. `importCancel.test.tsx` stubs
 * those actions, so it cannot see this. All data is synthetic.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);
// The TML dialog's map column pulls maplibre-gl (WebGL) at module load, which jsdom
// cannot execute; nothing drawn matters here.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 480,
  MAP_PANEL_MIN_WIDTH: 280,
  MAP_PANEL_MAX_WIDTH: 900,
}));
vi.mock('../../../utils/ipc');

import TmlImportDialog from '../TmlImportDialog';
import CompassImportDialog from '../CompassImportDialog';
import WallsImportDialog from '../WallsImportDialog';
import GpxSelectionDialog from '../GpxSelectionDialog';
import GpxPlacementDialog from '../GpxPlacementDialog';
import { useProjectStore } from '../../../store/projectStore';
import { initialLoadingState } from '../../../store/projectLoadingState';
import { mutationsSettled, runSerialized } from '../../../store/mutationQueue';
import { useUIStore } from '../../../store/uiStore';
import type { CompassFile, GpxFile, TmlFile, WallsFile } from '../../../types/commands';
import { emptyBinaryRefresh, invokeIpcMock } from '../../../test/factories/ipc';
import { deferred } from '../../../test/factories/async';

// --- fully synthetic files ------------------------------------------------------

const tmlFile = {
  cave_name: 'Test Cave',
  unit: 'Metric',
  use_magnetic_azimuth: false,
  first_start_absolute_elevation: 0,
  geo_coding: null,
  sections: [
    {
      name: 'S',
      shots: [{ id: 1, from_id: 0, shot_type: 'Real', azimuth: 0, length: 10, depth: 2, depth_in: 1, inclination: 0, left: 1, right: 1, up: 1, down: 1, comment: null, explorer: null, date: null, color: '#3b82f6', excluded: false, latitude: 0, longitude: 0, closure_to_id: 0, section: 'S' }],
      surveyor: null,
      date: null,
      color: '#3b82f6',
      is_branch_split: false,
      start_ariane_id: 0,
      end_ariane_id: 0,
      start_depth: 0,
    },
  ],
  closures: [],
} as unknown as TmlFile;

const compassFile = {
  cave_name: 'Test Cave',
  survey_count: 0,
  shot_count: 0,
  excluded_shot_count: 0,
  invalid_shot_counts: { non_excluded_corrected: 0, non_excluded_uncorrected: 0, excluded_corrected: 0, excluded_uncorrected: 0 },
  has_backsights: false,
  datum: null,
  utm_zone: null,
  has_georeference: false,
  declination_mode: 'File',
  length_unit: null,
  surveys: [],
  warnings: [],
} as unknown as CompassFile;

const wallsFile = {
  project_name: 'CaveA',
  book_count: 1,
  survey_count: 1,
  resolved_survey_count: 1,
  shot_count: 1,
  splay_count: 0,
  has_georeference: false,
  datum: null,
  utm_zone: null,
  fixed_station_count: 0,
  missing_file_count: 0,
  skipped_attachment_count: 0,
  any_feet_units: false,
  surveys: [{ name: 'SurveyA', title: 'Survey A', date: '2026-01-01', segment: '', shot_count: 1, splay_count: 0, fixed_station_count: 0, resolved: true }],
  warnings: [],
} as unknown as WallsFile;

const waypointsOnlyGpx: GpxFile = {
  name: 'waypoints.gpx',
  waypoints: [{ latitude: 1, longitude: 2, elevation: null, name: 'wpt-1', comment: null, time: null }],
  tracks: [],
  routes: [],
};

interface DialogCase {
  name: string;
  /** The import command the dialog's store action sends. */
  command: string;
  confirmId: string;
  cancelId: string;
  render: () => void;
}

const CASES: DialogCase[] = [
  { name: 'TmlImportDialog', command: 'execute_tml_import', confirmId: 'tml-import-confirm', cancelId: 'tml-import-cancel', render: () => render(<TmlImportDialog tmlFile={tmlFile} filePath="/tmp/cave.tml" />) },
  { name: 'CompassImportDialog', command: 'execute_compass_import', confirmId: 'compass-import-confirm', cancelId: 'compass-import-cancel', render: () => render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.mak" />) },
  { name: 'WallsImportDialog', command: 'execute_walls_import', confirmId: 'walls-import-confirm', cancelId: 'walls-import-cancel', render: () => render(<WallsImportDialog wallsFile={wallsFile} filePath="/tmp/cave.wpj" />) },
  { name: 'GpxSelectionDialog', command: 'execute_gpx_import', confirmId: 'gpx-import-confirm', cancelId: 'gpx-selection-cancel', render: () => render(<GpxSelectionDialog gpxFile={waypointsOnlyGpx} filePath="/tmp/waypoints.gpx" />) },
  { name: 'GpxPlacementDialog', command: 'execute_gpx_import', confirmId: 'gpx-import-confirm', cancelId: 'gpx-placement-cancel', render: () => render(<GpxPlacementDialog />) },
];

/** What the backend answers a cancelled import (import_runner.rs `cancelled`). */
const CANCELLED = { code: 'IMPORT_CANCELLED', message: 'the import was cancelled; nothing was imported' };

// A refused import still refreshes the project; answer that refresh with "no project open".
emptyBinaryRefresh();

let closeDialog: ReturnType<typeof vi.fn>;
/** What each case holds open, settled after it however it ended. */
let releases: Array<() => void>;

/** The import command's IPC, held until the test answers it. */
function holdImport(command: string) {
  const sent = deferred<Record<string, unknown> | undefined>();
  const answer = deferred<never>();
  releases.push(() => answer.reject(CANCELLED));
  invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
    if (cmd === command) {
      sent.resolve(args);
      return answer.promise;
    }
    if (cmd === 'suggest_orphan_placements') return [];
    if (cmd === 'count_unanchored_sections') return 0;
    return null; // cancel_loading answers nothing
  });
  return { sent, answer };
}

const cancelsSent = () =>
  invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'cancel_loading').map(([, args]) => args as unknown);

async function pressImport(c: DialogCase) {
  c.render();
  const confirm = (await screen.findByTestId(c.confirmId)) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  await userEvent.click(confirm);
}

beforeEach(() => {
  closeDialog = vi.fn();
  releases = [];
  invokeIpcMock.mockReset();
  useProjectStore.setState({
    calculateFileHash: vi.fn(async () => 'hash'),
    checkDuplicateImport: vi.fn(async () => false),
    loading: { ...initialLoadingState },
    databaseOpen: false,
    project: null,
  });
  useUIStore.setState({
    openDialog: vi.fn(),
    closeDialog,
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
    showToast: vi.fn(),
    importContext: null,
    // GpxPlacementDialog reads its step-1 answers from the dialog slot.
    dialogData: {
      gpxFile: { name: 'track.gpx', waypoints: [], tracks: [], routes: [] } satisfies GpxFile,
      filePath: '/tmp/track.gpx',
      fileHash: 'hash',
      importWaypoints: false,
      importTracks: true,
      importRoutes: true,
    },
  });
});

// A case that failed half-way must not leave its import holding the edit queue for the next.
afterEach(async () => {
  for (const release of releases) release();
  await act(async () => {
    await mutationsSettled();
  });
});

describe.each(CASES)('$name: Cancel during a running import', (c) => {
  it('names the import its command was sent with', async () => {
    const { sent, answer } = holdImport(c.command);
    await pressImport(c);
    const importName = (await sent.promise)?.loadToken;

    await userEvent.click(screen.getByTestId(c.cancelId));
    const cancels = cancelsSent();
    console.log(`[${c.name}] ${c.command} loadToken=${String(importName)}; cancel_loading sent ${JSON.stringify(cancels)}`);

    expect(typeof importName).toBe('string');
    expect(cancels).toEqual([{ loadToken: importName }]);

    // The import then ends as the cancelled import.
    await act(async () => answer.reject(CANCELLED));
    await waitFor(() => expect(closeDialog).toHaveBeenCalledTimes(1));
  });
});

describe('TmlImportDialog: Cancel while the import still waits in the edit queue', () => {
  // The window the backend half of the Codex #630 review covers: the Cancel is on
  // screen, and its IPC leaves, before the import's own IPC does.
  it('goes out before the import command, naming the import', async () => {
    const tml = CASES[0]!;
    const { sent, answer } = holdImport(tml.command);
    const earlierEdit = deferred<void>();
    releases.push(() => earlierEdit.resolve());
    void runSerialized(() => earlierEdit.promise);

    await pressImport(tml);
    await userEvent.click(screen.getByTestId(tml.cancelId));
    const issued = invokeIpcMock.mock.calls.map(([cmd]) => cmd as string);
    const cancels = cancelsSent();
    const pendingName = useProjectStore.getState().loading.loadToken;
    console.log(`[queued import] IPC sent so far ${JSON.stringify(issued)}; cancel_loading ${JSON.stringify(cancels)}`);
    expect(issued).not.toContain(tml.command);
    expect(typeof pendingName).toBe('string');
    expect(cancels).toEqual([{ loadToken: pendingName }]);

    // The earlier edit finishes; the import's IPC leaves, under the name its Cancel used.
    earlierEdit.resolve();
    expect((await sent.promise)?.loadToken).toBe(pendingName);
    await act(async () => answer.reject(CANCELLED));
    await waitFor(() => expect(closeDialog).toHaveBeenCalledTimes(1));
  });
});
