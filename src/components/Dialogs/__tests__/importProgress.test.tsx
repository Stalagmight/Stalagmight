/**
 * #576 NF-W3P-1 / RV3-FE-15: survey-file import progress where the user is looking.
 *
 * NF-W3P-1: the Compass, Walls and GPX imports report Reading -> Converting ->
 * Applying -> Finalizing through the store's `loading` state, but the only surface
 * that drew it was GlobalView's overlay - and an import into an open project runs in
 * Single System View, where that overlay is not mounted. A large file showed nothing
 * but a busy button. Each dialog now draws the progress itself, as the TML dialog
 * already did (#411 C3).
 *
 * RV3-FE-15: the GPX selection dialog's "import waypoints only" route ran the import
 * without opening the loading state at all, so the progress the backend emits for
 * every GPX import was dropped (useLoadingProgress listens only while loading).
 *
 * The real store's loading actions; the import itself is a promise the test holds.
 * `.tsx` for jsdom.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);
vi.mock('../../../utils/ipc');
invokeIpcMock.mockImplementation(async () => []);

import i18n from '../../../i18n';
import CompassImportDialog from '../CompassImportDialog';
import WallsImportDialog from '../WallsImportDialog';
import GpxPlacementDialog from '../GpxPlacementDialog';
import GpxSelectionDialog from '../GpxSelectionDialog';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import type { CompassFile, GpxFile, WallsFile } from '../../../types/commands';
import { accepted, type CommandOutcome } from '../../../store/commandResult';
import type { ImportOutcome } from '../../../types/ipc/ImportOutcome';
import { invokeIpcMock } from '../../../test/factories/ipc';

const OK = accepted<ImportOutcome>({ section_ids: [], import_receipt: null, warnings: [], remote_link: null });

let finishImport: (result: CommandOutcome<ImportOutcome>) => void;
const heldImport = () =>
  vi.fn(
    () =>
      new Promise<CommandOutcome<ImportOutcome>>((resolve) => {
        finishImport = resolve;
      })
  );

const compassFile = {
  cave_name: 'CaveA',
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
} as unknown as CompassFile;

const wallsFile = {
  project_name: 'CaveB',
  survey_count: 1,
  resolved_survey_count: 1,
  shot_count: 3,
  splay_count: 0,
  fixed_station_count: 0,
  missing_file_count: 0,
  skipped_attachment_count: 0,
  has_georeference: false,
  utm_zone: null,
  datum: null,
  // One resolved survey with shots, so Import is enabled.
  surveys: [
    { name: 'SurveyB', title: null, date: null, segment: null, resolved: true, shot_count: 3, splay_count: 0, fixed_station_count: 0 },
  ],
  warnings: [],
} as unknown as WallsFile;

const waypointsGpx: GpxFile = {
  name: 'CaveC.gpx',
  waypoints: [{ latitude: 1, longitude: 2, elevation: null, name: 'wpt-1', comment: null, time: null }],
  tracks: [],
  routes: [],
};

/** The progress the dialog shows, or null. */
const progress = (prefix: string) => screen.queryByTestId(`${prefix}-progress`);

async function expectProgressWhileImporting(prefix: string) {
  await waitFor(() => expect(progress(prefix), 'no import progress while the import runs').not.toBeNull());
  expect(screen.getByTestId(`${prefix}-progress-phase`).textContent).toBe(
    i18n.t('globalView.loading.phases.importParsing')
  );
  expect(useProjectStore.getState().loading.isLoading, 'the loading state was never opened').toBe(true);
  // A backend progress event lands on the store as it would through useLoadingProgress.
  act(() => {
    useProjectStore.getState().setLoadingProgress({ phase: 'import_applying_changes', overallProgress: 60 });
  });
  expect(screen.getByTestId(`${prefix}-progress-phase`).textContent).toBe(
    i18n.t('globalView.loading.phases.importApplyingChanges')
  );
  await act(async () => finishImport(OK));
  await waitFor(() => expect(useProjectStore.getState().loading.isLoading).toBe(false));
}

beforeEach(() => {
  useProjectStore.getState().resetLoading();
  useProjectStore.setState({
    calculateFileHash: vi.fn(async () => 'hash'),
    checkDuplicateImport: vi.fn(async () => false),
  });
  useUIStore.setState({
    openDialog: vi.fn(),
    closeDialog: vi.fn(),
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
    showToast: vi.fn(),
  });
});

afterEach(() => {
  useProjectStore.getState().resetLoading();
});

describe('survey-file import dialogs show the import progress (NF-W3P-1, RV3-FE-15)', () => {
  it('Compass', async () => {
    useProjectStore.setState({ executeCompassImport: heldImport() });
    render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/CaveA.mak" />);
    const confirm = (await screen.findByTestId('compass-import-confirm')) as HTMLButtonElement;
    await waitFor(() => expect(confirm.disabled).toBe(false));
    await userEvent.click(confirm);
    await expectProgressWhileImporting('compass-import');
  });

  it('Walls', async () => {
    useProjectStore.setState({ executeWallsImport: heldImport() });
    render(<WallsImportDialog wallsFile={wallsFile} filePath="/tmp/CaveB.wpj" />);
    const confirm = (await screen.findByTestId('walls-import-confirm')) as HTMLButtonElement;
    await waitFor(() => expect(confirm.disabled).toBe(false));
    await userEvent.click(confirm);
    await expectProgressWhileImporting('walls-import');
  });

  it('GPX, placing tracks', async () => {
    useProjectStore.setState({ executeGpxImport: heldImport() });
    useUIStore.setState({
      dialogData: {
        gpxFile: { name: 'CaveC.gpx', waypoints: [], tracks: [], routes: [] },
        filePath: '/tmp/CaveC.gpx',
        fileHash: 'hash',
        importWaypoints: false,
        importTracks: true,
        importRoutes: true,
      },
    });
    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));
    await expectProgressWhileImporting('gpx-import');
  });

  it('GPX, waypoints only', async () => {
    useProjectStore.setState({ executeGpxImport: heldImport() });
    render(<GpxSelectionDialog gpxFile={waypointsGpx} filePath="/tmp/CaveC.gpx" />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));
    await expectProgressWhileImporting('gpx-import');
  });
});
