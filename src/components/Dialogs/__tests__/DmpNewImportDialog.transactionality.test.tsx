/**
 * #394: the DMP wizard's two exits vs. the sections it has already imported.
 *
 * The wizard imports EAGERLY - the zero-datum anchor lands in the project from a
 * mount effect, before the user has agreed to anything, because the map has to
 * render it. Both exits used to ignore that:
 *
 * * BACK reopened the previous step and the forward pass re-ran the same mount
 *   effect, so the anchor section was imported a SECOND time (F1).
 * * CANCEL gated its confirmation on `connections.length` - client-side records
 *   that have imported nothing - so cancelling right after the anchor import
 *   closed silently and the project kept the section (F2).
 *
 * The oracle here is a fake project the IPC mock actually mutates: `import_sections`
 * appends ids, `execute_batch` removes the ones its `delete_section` commands name.
 * A test that only counted IPC calls would pass on a withdrawal that deleted the
 * wrong ids.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { failOnRefreshErrors } from '../../../test/refreshGuard';
import { render, screen, fireEvent, waitFor, act } from '@testing-library/react';
import type { DmpFile } from '../../../types/commands';
import type { ImportReceipt } from '../../../types/ipc/ImportReceipt';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute. Nothing here asserts on anything drawn.
vi.mock('../SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 400,
  MAP_PANEL_MAX_WIDTH: 800,
}));
// Interpolation is kept, not swallowed: "how many sections are at stake" is the
// whole point of the new confirmation copy, so the count has to be observable.
vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).i18nKeys('keyAndOptions'));
vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);

import { emptyBinaryRefresh, invokeIpcMock, stubGetErrorMessage } from '../../../test/factories/ipc';

vi.mock('../../../utils/ipc');
// [#576 NF-W6A-5] The refresh every edit/import ends with reads bytes; a test that
// gives none answers "no project open" rather than making the decoder throw.
emptyBinaryRefresh();
stubGetErrorMessage((e, fallback) => (e instanceof Error ? e.message : fallback));

import i18n from '../../../i18n';
import DmpNewImportDialog, { type SavedManualPlacingState } from '../DmpNewImportDialog';
import type { ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { convertToEditableShots } from '../../../utils/shotConversion';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import type { Project } from '../../../types/project';

const receipt = (): ImportReceipt => ({
  source_format: 'mNemo DMP',
  file_name: 'dive.dmp',
  sections_found: 2,
  sections_imported: 1,
  shot_count: 3,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: [],
  info: [],
  // #529's field. Null here: the withdrawal cases never assert on the datum
  // line, and a receipt that claims one would only muddy what they do assert.
  zero_datum_created: null,
});

// Fully synthetic, in the real `DmpShot` shape: `shot_type: 'Standard'` and a
// non-unit length are what `isImportableDmpShot` keeps.
const dmpSection = (name: string) => ({
  version: 1,
  name,
  direction: 'in',
  date: '2026-01-01',
  content_hash: `hash_${name}`,
  shots: Array.from({ length: 2 }, () => ({
    shot_type: 'Standard',
    heading_in: 10,
    heading_out: 10,
    length: 5,
    depth_in: 0,
    depth_out: 0,
    pitch_in: 0,
    pitch_out: 0,
    left: 0,
    right: 0,
    up: 0,
    down: 0,
    temperature: 20,
    hour: 12,
    minute: 0,
    second: 0,
    marker_index: 0,
  })),
});

const dmpFile = { version: 1, sections: [dmpSection('A'), dmpSection('B')] } as unknown as DmpFile;

// Hashes present, because the withdrawal has to forget exactly these (#394
// review claim 3): the section hashes are the SELECTED sections' - what this run
// sent to `import_sections` and therefore what it caused to be tracked.
const selectionConfig = (selected: number[]): DmpSelectionConfig => ({
  selectedIndices: new Set(selected),
  defaultSurveyor: 'S',
  defaultDate: null,
  fileHash: 'file-hash',
  sectionHashes: ['hash_A', 'hash_B'],
});

const savedState = (overrides: Partial<SavedManualPlacingState>): SavedManualPlacingState => ({
  connections: [],
  skippedIndices: [],
  segmentQueue: [],
  segmentAnchor: { sectionIndex: 0, stationIndex: 0 },
  directPlacements: [],
  skippedSuggestions: [],
  segmentSkipCounts: [],
  sectionEdits: [],
  ...overrides,
});

/** The single-section case: the zero datum IS the whole import, so the mount
 *  effect imports it and the wizard needs no further input. */
const zeroDatumOnly = (): ZeroDatumConfigResult => ({
  sectionIndex: 0,
  stationIndex: 0,
  gps: { latitude: 20, longitude: -87, depth_reference: 0 },
  metadata: {
    name: 'A',
    surveyor: 'S',
    date: '2026-01-01',
    color: '#ffffff',
    direction: 'in',
    quality: 1,
    reverse: false,
  },
  shots: convertToEditableShots(dmpFile.sections[0]!.shots, dmpFile.sections[0]!.version),
});

/** The fake project: what the wizard has actually put in it, right now. */
let projectSectionIds: number[] = [];
let nextSectionId = 0;
/** Every `execute_batch` payload the wizard sent, so "one undo step" is checkable. */
let batches: Array<Array<{ type: string; section_id?: number }>> = [];
/** Every command name, in order, so "did it save AFTER the batch" is checkable. */
let ipcOrder: string[] = [];
/** What `execute_batch` should answer with, for the refusal case. */
let batchResult: { success: boolean; error?: string } = { success: true };
/** Same, for the write-through save that follows a successful batch. */
let saveResult: { success: boolean; error?: string } = { success: true };

/** A promise the test resolves by hand, to hold one IPC call open. */
function deferred(): { promise: Promise<void>; release: () => void } {
  let release!: () => void;
  const promise = new Promise<void>((resolve) => {
    release = resolve;
  });
  outstandingReleases.push(release);
  return { promise, release };
}
/** Commands parked until the test releases them, by command name. */
let gates: Map<string, Promise<void>> = new Map();
/** Every parked promise's release, so a failing test cannot leave one hanging:
 *  the store serializes its mutations through one module-level chain, and a gate
 *  left closed would stall every command in every LATER test. */
let outstandingReleases: Array<() => void> = [];

const renderWizard = (
  saved: SavedManualPlacingState | undefined,
  props: {
    onComplete?: (ids?: number[], r?: ImportReceipt) => void;
    onCancel?: () => void;
    onBack?: () => void;
    selected?: number[];
    zeroDatumConfig?: ZeroDatumConfigResult;
  },
) =>
  render(
    <DmpNewImportDialog
      dmpFile={dmpFile}
      selectionConfig={selectionConfig(props.selected ?? [0])}
      project={null}
      stations={[]}
      onCancel={props.onCancel ?? vi.fn()}
      onBack={props.onBack ?? vi.fn()}
      onComplete={props.onComplete ?? vi.fn()}
      savedManualPlacingState={saved}
      zeroDatumConfig={props.zeroDatumConfig}
      autoStartManualPlacement={false}
    />,
  );

/**
 * The anchor import has landed AND the run has let go of it. The fake project
 * holds the id the moment the mock answers, but the wizard is still importing
 * until the store's import path returns (its refresh, its integrity check), and
 * both exits are shut while it is (`exitBlocked`) - a click in that window hits
 * a disabled button and opens nothing. Waiting on the id alone lost that race
 * (#576 NF-FIX5-2: the file's first case, 22 runs in 30 on a quiet machine).
 */
async function anchorImportSettled(): Promise<void> {
  await waitFor(() => expect(projectSectionIds).toEqual([7]));
  await waitFor(() => expect(screen.getByTestId('back-button')).toBeEnabled());
  expect(screen.getByTestId('cancel-button')).toBeEnabled();
}

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('DMP wizard: what Back and Cancel do with sections already imported (#394)', () => {
  beforeEach(() => {
    projectSectionIds = [];
    nextSectionId = 7;
    batches = [];
    ipcOrder = [];
    batchResult = { success: true };
    saveResult = { success: true };
    gates = new Map();
    outstandingReleases = [];
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      // What was ISSUED is recorded before any gate, so a test can assert on the
      // request while the reply is still parked.
      ipcOrder.push(cmd);
      const commands = (args?.commands ?? []) as Array<{ type: string; section_id?: number }>;
      if (cmd === 'execute_batch') batches.push(commands);
      const gate = gates.get(cmd);
      if (gate) await gate;
      if (cmd === 'import_sections') {
        const configs = (args?.sections as unknown[] | undefined) ?? [];
        const ids = configs.map(() => nextSectionId++);
        projectSectionIds.push(...ids);
        return { success: true, section_ids: ids, import_receipt: receipt(), warnings: [] };
      }
      if (cmd === 'execute_batch') {
        // A refusal is the backend's CommandError rejection (#576 C4).
        if (!batchResult.success) throw { code: 'FAILED', message: batchResult.error ?? '' };
        for (const command of commands) {
          if (command.type === 'delete_section') {
            projectSectionIds = projectSectionIds.filter((id) => id !== command.section_id);
          }
        }
        return { success: true, warnings: [] };
      }
      if (cmd === 'save_project') {
        if (!saveResult.success) throw { code: 'FAILED', message: saveResult.error ?? '' };
        return { saved_path: null, unlinked_copy_path: null, warnings: [] };
      }
      return null;
    });
    useUIStore.setState({
      activeDialog: null,
      dialogData: {},
      placementMode: null,
      importContext: null,
    });
    useProjectStore.setState({ project: null });
  });

  afterEach(async () => {
    // Unpark anything a failing assertion left closed - see `outstandingReleases`.
    for (const release of outstandingReleases) release();
    await act(async () => {});
  });

  /**
   * F1. The wizard cannot "remember it already imported" across Back: the mount
   * effect that imports the anchor is the SAME one that fires
   * `suggest_all_import_placements`, so suppressing it would strand the reopened
   * instance with nothing to do - and it would pin the anchor to the first pass's
   * GPS, which is what the user pressed Back to change. So Back withdraws, and
   * the forward pass imports exactly once more.
   *
   * Injection: drop the `withdrawImportedSections()` call from
   * `handleBackWithRemoval` and the second pass leaves TWO sections behind.
   */
  it('Back withdraws the anchor, so re-confirming the same config leaves one section', async () => {
    const onBack = vi.fn();
    const first = renderWizard(undefined, {
      onBack,
      selected: [0],
      zeroDatumConfig: zeroDatumOnly(),
    });

    await anchorImportSettled();

    fireEvent.click(screen.getByTestId('back-button'));
    expect(screen.getByTestId('back-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.backWizardMessage:{"count":1}',
    );

    fireEvent.click(screen.getByTestId('confirm-back-button'));
    await waitFor(() => expect(onBack).toHaveBeenCalled());
    expect(projectSectionIds).toEqual([]);
    first.unmount();

    // App.tsx reopens `zero-datum-config`; the user confirms the same anchor
    // again and a FRESH wizard instance mounts on it.
    const onComplete = vi.fn();
    renderWizard(undefined, { onComplete, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(projectSectionIds).toHaveLength(1);
    // #487 residue 1: the withdrawn pass's receipt went with it, so the summary
    // describes ONE import, not two.
    expect(onComplete.mock.calls[0]![1]).toMatchObject({ sections_imported: 1, shot_count: 3 });
  });

  /**
   * F2, first half. The confirmation has to exist at all, and has to say how many
   * sections are at stake - `connections` is empty here, which is exactly the
   * state the old gate read as "nothing to warn about".
   *
   * Injection: restore the gate to `connections.length > 0` and the wizard closes
   * without ever rendering the dialog this queries.
   */
  it('Cancel after the anchor import asks, and names the count', async () => {
    const onCancel = vi.fn();
    renderWizard(undefined, { onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await anchorImportSettled();

    fireEvent.click(screen.getByTestId('cancel-button'));

    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByTestId('cancel-confirm-dialog')).toBeInTheDocument();
    expect(screen.getByTestId('cancel-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.cancelWizardMessage:{"count":1}',
    );
    expect(screen.getByTestId('cancel-remove-button')).toBeInTheDocument();
  });

  /**
   * F2, second half, on a run that crossed the map-placement handoff: the ids
   * ride `SavedManualPlacingState.importedSectionIds` (#487 residue 2), and the
   * withdrawal is ONE `execute_batch` - one undo entry for one user action.
   * (That a batch of `delete_section` really is one undo entry is pinned in Rust:
   * `batched_section_deletes_are_one_undo_entry`.)
   *
   * Injection: drop `importedSectionIds` from the saved-state seed and the button
   * is not even offered; loop `deleteSection` instead of batching and `batches`
   * holds two one-command payloads.
   */
  it('Remove withdraws the whole run - every carried id, in one batch', async () => {
    projectSectionIds = [7, 8];
    const onCancel = vi.fn();
    const onComplete = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7, 8],
        importedReceipts: [receipt(), receipt()],
      }),
      { onCancel, onComplete, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    expect(screen.getByTestId('cancel-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.cancelWizardMessage:{"count":2}',
    );

    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(projectSectionIds).toEqual([]);
    expect(batches).toEqual([
      [
        { type: 'delete_section', section_id: 7 },
        { type: 'delete_section', section_id: 8 },
        { type: 'untrack_import', file_hashes: ['file-hash'], section_hashes: ['hash_A', 'hash_B'] },
      ],
    ]);
    // A withdrawn run completes nothing, so nothing is summarised for it either.
    expect(onComplete).not.toHaveBeenCalled();
  });

  /**
   * The other half of the F2 decision: "Keep and close" is still offered, and it
   * is the one exit that leaves the project alone.
   *
   * Injection: wire `confirm-cancel-button` to the withdrawal and the kept
   * section disappears.
   */
  it('Keep and close leaves the imported sections in the project', async () => {
    const onCancel = vi.fn();
    renderWizard(undefined, { onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await anchorImportSettled();

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('confirm-cancel-button'));

    expect(onCancel).toHaveBeenCalled();
    expect(projectSectionIds).toEqual([7]);
    expect(batches).toEqual([]);
  });

  /**
   * The done step completes the run ONCE (#576 D4). It was an effect watching
   * for `done` that ran again whenever the run's busy flag flipped, so a removal
   * from that screen - open to the user while App.tsx is still saving after the
   * first completion - completed the run a second time.
   *
   * Injection: put the completion back in an effect keyed on `isImporting` and
   * `onComplete` is called twice.
   */
  it('a removal from the done screen does not complete the run again', async () => {
    const onComplete = vi.fn();
    const onCancel = vi.fn();
    renderWizard(undefined, { onComplete, onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await waitFor(() => expect(onComplete).toHaveBeenCalledTimes(1));
    await anchorImportSettled();
    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));
    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    await act(async () => {});

    expect(projectSectionIds).toEqual([]);
    expect(onComplete).toHaveBeenCalledTimes(1);
  });

  /**
   * The ids are seeded from the saved state AND appended by live imports, so one
   * can appear twice across the map-placement handoff. `execute_batch` is
   * all-or-nothing and the second `delete_section` for an id fails "not found",
   * which rolls the WHOLE batch back - the withdrawal would remove nothing and
   * report an error for it.
   *
   * Injection: drop the `new Set(...)` and the batch carries two commands.
   */
  it('sends each section once even when an id was banked twice', async () => {
    projectSectionIds = [7];
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7, 7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(batches).toEqual([
      [
        { type: 'delete_section', section_id: 7 },
        { type: 'untrack_import', file_hashes: ['file-hash'], section_hashes: ['hash_A', 'hash_B'] },
      ],
    ]);
    expect(projectSectionIds).toEqual([]);
  });

  /**
   * A delete is on the wire, so every route out of the confirmation is shut -
   * "Keep and close" most of all. It would close the wizard over a removal that
   * still completes, leaving the user looking at a project they chose to keep
   * sections in while those sections vanish behind them.
   *
   * Injection: drop `disabled={withdrawInFlight}` from `confirm-cancel-button`
   * and `onCancel` fires while the batch is still parked.
   */
  it('will not close over a withdrawal that has not landed yet', async () => {
    projectSectionIds = [7];
    const batchGate = deferred();
    gates.set('execute_batch', batchGate.promise);
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));
    await waitFor(() => expect(batches).toHaveLength(1));

    // The batch is parked. Every escape hatch is refused.
    fireEvent.click(screen.getByTestId('confirm-cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-confirm-continue'));
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByTestId('cancel-confirm-dialog')).toBeInTheDocument();

    await act(async () => {
      batchGate.release();
    });
    await waitFor(() => expect(onCancel).toHaveBeenCalledTimes(1));
    expect(batches).toHaveLength(1);
    expect(projectSectionIds).toEqual([]);
  });

  /**
   * Two clicks in ONE tick both read the pre-`setState` latch, so the guard has
   * to be a ref - the rendered `disabled` only exists after React re-renders,
   * which is one flush too late. Dispatched raw inside a single `act` for
   * exactly that reason: `fireEvent` flushes between calls, so a pair of
   * `fireEvent.click`s would be caught by the `disabled` attribute and would
   * pass with no latch at all.
   *
   * Injection: replace the `withdrawInFlightRef` entry guard with `false` and a
   * second batch goes out for ids the first is already deleting - which the
   * backend answers "not found", rolling that one back and removing nothing.
   */
  it('two clicks in one tick still send one batch', async () => {
    projectSectionIds = [7];
    const batchGate = deferred();
    gates.set('execute_batch', batchGate.promise);
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    const remove = screen.getByTestId('cancel-remove-button');
    await act(async () => {
      remove.dispatchEvent(new MouseEvent('click', { bubbles: true }));
      remove.dispatchEvent(new MouseEvent('click', { bubbles: true }));
    });

    expect(batches).toHaveLength(1);
    await act(async () => {
      batchGate.release();
    });
    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(batches).toHaveLength(1);
  });

  /**
   * An import banks its ids only when it RESOLVES, so a withdrawal decided while
   * one is in flight would snapshot a list about to grow and leave the late
   * arrival in the project with nothing tracking it - the exact F2 symptom, one
   * layer down. Neither exit may open while an import is running.
   *
   * BOTH halves are asserted, because they fail independently: the footer
   * buttons carry `disabled`, and the routes that bypass them - the wizard's own
   * close button and Escape, which both land on `handleCancel` - are covered by
   * the handler's own guard.
   *
   * Injection: drop `disabled={exitBlocked}` and the first assertion goes red;
   * drop `if (isImporting) return;` from `handleCancel` and the close-button one
   * does, opening a confirmation that names 0 sections while an id is a moment
   * from landing.
   */
  it('refuses to open an exit while an import is in flight, and loses no id', async () => {
    const importGate = deferred();
    gates.set('import_sections', importGate.promise);
    const onCancel = vi.fn();
    renderWizard(undefined, { onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    // The anchor import is parked, so nothing is banked yet.
    await waitFor(() => expect(ipcOrder).toContain('import_sections'));
    expect(screen.getByTestId('cancel-button')).toBeDisabled();
    expect(screen.getByTestId('back-button')).toBeDisabled();
    // The X in the title bar has no `disabled` to obey - it goes straight to the
    // same handler, which refuses on its own.
    fireEvent.click(screen.getByTestId('dmp-new-import-dialog-close'));
    expect(screen.queryByTestId('cancel-confirm-dialog')).not.toBeInTheDocument();
    expect(onCancel).not.toHaveBeenCalled();

    await act(async () => {
      importGate.release();
    });
    await anchorImportSettled();

    // And the id that landed is tracked: cancelling now offers to remove it.
    fireEvent.click(screen.getByTestId('cancel-button'));
    expect(screen.getByTestId('cancel-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.cancelWizardMessage:{"count":1}',
    );
  });

  /**
   * #394 F14 residue. `deferInitialSave` only delays the write until the eager
   * anchor import, which resolves it (`finalize_deferred_import`) - so by the
   * time the user withdraws, the file on disk already holds the section. The
   * withdrawal writes the emptied project through; it never unlinks the file.
   *
   * Injection: drop the `saveProject()` call and no save follows the batch, so
   * disk keeps a section the project no longer has.
   */
  it('saves the withdrawn state so disk stops holding the removed sections', async () => {
    useUIStore.setState({ importContext: 'new-project' });
    const onCancel = vi.fn();
    renderWizard(undefined, { onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await anchorImportSettled();
    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    // AFTER the batch, not before: the save has to capture the withdrawal.
    expect(ipcOrder.indexOf('save_project')).toBeGreaterThan(ipcOrder.indexOf('execute_batch'));
  });

  /**
   * The save is not best-effort. Closing here is what makes the removal final
   * from the user's side, so a `.stalx` that still holds the sections while the
   * project no longer does is the F2 shape one layer out - and the user would
   * have no idea. The wizard stays open with the reason instead.
   *
   * Injection: ignore `saved.success` (the original `await saveProject()`) and
   * the wizard closes over a stale file.
   */
  it('stays open when the write-through save fails, and a retry re-attempts it', async () => {
    useUIStore.setState({ importContext: 'new-project' });
    saveResult = { success: false, error: 'FS_PERMISSION: folder is read-only' };
    projectSectionIds = [7];
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    // In the confirmation the user is looking at (#576 NF-AN4-N2), not behind it.
    await waitFor(() =>
      expect(screen.getByTestId('cancel-confirm-dialog')).toHaveTextContent('FS_PERMISSION'),
    );
    expect(onCancel).not.toHaveBeenCalled();
    // The delete DID land - this is a disk failure, not a backend refusal.
    expect(projectSectionIds).toEqual([]);

    // The retry has no sections left to delete, so it must retry the SAVE
    // rather than find an empty list and close over the stale file.
    saveResult = { success: true };
    const savesBefore = ipcOrder.filter((c) => c === 'save_project').length;
    fireEvent.click(screen.getByTestId('cancel-remove-button'));
    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(ipcOrder.filter((c) => c === 'save_project').length).toBe(savesBefore + 1);
  });

  /**
   * #394 review claim 3. `execute_import_sections` records the file hash and each
   * section's content hash, and `delete_section` does not touch them - so without
   * this the project still claims the file was imported while none of it remains,
   * and `DmpSelectionDialog` auto-DESELECTS every section it thinks is a
   * duplicate. The forget rides the same batch, so one undo restores sections and
   * tracking together.
   *
   * Injection: drop the `untrack_import` entry and the batch carries deletes only.
   */
  it('forgets the import-dedup hashes in the same batch as the deletes', async () => {
    projectSectionIds = [7];
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(batches).toHaveLength(1);
    expect(batches[0]).toEqual([
      { type: 'delete_section', section_id: 7 },
      { type: 'untrack_import', file_hashes: ['file-hash'], section_hashes: ['hash_A', 'hash_B'] },
    ]);
  });

  /**
   * Importing into an EXISTING project, the user may have unrelated pending
   * edits open, and committing those behind their back is not this action's to
   * do - the same reason `saveAfterImport` only saves for `new-project`. The
   * withdrawal leaves the project dirty and autosave carries it.
   *
   * Injection: make the save unconditional and this sees a `save_project`.
   */
  it('does not force a save when the import went into an existing project', async () => {
    useUIStore.setState({ importContext: 'existing-project' });
    const onCancel = vi.fn();
    renderWizard(undefined, { onCancel, selected: [0], zeroDatumConfig: zeroDatumOnly() });

    await anchorImportSettled();
    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(onCancel).toHaveBeenCalled());
    expect(ipcOrder).not.toContain('save_project');
  });

  /**
   * A refused batch must leave everything as it was: the wizard open, the ids
   * still tracked, the reason on screen. Closing over a project it failed to
   * clean up would strand exactly the sections this action exists to remove.
   *
   * Injection: return `true` unconditionally from `withdrawImportedSections` and
   * the wizard closes on a project that still holds the section.
   */
  it('stays open, with the ids intact, when the backend refuses the batch', async () => {
    projectSectionIds = [7];
    batchResult = { success: false, error: 'Section 7 is referenced by a closure' };
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() => expect(batches).toHaveLength(1));
    await waitFor(() =>
      expect(screen.getByTestId('cancel-confirm-dialog')).toHaveTextContent(
        'Section 7 is referenced by a closure',
      ),
    );
    expect(onCancel).not.toHaveBeenCalled();
    expect(projectSectionIds).toEqual([7]);

    // Still tracked: a second attempt is still offered the same section.
    fireEvent.click(screen.getByTestId('cancel-button'));
    expect(screen.getByTestId('cancel-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.cancelWizardMessage:{"count":1}',
    );
  });

  /**
   * Section ids are small per-project integers and nothing clears `dialogData`
   * when the open project changes, so a saved state can outlive its project -
   * and every stale id would RESOLVE against whatever is open now. Deleting
   * same-numbered sections out of an unrelated survey is the one failure here
   * that destroys data the user never touched, so it refuses.
   *
   * Injection: drop the `projectSwitched` guard and the batch goes out against
   * the new project.
   */
  it('refuses to withdraw into a project that is not the one it imported to', async () => {
    projectSectionIds = [7];
    useProjectStore.setState({ project: { created_at: 'project-B' } as Project });
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
        projectKey: 'project-A',
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));

    await waitFor(() =>
      expect(screen.getByTestId('cancel-confirm-dialog')).toHaveTextContent(
        'dialogs.dmpNewImport.removeImportedWrongProject',
      ),
    );
    expect(batches).toEqual([]);
    expect(projectSectionIds).toEqual([7]);
    expect(onCancel).not.toHaveBeenCalled();
  });

  /**
   * With nothing imported the wizard has nothing to withdraw, so the copy is the
   * plain question - the string that had been unreachable since the gate keyed
   * on `connections.length`, the same field that decided which branch to show.
   *
   * Injection: key the message off `connections.length` again and this renders
   * the count-bearing string with `count: 0`.
   */
  it('offers the plain confirmation when a connection is recorded but nothing is imported', () => {
    const onCancel = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        connections: [
          {
            importSectionIndex: 1,
            importStationIndex: 0,
            targetType: 'existing',
            existingSectionId: 3,
            existingStationIndex: 0,
          },
        ],
      }),
      { onCancel, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('cancel-button'));

    expect(screen.getByTestId('cancel-confirm-message')).toHaveTextContent(
      'dialogs.dmpNewImport.cancelWizardNoImports',
    );
    expect(screen.queryByTestId('cancel-remove-button')).not.toBeInTheDocument();
  });
});

/**
 * #576 NF-AN4-N2. A failed "remove imported sections" wrote its reason into the
 * wizard's one shared error slot, which renders in the panel BEHIND the open
 * Cancel/Back confirmation: the confirmation kept showing only its static copy.
 * And because the slot was shared, "Continue editing" then showed the removal's
 * error next to the late import's Retry - a button that re-runs the IMPORT.
 */
describe('DMP wizard: a failed removal is reported in the confirmation (#576 NF-AN4-N2)', () => {
  const placedOnly = (overrides: Partial<SavedManualPlacingState> = {}) =>
    savedState({
      segmentQueue: [],
      directPlacements: [
        {
          anchorSectionIndex: 0,
          anchorStationIndex: 0,
          placement: { type: 'manual', align_station_index: 0, offset: { x: 0, y: 0, z: 0 } },
        },
      ],
      ...overrides,
    });
  let importResult: Record<string, unknown> | null;
  let batchAnswer: Record<string, unknown>;

  beforeEach(() => {
    ipcOrder = [];
    importResult = null;
    // Refusals, as the backend's CommandError rejections (#576 C4).
    batchAnswer = { code: 'FAILED', message: 'Batch refused (test)' };
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      ipcOrder.push(cmd);
      if (cmd === 'import_sections') {
        if (importResult) throw importResult;
        const configs = (args?.sections as unknown[] | undefined) ?? [];
        return { success: true, section_ids: configs.map(() => 20), import_receipt: receipt(), warnings: [] };
      }
      if (cmd === 'execute_batch') throw batchAnswer;
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, importContext: null, toasts: [] });
    useProjectStore.setState({ project: null });
  });

  it('Back shows the refusal inside its confirmation, not in the panel behind it', async () => {
    const onBack = vi.fn();
    renderWizard(
      savedState({
        segmentQueue: [[1]],
        segmentAnchor: { sectionIndex: 1, stationIndex: 0 },
        importedSectionIds: [7],
      }),
      { onBack, selected: [0, 1] },
    );

    fireEvent.click(screen.getByTestId('back-button'));
    fireEvent.click(screen.getByTestId('confirm-back-button'));

    await waitFor(() =>
      expect(screen.getByTestId('back-confirm-dialog')).toHaveTextContent('Batch refused (test)'),
    );
    expect(screen.getByTestId('wizard-manual-placing')).not.toHaveTextContent('Batch refused (test)');
    expect(onBack).not.toHaveBeenCalled();
  });

  it('Continue editing returns to the late-import failure and its Retry, not to the removal', async () => {
    importResult = { code: 'FAILED', message: 'Late refused (test)' };
    renderWizard(placedOnly({ importedSectionIds: [7] }), { selected: [0] });

    await waitFor(() => expect(screen.getByTestId('wizard-placing')).toHaveTextContent('Late refused (test)'));
    expect(screen.getByTestId('retry-button')).toBeInTheDocument();

    fireEvent.click(screen.getByTestId('cancel-button'));
    fireEvent.click(screen.getByTestId('cancel-remove-button'));
    await waitFor(() =>
      expect(screen.getByTestId('cancel-confirm-dialog')).toHaveTextContent('Batch refused (test)'),
    );

    fireEvent.click(screen.getByTestId('cancel-confirm-continue'));

    await waitFor(() => expect(screen.queryByTestId('cancel-confirm-dialog')).not.toBeInTheDocument());
    // The Retry beside the error re-runs the late IMPORT, so the error beside it
    // has to be the import's.
    expect(screen.getByTestId('wizard-placing')).toHaveTextContent('Late refused (test)');
    expect(screen.getByTestId('wizard-placing')).not.toHaveTextContent('Batch refused (test)');
    importResult = null;
    fireEvent.click(screen.getByTestId('retry-button'));
    await waitFor(() => expect(ipcOrder.filter((c) => c === 'import_sections')).toHaveLength(2));
  });
});

/**
 * #576 D6: the wizard's late import goes through the store's single edit path
 * (`importSections` -> `executeCommand`), so it gets that path's aftermath -
 * among it the deferred-save flag (`noteImportResolvedDeferredSave`) and the
 * integrity re-check - instead of the two steps the wizard used to replicate by
 * hand. The case that mattered: a brand-new project without a zero datum, left
 * with "Keep and close", kept `databaseOpen` false (warning resolution and
 * dive-plan saving disabled) although the import had written the file.
 */
describe('DMP wizard: wizard imports run through the store edit path (#576 D6)', () => {
  beforeEach(() => {
    ipcOrder = [];
    nextSectionId = 7;
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      ipcOrder.push(cmd);
      if (cmd === 'import_sections') {
        const configs = (args?.sections as unknown[] | undefined) ?? [];
        return { success: true, section_ids: configs.map(() => nextSectionId++), import_receipt: receipt(), warnings: [] };
      }
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, importContext: null });
    // A project created with a deferred first save: no file yet, so the DB-backed
    // surfaces are off until something writes it.
    useProjectStore.setState({ project: null, databaseOpen: false, isDirty: false });
  });

  it('opens the deferred-save gate and re-checks integrity after the late import', async () => {
    const onComplete = vi.fn();
    // Reopened after the user placed section 0 by hand: queue drained, the placement
    // recorded but not imported yet, so the late import sends it.
    renderWizard(
      savedState({
        segmentQueue: [],
        directPlacements: [
          {
            anchorSectionIndex: 0,
            anchorStationIndex: 0,
            placement: { type: 'manual', align_station_index: 0, offset: { x: 0, y: 0, z: 0 } },
          },
        ],
      }),
      { onComplete, selected: [0] },
    );

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    const afterImport = ipcOrder.slice(ipcOrder.indexOf('import_sections'));
    expect(afterImport[0]).toBe('import_sections');
    expect({
      databaseOpen: useProjectStore.getState().databaseOpen,
      integrityRechecked: afterImport.includes('check_project_integrity'),
      dirty: useProjectStore.getState().isDirty,
    }).toEqual({ databaseOpen: true, integrityRechecked: true, dirty: true });
  });
});

/**
 * #576 RV2-FE-D1 / RV2-FE-D3: the wizard's own run identity, and a read-only refusal.
 *
 * D1: the store's edit path refuses an import whose project changed between the call
 * and the IPC - but it captures the project at the CALL, so a run whose project changed
 * before that (the saved state outlives a project switch: nothing clears `dialogData`)
 * imported into whatever was open. Only the withdrawal checked the run's own project
 * key. Every import now does.
 *
 * D3: a read-only refusal of a wizard import (the lock lost mid-wizard, the Plan/Export
 * workspace) showed the raw coded EDIT_DISABLED string inline, AND the store's friendly
 * read-only toast. One message now: the friendly one, inline, where the wizard shows
 * its errors.
 */
describe('DMP wizard: imports land in the run project, a read-only refusal reads once (#576 RV2-FE-D1/D3)', () => {
  const placedOnly = (overrides: Partial<SavedManualPlacingState> = {}) =>
    savedState({
      segmentQueue: [],
      directPlacements: [
        {
          anchorSectionIndex: 0,
          anchorStationIndex: 0,
          placement: { type: 'manual', align_station_index: 0, offset: { x: 0, y: 0, z: 0 } },
        },
      ],
      ...overrides,
    });
  let importResult: Record<string, unknown> | null;

  beforeEach(() => {
    ipcOrder = [];
    nextSectionId = 7;
    importResult = null;
    invokeIpcMock.mockReset();
    invokeIpcMock.mockImplementation(async (cmd: string, args?: Record<string, unknown>) => {
      ipcOrder.push(cmd);
      if (cmd === 'import_sections') {
        if (importResult) throw importResult;
        const configs = (args?.sections as unknown[] | undefined) ?? [];
        return { success: true, section_ids: configs.map(() => nextSectionId++), import_receipt: receipt(), warnings: [] };
      }
      return null;
    });
    useUIStore.setState({ activeDialog: null, dialogData: {}, placementMode: null, importContext: null, toasts: [] });
    useProjectStore.setState({ project: { created_at: 'project-A' } as Project });
  });

  it('refuses the late import once another project is open', async () => {
    useProjectStore.setState({ project: { created_at: 'project-B' } as Project });
    const onComplete = vi.fn();
    renderWizard(placedOnly({ projectKey: 'project-A' }), { onComplete, selected: [0] });

    await waitFor(() =>
      expect(screen.getByTestId('wizard-placing')).toHaveTextContent('dialogs.dmpNewImport.importWrongProject'),
    );
    expect(ipcOrder, 'imported into the project that replaced the run').not.toContain('import_sections');
    expect(onComplete).not.toHaveBeenCalled();
  });

  it('still imports into the project the run belongs to', async () => {
    const onComplete = vi.fn();
    renderWizard(placedOnly({ projectKey: 'project-A' }), { onComplete, selected: [0] });

    await waitFor(() => expect(onComplete).toHaveBeenCalled());
    expect(ipcOrder).toContain('import_sections');
  });

  it('shows a read-only refusal as the friendly notice, inline and nowhere else', async () => {
    importResult = {
      code: 'WORKSPACE_READ_ONLY',
      message: 'Import sections is disabled in Plan and Export.',
    };
    renderWizard(placedOnly(), { selected: [0] });

    await waitFor(() =>
      expect(screen.getByTestId('wizard-placing')).toHaveTextContent(i18n.t('editGate.workspaceReadOnlyNotice')),
    );
    expect(screen.getByTestId('wizard-placing')).not.toHaveTextContent('EDIT_DISABLED');
    expect(screen.getByTestId('wizard-placing')).not.toHaveTextContent('Import sections is disabled in Plan and Export.');
    expect(useUIStore.getState().toasts.map((t) => t.message), 'the same notice toasted on top').toEqual([]);
  });
});
