/**
 * Component test (#110) - GPX import warning surfacing.
 *
 * Must surface the deferred-save partial-success warning ("imported but
 * unsaved"), but must NOT modal the routine per-shot validation warnings that
 * ride along on a fully-saved import. The helper's filtering logic is unit-tested
 * in utils/__tests__/saveFeedback.test.ts; this guards the dialog wiring.
 * (GPX has no orphan-placement step, so there's no ordering concern here.)
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import GpxPlacementDialog from './GpxPlacementDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult, GpxFile, ValidationWarning } from '../../types/commands';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';

const SAVE_WARNING: ValidationWarning = {
  code: 'SYSTEM',
  message:
    'Imported, but could not save the new project to disk: disk full. ' +
    'Your changes are unsaved - please save manually.',
  severity: 'warning',
};
const VALIDATION_WARNING: ValidationWarning = {
  code: 'LONG_SHOT',
  message: 'Shot 3:5 length 142.0m above maximum threshold 120.0m',
  severity: 'warning',
};

const emptyGpx: GpxFile = { name: 'cave.gpx', waypoints: [], tracks: [], routes: [] };

const RECEIPT: ImportReceipt = {
  source_format: 'GPX',
  file_name: 'cave.gpx',
  sections_found: 2,
  sections_imported: 2,
  shot_count: 0,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  zero_datum_created: null,
};

const ok = (warnings: ValidationWarning[], import_receipt?: ImportReceipt): CommandResult =>
  ({ success: true, error: null, warnings, section_ids: [], import_receipt } as CommandResult);

let executeGpxImport: ReturnType<typeof vi.fn>;
let openDialog: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeDialog = vi.fn();
  showToast = vi.fn();
  executeGpxImport = vi.fn();
  useProjectStore.setState({ executeGpxImport });
  useUIStore.setState({
    openDialog,
    closeDialog,
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
    showToast,
    dialogData: {
      gpxFile: emptyGpx,
      filePath: '/tmp/cave.gpx',
      fileHash: 'hash',
      importWaypoints: false,
      importTracks: true,
      importRoutes: true,
    },
  });
});

describe('GpxPlacementDialog warning surfacing (#110)', () => {
  it('surfaces the deferred-save warning on an otherwise-successful import', async () => {
    executeGpxImport.mockResolvedValue(ok([SAVE_WARNING]));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ title: 'Import Warning' })),
    );
  });

  it('does NOT modal a routine validation warning on a saved import', async () => {
    executeGpxImport.mockResolvedValue(ok([VALIDATION_WARNING]));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('opens no dialog when the import fully succeeds with no warnings', async () => {
    executeGpxImport.mockResolvedValue(ok([]));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(openDialog).not.toHaveBeenCalled();
  });

  it('threads the save warning into the receipt as its leading row on a deferred-save partial success (#92)', async () => {
    executeGpxImport.mockResolvedValue(ok([SAVE_WARNING], RECEIPT));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith(
        'import-receipt',
        expect.objectContaining({
          receipt: expect.objectContaining({
            warnings: [SAVE_WARNING.message, ...RECEIPT.warnings],
          }),
        }),
      ),
    );
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
  });

  it('toasts (does not modal) a warning-free receipt with something to show (#411/#420)', async () => {
    // Was "shows the receipt dialog" pre-#411: a receipt with counts but no
    // warnings has no decision in it, so it is now a toast, not a modal.
    executeGpxImport.mockResolvedValue(ok([], RECEIPT));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(showToast).toHaveBeenCalledTimes(1));
    expect(openDialog).not.toHaveBeenCalledWith('import-receipt', expect.anything());
  });

  it('opens the modal for a zero-count receipt even with no warnings (#442)', async () => {
    const empty: ImportReceipt = { ...RECEIPT, sections_imported: 0 };
    executeGpxImport.mockResolvedValue(ok([], empty));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: empty }));
    expect(showToast).not.toHaveBeenCalled();
  });

  it('keeps the modal for a unit-converted receipt even with no warnings', async () => {
    const converted: ImportReceipt = { ...RECEIPT, unit_conversion: 'Lengths converted from feet to meters' };
    executeGpxImport.mockResolvedValue(ok([], converted));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: converted }));
    expect(showToast).not.toHaveBeenCalled();
  });
});

// #576 C10: GPX now runs on the shared import runner, which reports the
// Reading -> Converting -> Applying -> Finalizing phases like the other
// importers; the dialog shows the loading overlay for them, and takes it down
// whether the import succeeds or the call throws.
describe('GpxPlacementDialog import progress (#576 C10)', () => {
  it('shows the loading overlay while the import runs', async () => {
    let finish: (r: CommandResult) => void = () => {};
    executeGpxImport.mockReturnValue(new Promise<CommandResult>((resolve) => (finish = resolve)));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(useProjectStore.getState().loading.isLoading).toBe(true));
    expect(useProjectStore.getState().loading.phase).toBe('import_parsing');
    finish(ok([], RECEIPT));
    await waitFor(() => expect(useProjectStore.getState().loading.isLoading).toBe(false));
  });

  it('takes the overlay down when the import call throws', async () => {
    executeGpxImport.mockRejectedValue(new Error('ipc gone'));

    render(<GpxPlacementDialog />);
    await userEvent.click(await screen.findByTestId('gpx-import-confirm'));

    await waitFor(() => expect(openDialog).toHaveBeenCalled());
    expect(useProjectStore.getState().loading.isLoading).toBe(false);
  });
});
