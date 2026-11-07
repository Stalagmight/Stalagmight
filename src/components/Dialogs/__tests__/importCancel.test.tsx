/**
 * RV4-FE-MISC1 (#576 NF-FIX3A-2): the footer Cancel of every survey-file import
 * dialog stops a RUNNING import.
 *
 * The backend registers a file import as the active load and checks its token
 * between phases (`run_file_import`), so `cancel_loading` stops it before anything
 * is applied, and the command then fails with `IMPORT_CANCELLED: ...`. No button a
 * user could press reached that: TML and GPX placement disabled Cancel while the
 * import ran, and Compass, Walls and GPX selection closed the dialog while the
 * import kept running in the backend and landed anyway.
 *
 * For each dialog, with its import command held open:
 *  (a) Cancel stays enabled; pressing it invokes `cancel_loading` once (a second
 *      press does nothing) and does not close the dialog;
 *  (b) the command then failing IMPORT_CANCELLED ends through the cancelled path:
 *      the router's quiet toast and close (for TML on the empty project the import
 *      created, the keep/discard prompt the pre-import Cancel shows);
 *  (c) a success that lands after the cancel (it arrived past the last cancellable
 *      point) completes the import normally;
 *  (d) Escape mid-import does not close the dialog either (GPX placement keeps its
 *      Escape-closes behaviour, so it is not in this one);
 *  (e) after a move-carry refusal (nothing ran, the dialog stays for the retry) the
 *      footer is the idle Cancel again - TML, Compass and Walls used to stay in
 *      their importing state there, which with a live Cancel would be a trap.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
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
// The IPC boundary: `cancel_loading` is what the backend sees of a Cancel press,
// and the orphan lookups after a successful import resolve to "nothing to offer".
vi.mock('../../../utils/ipc');

import TmlImportDialog from '../TmlImportDialog';
import CompassImportDialog from '../CompassImportDialog';
import WallsImportDialog from '../WallsImportDialog';
import GpxSelectionDialog from '../GpxSelectionDialog';
import GpxPlacementDialog from '../GpxPlacementDialog';
import { beginMoveCarry, refuseMutationDuringCarry } from '../../../store/moveCarry';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { refusedBy } from '../../../store/commandResult';
import type {
  CommandResult,
  CompassFile,
  GpxFile,
  TmlFile,
  TmlShot,
  WallsFile,
} from '../../../types/commands';
import type { Project } from '../../../types/project';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { deferred } from '../../../test/factories/async';

// --- fully synthetic files, one importable shot / waypoint each ----------------

const tmlShot: TmlShot = {
  id: 1,
  from_id: 0,
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
  closure_to_id: 0,
  section: 'S',
} as TmlShot;

const tmlFile: TmlFile = {
  cave_name: 'Test Cave',
  unit: 'Metric',
  use_magnetic_azimuth: false,
  first_start_absolute_elevation: 0,
  geo_coding: null,
  sections: [
    {
      name: 'S',
      shots: [tmlShot],
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
};

const compassFile: CompassFile = {
  cave_name: 'Test Cave',
  survey_count: 0,
  shot_count: 0,
  excluded_shot_count: 0,
  invalid_shot_counts: {
    non_excluded_corrected: 0,
    non_excluded_uncorrected: 0,
    excluded_corrected: 0,
    excluded_uncorrected: 0,
  },
  has_backsights: false,
  datum: null,
  utm_zone: null,
  has_georeference: false,
  declination_mode: 'File',
  length_unit: null,
  surveys: [],
  warnings: [],
};

const wallsFile: WallsFile = {
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
  surveys: [
    {
      name: 'SurveyA',
      title: 'Survey A',
      date: '2026-01-01',
      segment: '',
      shot_count: 1,
      splay_count: 0,
      fixed_station_count: 0,
      resolved: true,
    },
  ],
  warnings: [],
};

const waypointsOnlyGpx: GpxFile = {
  name: 'waypoints.gpx',
  waypoints: [{ latitude: 1, longitude: 2, elevation: null, name: 'wpt-1', comment: null, time: null }],
  tracks: [],
  routes: [],
};

// --- the cases ------------------------------------------------------------------

type ImportAction = 'executeTmlImport' | 'executeCompassImport' | 'executeWallsImport' | 'executeGpxImport';

interface DialogCase {
  name: string;
  action: ImportAction;
  /** The `label` the backend's runner puts in its cancelled message. */
  format: string;
  dialogId: string;
  confirmId: string;
  cancelId: string;
  /** What a landed import answers (Walls routes an empty one to its error dialog). */
  sectionIds: number[];
  render: () => void;
}

const TML_CASE: DialogCase = {
  name: 'TmlImportDialog',
  action: 'executeTmlImport',
  format: 'TML',
  dialogId: 'tml-import-dialog',
  confirmId: 'tml-import-confirm',
  cancelId: 'tml-import-cancel',
  sectionIds: [],
  render: () => render(<TmlImportDialog tmlFile={tmlFile} filePath="/tmp/cave.tml" />),
};

const CASES: DialogCase[] = [
  TML_CASE,
  {
    name: 'CompassImportDialog',
    action: 'executeCompassImport',
    format: 'Compass',
    dialogId: 'compass-import-dialog',
    confirmId: 'compass-import-confirm',
    cancelId: 'compass-import-cancel',
    sectionIds: [],
    render: () => render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.mak" />),
  },
  {
    name: 'WallsImportDialog',
    action: 'executeWallsImport',
    format: 'Walls',
    dialogId: 'walls-import-dialog',
    confirmId: 'walls-import-confirm',
    cancelId: 'walls-import-cancel',
    sectionIds: [1],
    render: () => render(<WallsImportDialog wallsFile={wallsFile} filePath="/tmp/cave.wpj" />),
  },
  {
    name: 'GpxSelectionDialog',
    action: 'executeGpxImport',
    format: 'GPX',
    dialogId: 'gpx-selection-dialog',
    confirmId: 'gpx-import-confirm',
    cancelId: 'gpx-selection-cancel',
    sectionIds: [],
    render: () => render(<GpxSelectionDialog gpxFile={waypointsOnlyGpx} filePath="/tmp/waypoints.gpx" />),
  },
  {
    name: 'GpxPlacementDialog',
    action: 'executeGpxImport',
    format: 'GPX',
    dialogId: 'gpx-placement-dialog',
    confirmId: 'gpx-import-confirm',
    cancelId: 'gpx-placement-cancel',
    sectionIds: [],
    render: () => render(<GpxPlacementDialog />),
  },
];

/** The dialogs whose Escape / close button must not close mid-import. */
const ESCAPE_GUARDED = CASES.filter((c) => c.name !== 'GpxPlacementDialog');

const CANCELLED_TOAST = 'Import cancelled. Nothing was imported.';

/** The store's refused outcome for a cancelled import: the backend's IMPORT_CANCELLED (#576 C4). */
const cancelledResult = (format: string): CommandResult =>
  refusedBy({
    code: 'IMPORT_CANCELLED',
    message: `the ${format} import was cancelled; nothing was imported`,
  }) as unknown as CommandResult;

const okResult = (sectionIds: number[]): CommandResult =>
  ({ success: true, error: null, warnings: [], section_ids: sectionIds }) as CommandResult;

let importMocks: Record<ImportAction, ReturnType<typeof vi.fn>>;
let openDialog: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let setView: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeDialog = vi.fn();
  setView = vi.fn();
  showToast = vi.fn();
  importMocks = {
    executeTmlImport: vi.fn(),
    executeCompassImport: vi.fn(),
    executeWallsImport: vi.fn(),
    executeGpxImport: vi.fn(),
  };
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation(async (cmd: string) => {
    if (cmd === 'suggest_orphan_placements') return [];
    if (cmd === 'count_unanchored_sections') return 0;
    return null; // cancel_loading answers nothing
  });
  useProjectStore.setState({
    ...importMocks,
    calculateFileHash: vi.fn(async () => 'hash'),
    checkDuplicateImport: vi.fn(async () => false),
    startLoading: vi.fn(() => 0),
    setLoadingProgress: vi.fn(),
    resetLoading: vi.fn(),
    databaseOpen: false,
    project: null,
  });
  useUIStore.setState({
    openDialog,
    closeDialog,
    setView,
    triggerFitToBounds: vi.fn(),
    showToast,
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

const cancelLoadingCalls = () => invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'cancel_loading').length;

/** Render the dialog and start its import, held open on `pending`. */
async function startHeldImport(c: DialogCase) {
  const pending = deferred<CommandResult>();
  importMocks[c.action].mockReturnValue(pending.promise);
  c.render();
  const confirm = (await screen.findByTestId(c.confirmId)) as HTMLButtonElement;
  await waitFor(() => expect(confirm.disabled).toBe(false));
  await userEvent.click(confirm);
  expect(importMocks[c.action]).toHaveBeenCalledTimes(1);
  return pending;
}

const cancelButton = (c: DialogCase) => screen.getByTestId(c.cancelId) as HTMLButtonElement;

describe.each(CASES)('$name Cancel during a running import (RV4-FE-MISC1)', (c) => {
  it('(a) stays enabled, asks the backend to stop once, and keeps the dialog open', async () => {
    await startHeldImport(c);

    expect(cancelButton(c).disabled).toBe(false);
    await userEvent.click(cancelButton(c));

    expect(cancelLoadingCalls()).toBe(1);
    expect(closeDialog).not.toHaveBeenCalled();
    expect(screen.getByTestId(c.dialogId)).toBeInTheDocument();
    // The request is in: the button says so and a second press does nothing.
    expect(cancelButton(c)).toHaveTextContent('Cancelling...');
    expect(cancelButton(c).disabled).toBe(true);
    await userEvent.click(cancelButton(c));
    expect(cancelLoadingCalls()).toBe(1);
  });

  it('(b) ends through the cancelled path when the import answers IMPORT_CANCELLED', async () => {
    const pending = await startHeldImport(c);
    await userEvent.click(cancelButton(c));
    expect(cancelLoadingCalls()).toBe(1);
    expect(closeDialog).not.toHaveBeenCalled(); // open until the command settles

    await act(async () => pending.resolve(cancelledResult(c.format)));

    await waitFor(() => expect(closeDialog).toHaveBeenCalledTimes(1));
    expect(showToast).toHaveBeenCalledWith(CANCELLED_TOAST, 'import-cancelled', 'info');
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
    expect(setView).not.toHaveBeenCalled();
  });

  it('(c) completes normally when the import lands despite the cancel', async () => {
    const pending = await startHeldImport(c);
    await userEvent.click(cancelButton(c));
    expect(cancelLoadingCalls()).toBe(1);

    await act(async () => pending.resolve(okResult(c.sectionIds)));

    await waitFor(() => expect(setView).toHaveBeenCalledWith('single'));
    expect(closeDialog).toHaveBeenCalledTimes(1);
    expect(showToast).not.toHaveBeenCalledWith(CANCELLED_TOAST, expect.anything(), expect.anything());
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
  });
});

describe.each(ESCAPE_GUARDED)('$name Escape during a running import (RV4-FE-MISC1)', (c) => {
  it('(d) does not close the dialog, and does not cancel either', async () => {
    await startHeldImport(c);

    fireEvent.keyDown(screen.getByTestId(c.dialogId), { key: 'Escape' });
    await userEvent.click(screen.getByTestId(`${c.dialogId}-close`));

    expect(closeDialog).not.toHaveBeenCalled();
    expect(cancelLoadingCalls()).toBe(0);
    expect(screen.getByTestId(c.dialogId)).toBeInTheDocument();
  });
});

describe.each(CASES)('$name after a move-carry refusal (GHOST-2)', (c) => {
  it('(e) goes back to the idle Cancel, which closes without asking the backend', async () => {
    // A real refusal: an import issued while a move carry is in flight.
    beginMoveCarry(0, { cancel: vi.fn(), restorePreview: vi.fn() });
    const refused = refuseMutationDuringCarry();
    expect(refused).not.toBeNull();
    importMocks[c.action].mockResolvedValue(refused);
    c.render();
    const confirm = (await screen.findByTestId(c.confirmId)) as HTMLButtonElement;
    await waitFor(() => expect(confirm.disabled).toBe(false));
    await userEvent.click(confirm);

    await waitFor(() => expect(confirm.disabled).toBe(false)); // ready for the retry
    expect(cancelButton(c).textContent).toBe('Cancel');
    await userEvent.click(cancelButton(c));
    expect(cancelLoadingCalls()).toBe(0);
    expect(closeDialog).toHaveBeenCalledTimes(1);
  });
});

describe('TmlImportDialog cancelled on the empty project it created (#411 C2)', () => {
  it('asks keep/discard exactly as the pre-import Cancel does, without the IMPORT_CANCELLED text', async () => {
    useProjectStore.setState({
      project: { sections: [], created_at: '2026-08-05T00:00:00Z' } as unknown as Project,
    });
    useUIStore.setState({ importContext: 'new-project' });
    const tml = TML_CASE;
    const pending = await startHeldImport(tml);
    await userEvent.click(cancelButton(tml));
    expect(cancelLoadingCalls()).toBe(1);

    await act(async () => pending.resolve(cancelledResult(tml.format)));

    expect(await screen.findByTestId('tml-import-residue-dialog')).toBeInTheDocument();
    // The 'cancel' prompt carries no error block; 'failed' would quote the raw text.
    expect(screen.queryByTestId('tml-import-residue-error')).toBeNull();
    expect(screen.queryByText(/IMPORT_CANCELLED/)).toBeNull();
    expect(screen.queryByText(/import was cancelled; nothing was imported/)).toBeNull();
    expect(closeDialog).not.toHaveBeenCalled();
    expect(showToast).not.toHaveBeenCalled();
  });
});
