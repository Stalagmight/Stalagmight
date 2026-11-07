/**
 * Component test (#107) - OrphanPlacementDialog completion wiring.
 *
 * This is the bug-prone "completing X must call Y" path behind #92 (data loss):
 * when the orphan-placement step finishes it MUST persist the import via
 * `saveAfterImport`, and on a FAILED save it must keep the dialog open (so the
 * error stays visible) rather than closing as if the import succeeded.
 *
 * The store-level contract of `saveAfterImport` itself is covered by
 * projectStore.saveAfterImport.test.ts; this renders the real dialog and asserts
 * the UI action is actually wired to it - the layer that was previously
 * untestable (no component tier) and shipped #92 on manual verification alone.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// SectionPreview imports maplibre-gl (WebGL) at module load, which jsdom cannot
// execute. Stub it - this test is about completion wiring, not the preview.
vi.mock('./SectionPreview', () => ({ default: () => null }));
// Telemetry is fire-and-forget; keep it from reaching Tauri IPC under jsdom.
vi.mock('../../utils/telemetry', async () => (await import('../../test/factories/telemetry')).telemetryWith());

import OrphanPlacementDialog from './OrphanPlacementDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { OrphanPlacementSuggestion } from '../../types/commands';
import { accepted, type CommandOutcome } from '../../store/commandResult';
import type { EditOutcome } from '../../types/ipc/EditOutcome';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';

const ok = async (): Promise<CommandOutcome<EditOutcome>> => accepted<EditOutcome>({ warnings: [], remote_link: null });

/** A minimal, valid suggestion. `matchedShots: 0` skips the shot-rewrite branch
 *  (applyOverlapTreatment returns early), keeping the test focused on the
 *  accept → persist → close wiring. */
function makeSuggestion(over: Partial<OrphanPlacementSuggestion> = {}): OrphanPlacementSuggestion {
  return {
    orphanSectionId: 1,
    orphanSectionName: 'Orphan',
    orphanStationIndex: 0,
    targetSectionId: 2,
    targetSectionName: 'Mainline',
    targetStationIndex: 1,
    score: 0.9,
    matchedShots: 0,
    existingMatchedShots: 0,
    preferredMergeOffset: 0,
    direction: 'forward',
    isPartial: false,
    confidence: 'high',
    ...over,
  };
}

let saveAfterImport: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let refreshProjectState: ReturnType<typeof vi.fn>;

beforeEach(() => {
  saveAfterImport = vi.fn(async () => true);
  closeDialog = vi.fn();
  refreshProjectState = vi.fn(async () => {});
  // Override every store method the dialog touches so nothing reaches real IPC.
  // No afterEach restore: vitest isolates module state per file and beforeEach
  // re-seeds, so a trailing finish() (the dialog can call it twice) always hits
  // a mock rather than a restored real method mid-unmount.
  useProjectStore.setState({
    project: null,
    mergeStations: vi.fn(ok),
    updateShot: vi.fn(ok),
    setShotKind: vi.fn(ok),
    refreshProjectState,
    saveAfterImport,
  });
  useUIStore.setState({
    closeDialog,
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
  });
});

describe('OrphanPlacementDialog - completion persists the import (#92 / #107)', () => {
  it('accepting the last suggestion saves and closes the dialog', async () => {
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    // closeDialog is finish()'s terminal step - waiting for it guarantees the
    // whole chain ran (refresh → save → close), avoiding a race where we assert
    // before the awaited save resolves. (finish() can run twice - from
    // handleAccept and the isFinished effect - so assert "called", not a count.)
    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(refreshProjectState).toHaveBeenCalled();
    expect(saveAfterImport).toHaveBeenCalled();
  });

  it('keeps the dialog open when the save fails - no phantom success (#92)', async () => {
    saveAfterImport.mockResolvedValue(false);

    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() => expect(saveAfterImport).toHaveBeenCalled());
    // saveAfterImport resolved false. Flush a full macrotask so finish()'s
    // post-await continuation runs to completion - a regression that closed the
    // dialog *after* awaiting the failed save would fire here and be caught,
    // rather than racing past this assertion.
    await new Promise((resolve) => setTimeout(resolve, 0));
    // The save failed, so the dialog must NOT close (the error dialog raised by
    // saveAfterImport stays visible instead of being dismissed).
    expect(closeDialog).not.toHaveBeenCalled();
  });
});

describe('OrphanPlacementDialog - receipt surface routing (#411/#420)', () => {
  const RECEIPT: ImportReceipt = {
    source_format: 'mNemo DMP',
    file_name: 'cave.dmp',
    sections_found: 1,
    sections_imported: 1,
    shot_count: 12,
    closures_requested: 0,
    closures_created: 0,
    unit_conversion: null,
    warnings: [],
    info: [],
    zero_datum_created: null,
  };

  let openDialog: ReturnType<typeof vi.fn>;
  let showToast: ReturnType<typeof vi.fn>;

  beforeEach(() => {
    openDialog = vi.fn();
    showToast = vi.fn();
    useUIStore.setState({ openDialog, showToast });
  });

  it('toasts a warning-free receipt once orphan placement finishes', async () => {
    // finish() can run twice (handleAccept + the isFinished effect, see the
    // completion-wiring tests above) - assert "called", not a count.
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} receipt={RECEIPT} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() => expect(showToast).toHaveBeenCalled());
    expect(openDialog).not.toHaveBeenCalledWith('import-receipt', expect.anything());
  });

  it('opens the modal when the receipt carries a warning', async () => {
    const withWarning: ImportReceipt = { ...RECEIPT, warnings: ['1 shot skipped'] };
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} receipt={withWarning} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: withWarning }),
    );
    expect(showToast).not.toHaveBeenCalled();
  });

  it('opens the modal for a zero-count receipt even with no warnings (#442)', async () => {
    const empty: ImportReceipt = { ...RECEIPT, sections_imported: 0, shot_count: 0 };
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} receipt={empty} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() => expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: empty }));
    expect(showToast).not.toHaveBeenCalled();
  });

  it('opens the modal when sections_imported is zero even if shot_count disagrees (#442)', async () => {
    const disagreeing: ImportReceipt = { ...RECEIPT, sections_imported: 0, shot_count: 12 };
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} receipt={disagreeing} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: disagreeing }),
    );
    expect(showToast).not.toHaveBeenCalled();
  });

  it('keeps the modal for a unit-converted receipt even with no warnings', async () => {
    const converted: ImportReceipt = { ...RECEIPT, unit_conversion: 'Lengths converted from feet to meters' };
    render(<OrphanPlacementDialog suggestions={[makeSuggestion()]} receipt={converted} />);

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: converted }),
    );
    expect(showToast).not.toHaveBeenCalled();
  });
});

describe('OrphanPlacementDialog - the overlap picker while an Accept runs (#576 HFI2-F6)', () => {
  it('disables the overlap-treatment picker until the Accept finishes, like the Accept button', async () => {
    // The Accept captured the mode at click; a change made while it runs would
    // not apply to it and would carry over to the next suggestion unannounced.
    let release: () => void = () => {};
    const merging = new Promise<CommandOutcome<EditOutcome>>((resolve) => {
      release = () => resolve(accepted<EditOutcome>({ warnings: [], remote_link: null }));
    });
    useProjectStore.setState({ mergeStations: vi.fn(() => merging) });
    const suggestions = [
      makeSuggestion({ matchedShots: 2 }),
      makeSuggestion({ orphanSectionId: 3, matchedShots: 2 }),
    ];
    render(<OrphanPlacementDialog suggestions={suggestions} />);
    expect(screen.getByTestId('overlap-mode-select')).toBeEnabled();

    await userEvent.click(screen.getByTestId('orphan-accept'));

    await waitFor(() => expect(screen.getByTestId('orphan-accept')).toBeDisabled());
    const whileRunning = {
      accept: screen.getByTestId('orphan-accept').hasAttribute('disabled'),
      picker: screen.getByTestId('overlap-mode-select').hasAttribute('disabled'),
    };
    expect(whileRunning).toEqual({ accept: true, picker: true });

    release();
    await waitFor(() => expect(screen.getByTestId('orphan-accept')).toBeEnabled());
    expect(screen.getByTestId('overlap-mode-select')).toBeEnabled();
  });
});
