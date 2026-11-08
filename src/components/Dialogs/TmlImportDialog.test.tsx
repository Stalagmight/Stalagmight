/**
 * Component test (#110) - TML import warning surfacing.
 *
 * The dialog must surface the deferred-save partial-success warning ("imported
 * but unsaved") BEFORE the optional orphan-placement step - but must NOT modal
 * (or skip orphan for) the routine per-shot validation warnings (LONG_SHOT, …)
 * that ride along on a fully-saved import (those are surfaced via the data-table
 * UI). The helper's filtering logic is unit-tested in
 * utils/__tests__/saveFeedback.test.ts; this guards the dialog wiring + ordering.
 *
 * Plus the #411 repairs: a terminal failure never navigates, the empty project a
 * cancelled/failed import leaves behind is acknowledged, and the preview counts
 * what will actually import. Cancel WHILE the import runs (it asks the backend to
 * stop it) is covered for all five import dialogs in
 * __tests__/importCancel.test.tsx.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);
// SectionPreview and the map panel pull maplibre-gl (WebGL) at module load,
// which jsdom cannot execute. What the map DRAWS is covered by
// utils/__tests__/tmlPreviewLayout.test.ts, on the pure layout function.
vi.mock('./SectionPreview', async (importOriginal) => ({
  ...(await importOriginal<typeof import('./SectionPreview')>()),
  default: () => null,
}));
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('./WizardMapPanel', () => ({
  WizardMapPanel: () => null,
  MAP_PANEL_DEFAULT_WIDTH: 480,
  MAP_PANEL_MIN_WIDTH: 280,
  MAP_PANEL_MAX_WIDTH: 900,
}));
// invokeIpc is used here only for suggest_orphan_placements; mock it so the
// orphan branch resolves and we can assert whether it ran.
vi.mock('../../utils/ipc');

import TmlImportDialog from './TmlImportDialog';
import { trackDialogComplete } from '../../utils/telemetry';
import { useProjectStore } from '../../store/projectStore';
import { noteProjectSessionChanged } from '../../store/projectSession';
import { useUIStore } from '../../store/uiStore';
import { refusedBy } from '../../store/commandResult';
import type { CommandResult, TmlFile, TmlShot, ValidationWarning } from '../../types/commands';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import type { Project } from '../../types/project';
import { invokeIpcMock } from '../../test/factories/ipc';

const trackDialogCompleteMock = vi.mocked(trackDialogComplete);

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

const tmlFile: TmlFile = {
  cave_name: 'Test Cave',
  unit: 'Metric',
  use_magnetic_azimuth: false,
  first_start_absolute_elevation: 0,
  geo_coding: null,
  sections: [],
  closures: [],
};

const ok = (warnings: ValidationWarning[], section_ids: number[] = []): CommandResult =>
  ({ success: true, error: null, warnings, section_ids } as CommandResult);

/** What the backend reports for a file whose every section was dropped (#411). */
const EMPTY_RECEIPT: ImportReceipt = {
  source_format: 'Ariane TML',
  file_name: 'cave.tml',
  sections_found: 2,
  sections_imported: 0,
  shot_count: 0,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: ['2 sections had no shots after filtering and were dropped'],
  info: [],
  zero_datum_created: null,
};

/**
 * The wire shape that CRASHED this dialog in #411: `section_ids` was
 * `#[serde(skip_serializing_if = "Vec::is_empty")]`, so an import that created
 * no section omitted the key while the generated type promised an array, and a
 * bare `result.section_ids.length` threw. #441 removed the skip, so the backend
 * always writes the key and this shape is no longer reachable from the shipped
 * binary - the literal is kept as the regression record for #411 and pins the
 * `?? []` that survives it. Every other mock writes the key, which is why the
 * bug hid; only a hand-built literal can express the omission.
 */
const okWithoutSectionIds = (receipt: ImportReceipt): CommandResult =>
  ({ success: true, error: null, warnings: [], import_receipt: receipt } as unknown as CommandResult);

/** A section carrying one shot per flavour asked for. */
const shot = (overrides: Partial<TmlShot> = {}): TmlShot =>
  ({
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
    ...overrides,
  }) as TmlShot;

const withShots = (shots: TmlShot[]): TmlFile => ({
  ...tmlFile,
  sections: [
    {
      name: 'S',
      shots,
      surveyor: null,
      date: null,
      color: '#3b82f6',
      is_branch_split: false,
      start_ariane_id: 0,
      end_ariane_id: 0,
      start_depth: 0,
    },
  ],
});

let executeTmlImport: ReturnType<typeof vi.fn>;
let openDialog: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;
let setView: ReturnType<typeof vi.fn>;
let triggerFitToBounds: ReturnType<typeof vi.fn>;
let closeProject: ReturnType<typeof vi.fn>;
let deleteProject: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeDialog = vi.fn();
  setView = vi.fn();
  triggerFitToBounds = vi.fn();
  executeTmlImport = vi.fn();
  closeProject = vi.fn(async () => ({ success: true }) as CommandResult);
  deleteProject = vi.fn(async () => ({ success: true }) as CommandResult);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue([]); // suggest_orphan_placements → no suggestions
  useProjectStore.setState({
    executeTmlImport,
    calculateFileHash: vi.fn(async () => 'hash'),
    startLoading: vi.fn(() => 0),
    setLoadingProgress: vi.fn(),
    resetLoading: vi.fn(),
    closeProject,
    deleteProject,
    databaseOpen: false,
    project: null,
  });
  // setImportContext is deliberately NOT mocked: the leak these tests guard is
  // about the value left in the store, so the real setter has to run.
  useUIStore.setState({
    openDialog,
    closeDialog,
    setView,
    triggerFitToBounds,
    importContext: null,
  });
});

const importContext = () => useUIStore.getState().importContext;

/** The project a new-project import created: open, and still empty. `created_at`
 *  is the identity token the discard guard re-checks at click time. */
const emptyProject = (overrides: Partial<Project> = {}) =>
  ({ sections: [], created_at: '2026-08-05T00:00:00Z', ...overrides }) as unknown as Project;

function openEmptyImportProject() {
  useProjectStore.setState({ project: emptyProject() });
  useUIStore.setState({ importContext: 'new-project' });
}

async function clickImport() {
  const btn = (await screen.findByTestId('tml-import-confirm')) as HTMLButtonElement;
  await waitFor(() => expect(btn.disabled).toBe(false));
  await userEvent.click(btn);
}

const orphanCalled = () =>
  invokeIpcMock.mock.calls.some(([cmd]) => cmd === 'suggest_orphan_placements');

describe('TmlImportDialog warning surfacing (#110)', () => {
  it('surfaces the deferred-save warning before orphan placement, and skips orphan', async () => {
    executeTmlImport.mockResolvedValue(ok([SAVE_WARNING], [1]));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ title: 'Import Warning' })),
    );
    expect(orphanCalled()).toBe(false); // the save warning short-circuits before the orphan step
  });

  it('does NOT modal a routine validation warning on a saved import - orphan still runs', async () => {
    executeTmlImport.mockResolvedValue(ok([VALIDATION_WARNING], [1]));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() => expect(orphanCalled()).toBe(true)); // proceeded past the warning gate
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.objectContaining({ title: 'Import Warning' }));
  });

  it('shows the receipt when the import created nothing and section_ids is absent (#411)', async () => {
    executeTmlImport.mockResolvedValue(okWithoutSectionIds(EMPTY_RECEIPT));
    // A non-empty PREVIEW: this guards the backend dropping everything it
    // promised (sections_imported: 0 with real content going in), which is
    // distinct from this dialog's own zero-shot preview gate - that one blocks
    // Import outright and never reaches the backend at all.
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: EMPTY_RECEIPT }),
    );
    // Not a transport failure: reading the absent key used to throw, and the
    // catch reported the empty import as a broken IPC call.
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
    expect(orphanCalled()).toBe(false);
  });

  it('opens no warning dialog when the import fully succeeds with no warnings', async () => {
    executeTmlImport.mockResolvedValue(ok([], []));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() => expect(closeDialog).toHaveBeenCalled());
    expect(openDialog).not.toHaveBeenCalled();
  });
});

describe('TmlImportDialog failure handling (#411 C2)', () => {
  it('reports a backend failure WITHOUT navigating into the empty project', async () => {
    executeTmlImport.mockResolvedValue({
      success: false,
      error: 'Section 2 is unreadable',
      warnings: [],
      section_ids: [],
    } as unknown as CommandResult);
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith(
        'error',
        expect.objectContaining({ title: 'Import Errors', message: 'Section 2 is unreadable' }),
      ),
    );
    // The dialog closes (its work is over), but the navigate/fit half of the
    // exit sequence must not run - that showed the empty project first.
    expect(setView).not.toHaveBeenCalled();
    expect(triggerFitToBounds).not.toHaveBeenCalled();
  });

  it('asks about the empty project it was importing into instead of a bare error', async () => {
    openEmptyImportProject();
    executeTmlImport.mockResolvedValue({
      success: false,
      error: 'Section 2 is unreadable',
      warnings: [],
      section_ids: [],
    } as unknown as CommandResult);
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    expect(await screen.findByTestId('tml-import-residue-dialog')).toBeInTheDocument();
    expect(screen.getByTestId('tml-import-residue-error')).toHaveTextContent('Section 2 is unreadable');
    expect(setView).not.toHaveBeenCalled();
  });

  /**
   * #576 RV4-FE-MISC4: a CANCELLED import into the project created for it is not a
   * failure. The residue prompt printed the backend's machine marker verbatim
   * ("IMPORT_CANCELLED: ...") in its error box; it now asks the same keep/discard
   * question with the cancelled wording the other import dialogs' toast uses - routed
   * on the refusal's code (#576 A6), never on its sentence.
   */
  it('words a cancelled import as cancelled in the residue prompt, never the raw marker', async () => {
    openEmptyImportProject();
    executeTmlImport.mockResolvedValue(
      refusedBy({ code: 'IMPORT_CANCELLED', message: 'the TML import was cancelled; nothing was imported' }),
    );
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    const prompt = await screen.findByTestId('tml-import-residue-dialog');
    console.log('[MISC4] residue prompt text:', prompt.textContent);
    expect(prompt).not.toHaveTextContent('IMPORT_CANCELLED');
    expect(prompt).not.toHaveTextContent('the TML import was cancelled; nothing was imported');
    expect(screen.queryByTestId('tml-import-residue-error')).toBeNull();
    expect(screen.getByTestId('tml-import-residue-cancelled')).toHaveTextContent(
      'Import cancelled. Nothing was imported.',
    );
    // Still the keep/discard question about the empty project.
    expect(screen.getByTestId('tml-import-residue-keep')).toBeInTheDocument();
    expect(screen.getByTestId('tml-import-residue-discard')).toBeInTheDocument();
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
  });

  it('raises the residue prompt on a TRANSPORT failure too, instead of stranding the empty project', async () => {
    openEmptyImportProject();
    executeTmlImport.mockRejectedValue(new Error('network gone'));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    // NOT routed to the store-level error dialog: openDialog is a single slot
    // (App.tsx mounts this component only while activeDialog === 'tml-import'),
    // so that would unmount this dialog with no stack underneath it, and
    // importContext would already be null - isEmptyImportProject could never
    // recognise the still-open empty project as residue again.
    expect(await screen.findByTestId('tml-import-residue-dialog')).toBeInTheDocument();
    expect(screen.getByTestId('tml-import-residue-error')).toHaveTextContent('network gone');
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
    expect(closeDialog).not.toHaveBeenCalled();
    // Preserved until the user actually resolves the residue (Keep or Discard).
    expect(importContext()).toBe('new-project');

    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    await waitFor(() => expect(closeProject).toHaveBeenCalled());
    expect(importContext()).toBeNull();
  });

  it('falls back to the store-level error dialog for a TRANSPORT failure with no empty project to protect', async () => {
    executeTmlImport.mockRejectedValue(new Error('network gone'));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith(
        'error',
        expect.objectContaining({ message: 'network gone' }),
      ),
    );
    expect(screen.queryByTestId('tml-import-residue-dialog')).toBeNull();
    expect(importContext()).toBeNull();
  });
});

describe('TmlImportDialog empty-project residue (#411 C2)', () => {
  it('offers keep/discard when cancelling out of a project created for this import', async () => {
    openEmptyImportProject();
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    expect(await screen.findByTestId('tml-import-residue-dialog')).toBeInTheDocument();
    expect(closeDialog).not.toHaveBeenCalled();

    await userEvent.click(screen.getByTestId('tml-import-residue-keep'));
    expect(closeProject).not.toHaveBeenCalled();
    expect(deleteProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
  });

  it('discards the deferred project by closing it and returning to the project list', async () => {
    openEmptyImportProject();
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-import-cancel'));
    await userEvent.click(await screen.findByTestId('tml-import-residue-discard'));

    await waitFor(() => expect(closeProject).toHaveBeenCalled());
    expect(deleteProject).not.toHaveBeenCalled(); // nothing on disk yet
    expect(setView).toHaveBeenCalledWith('global');
    expect(closeDialog).toHaveBeenCalled();
  });

  it('deletes the .stalx when the project was created with a save path', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-import-cancel'));
    await userEvent.click(await screen.findByTestId('tml-import-residue-discard'));

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('/tmp/new.stalx'));
    expect(closeProject).not.toHaveBeenCalled();
  });

  it('closes straight out when the project already holds sections', async () => {
    useProjectStore.setState({ project: emptyProject({ sections: [{}] as unknown as Project['sections'] }) });
    useUIStore.setState({ importContext: 'new-project' });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    expect(closeDialog).toHaveBeenCalled();
    expect(screen.queryByTestId('tml-import-residue-dialog')).toBeNull();
  });
});

describe('TmlImportDialog preview honesty (#411 C4)', () => {
  it('counts only the shots that will actually import', async () => {
    const file = withShots([
      shot({ id: 1 }),
      shot({ id: 2, excluded: true }),
      shot({ id: 3, shot_type: 'Closure' }),
    ]);
    render(<TmlImportDialog tmlFile={file} filePath="/tmp/cave.tml" />);

    // 3 raw rows, but the CLOSURE marker never imports and the excluded shot is
    // out until the toggle says otherwise.
    expect(await screen.findByTestId('tml-shot-count')).toHaveTextContent('1');
    expect(screen.getByTestId('tml-section-row-0')).toHaveTextContent('1');

    await userEvent.click(screen.getByTestId('tml-include-excluded'));
    expect(screen.getByTestId('tml-shot-count')).toHaveTextContent('2');
  });

  it('blocks the import when nothing survives the current options', async () => {
    const file = withShots([shot({ shot_type: 'Closure' }), shot({ id: 2, excluded: true })]);
    render(<TmlImportDialog tmlFile={file} filePath="/tmp/cave.tml" />);

    expect(await screen.findByTestId('tml-import-empty')).toBeInTheDocument();
    expect((screen.getByTestId('tml-import-confirm') as HTMLButtonElement).disabled).toBe(true);

    // The excluded shot is recoverable: opting it in re-enables the import.
    await userEvent.click(screen.getByTestId('tml-include-excluded'));
    expect((screen.getByTestId('tml-import-confirm') as HTMLButtonElement).disabled).toBe(false);
    expect(screen.queryByTestId('tml-import-empty')).toBeNull();
  });

  it('drops a section the toggles empty out from the section count', async () => {
    const file: TmlFile = {
      ...tmlFile,
      sections: [
        withShots([shot({ id: 1 })]).sections[0]!,
        { ...withShots([shot({ id: 2, excluded: true })]).sections[0]!, name: 'All excluded' },
      ],
    };
    render(<TmlImportDialog tmlFile={file} filePath="/tmp/cave.tml" />);

    // The converter drops a section with no shots left after filtering, so a
    // preview claiming 2 promises a section that never arrives.
    expect(await screen.findByTestId('tml-section-count')).toHaveTextContent('1');

    await userEvent.click(screen.getByTestId('tml-include-excluded'));
    expect(screen.getByTestId('tml-section-count')).toHaveTextContent('2');
  });
});

describe('TmlImportDialog import-context lifetime (#411 fix round)', () => {
  it('drops the import context when the import succeeds', async () => {
    openEmptyImportProject();
    executeTmlImport.mockResolvedValue(ok([], []));
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    // Left set, a later import whose route sets no context inherits it and takes
    // saveAfterImport's full-save branch, committing unrelated pending edits.
    await waitFor(() => expect(importContext()).toBeNull());
  });

  it('drops the import context when the user keeps the empty project', async () => {
    openEmptyImportProject();
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-import-cancel'));
    await userEvent.click(await screen.findByTestId('tml-import-residue-keep'));

    expect(importContext()).toBeNull();
  });

  it('drops the import context when the failure goes through the router', async () => {
    openEmptyImportProject();
    useUIStore.setState({ importContext: 'existing-project' }); // no residue prompt
    executeTmlImport.mockResolvedValue({
      success: false,
      error: 'nope',
      warnings: [],
      section_ids: [],
    } as unknown as CommandResult);
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() => expect(importContext()).toBeNull());
  });
});

describe('TmlImportDialog discard failures (#411 fix round)', () => {
  async function raiseResidueAndDiscard() {
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));
    await userEvent.click(await screen.findByTestId('tml-import-residue-discard'));
  }

  it('surfaces a refused delete instead of closing success-shaped', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    // The backend closes the session BEFORE handing the file to the OS trash, so
    // a denied trash returns failure with the session already gone: "it failed"
    // is not "nothing happened", and the file is still on disk and in Recents.
    deleteProject.mockResolvedValue({
      success: false,
      session_closed: true,
      error: 'Access is denied',
    } as unknown as CommandResult);

    await raiseResidueAndDiscard();

    expect(await screen.findByTestId('tml-import-residue-discard-error')).toHaveTextContent(
      'Access is denied',
    );
    expect(closeDialog).not.toHaveBeenCalled();
    expect(setView).not.toHaveBeenCalled();
    // Both buttons come back, so the user is not stuck on a dead prompt.
    expect((screen.getByTestId('tml-import-residue-discard') as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId('tml-import-residue-keep') as HTMLButtonElement).disabled).toBe(false);
  });

  it('survives a close that throws instead of stranding the prompt', async () => {
    openEmptyImportProject();
    closeProject.mockRejectedValue(new Error('ipc gone'));

    await raiseResidueAndDiscard();

    expect(await screen.findByTestId('tml-import-residue-discard-error')).toHaveTextContent('ipc gone');
    expect(closeDialog).not.toHaveBeenCalled();
    expect((screen.getByTestId('tml-import-residue-discard') as HTMLButtonElement).disabled).toBe(false);
  });

  it('re-checks the delete condition at click time, not just when the prompt opened', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    // Something landed in the project between raising the prompt and the click -
    // this arm calls the app's only file-deleting API, so it must bail.
    useProjectStore.setState({
      project: emptyProject({ sections: [{}] as unknown as Project['sections'], file_path: '/tmp/new.stalx' }),
    });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    expect(deleteProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    // This bail is a flow exit too: a stale 'new-project' would make the next
    // import that sets no context of its own (GPX/Compass/Walls) inherit it and
    // take saveAfterImport's full-save branch, committing unrelated pending
    // edits (#411).
    expect(importContext()).toBeNull();
  });

  it('bails when the open project is no longer the one the prompt was raised for', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    // A different project is open now - equally new and equally empty, so only
    // its identity says the snapshot no longer describes it.
    useProjectStore.setState({
      project: emptyProject({ created_at: '2026-08-05T09:00:00Z', file_path: '/tmp/other.stalx' }),
    });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    expect(deleteProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(importContext()).toBeNull();
  });

  // [#576 RVF-c-A2-4] Two copies of one project file share `created_at`: a switch to
  // another copy (another file, a new session) is another project too.
  it('bails when another copy of the project the prompt was raised for is open now', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    noteProjectSessionChanged();
    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/new copy.stalx' }) });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    expect(deleteProject, 'deleted the copy the prompt was raised for').not.toHaveBeenCalled();
    expect(closeProject, 'closed the other copy').not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(importContext()).toBeNull();
  });

  // [#576 W50-NF1] A Save As moves the project to another file within its session: the
  // file the prompt captured is then the copy left behind, no longer the open project's.
  // Which file a Discard should delete then is not settled; it deletes none.
  it('bails when a Save As moved the project to another file since the prompt', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/saved as.stalx' }) }); // same session
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    console.log(`[W50-NF1] Save As, then Discard: deleteProject calls ${JSON.stringify(deleteProject.mock.calls)}`);
    expect(deleteProject, 'deleted the copy the Save As left behind').not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(importContext()).toBeNull();
  });

  it('bails when the copy a Save As left behind is open again', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    // Save As, then the session ends on the saved-as file and the copy left behind is reopened.
    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/saved as.stalx' }) });
    noteProjectSessionChanged();
    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/new.stalx' }) });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    console.log(`[W50-NF1] Save As, reopen the old copy, Discard: deleteProject calls ${JSON.stringify(deleteProject.mock.calls)}`);
    expect(deleteProject, 'deleted the copy the Save As left behind').not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(importContext()).toBeNull();
  });

  // [#576 W54-NF1] A save in place since the prompt wrote the deferred project to the
  // file the backend reserved for it: the project's own file, still in its session.
  // Closing it (the deferred arm the snapshot picked) left that empty .stalx behind.
  it('deletes the file a save in place wrote since the prompt of a deferred project', async () => {
    openEmptyImportProject();
    useProjectStore.setState({ databaseOpen: false, project: emptyProject({ file_path: '/tmp/reserved.stalx' }) });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    // What a save in place leaves: the same project and file, now written.
    useProjectStore.setState({ databaseOpen: true });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('/tmp/reserved.stalx'));
    expect(closeProject, 'closed it and left the written file behind').not.toHaveBeenCalled();
  });

  // [#576 Q-W54-2-D1, W54-NF] Pin of the decision as a whole: a save in place deletes
  // the file it wrote, and a Save As after it still deletes nothing - neither the
  // reserved file the save in place wrote (the copy left behind now) nor the file the
  // Save As named.
  it('a save in place, then a Save As: deletes nothing and closes nothing', async () => {
    openEmptyImportProject();
    useProjectStore.setState({ databaseOpen: false, project: emptyProject({ file_path: '/tmp/reserved.stalx' }) });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    useProjectStore.setState({ databaseOpen: true }); // the save in place
    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/saved as.stalx' }) }); // the Save As
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    expect(deleteProject).not.toHaveBeenCalled();
    expect(closeProject).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(importContext()).toBeNull();
  });

  it('still deletes after a reload of the same file, with no Save As', async () => {
    openEmptyImportProject();
    useProjectStore.setState({
      databaseOpen: true,
      project: emptyProject({ file_path: '/tmp/new.stalx' }),
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);
    await userEvent.click(await screen.findByTestId('tml-import-cancel'));

    noteProjectSessionChanged();
    useProjectStore.setState({ project: emptyProject({ file_path: '/tmp/new.stalx' }) });
    await userEvent.click(screen.getByTestId('tml-import-residue-discard'));

    await waitFor(() => expect(deleteProject).toHaveBeenCalledWith('/tmp/new.stalx'));
  });
});

describe('TmlImportDialog failure after unmount (#411 fix round)', () => {
  it('falls back to the store-level router when the in-dialog surface is gone', async () => {
    openEmptyImportProject();
    let settle: (result: CommandResult) => void = () => {};
    executeTmlImport.mockReturnValue(new Promise<CommandResult>((resolve) => { settle = resolve; }));
    const view = render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();
    // A drop of another importable file re-routes the dialog slot mid-import.
    view.unmount();
    settle({ success: false, error: 'late failure', warnings: [], section_ids: [] } as unknown as CommandResult);

    // setResidue on an unmounted component is a no-op, i.e. silence - the router
    // is store-level and always lands.
    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith(
        'error',
        expect.objectContaining({ title: 'Import Errors', message: 'late failure' }),
      ),
    );
    expect(importContext()).toBeNull();
  });

  it('stamps the same completion telemetry whichever surface takes the failure', async () => {
    openEmptyImportProject();
    executeTmlImport.mockResolvedValue({
      success: false,
      error: 'nope',
      warnings: [],
      section_ids: [],
    } as unknown as CommandResult);
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    // The residue prompt IS this dialog's completion; without this it would be
    // recorded as a cancel while the router records a complete for the same event.
    expect(await screen.findByTestId('tml-import-residue-dialog')).toBeInTheDocument();
    expect(trackDialogCompleteMock).toHaveBeenCalledWith('tml-import');
  });
});

/**
 * #425: the section filter and the placement step. The topology consequences
 * are covered on both sides of the wire (import_options.rs, tmlPreviewLayout);
 * these pin what the DIALOG sends and what it refuses to send.
 */
describe('TmlImportDialog section selection and placement (#425)', () => {
  /** Two sections, the second hanging off the first's only shot. */
  const twoSections = (over: Partial<TmlShot> = {}): TmlFile => ({
    ...tmlFile,
    sections: [
      {
        name: 'A',
        shots: [shot({ id: 1, from_id: 0, ...over })],
        surveyor: null,
        date: null,
        color: '#3b82f6',
        is_branch_split: false,
        start_ariane_id: 0,
        end_ariane_id: 1,
        start_depth: 0,
      },
      {
        name: 'B',
        shots: [shot({ id: 2, from_id: 1 })],
        surveyor: null,
        date: null,
        color: '#ef4444',
        is_branch_split: false,
        start_ariane_id: 1,
        end_ariane_id: 2,
        start_depth: 0,
      },
    ],
  });

  const lastImportArgs = (): unknown[] =>
    executeTmlImport.mock.calls[executeTmlImport.mock.calls.length - 1] ?? [];

  it('sends no section list while every section is ticked', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    await clickImport();
    // arg 5 is selectedSections: null keeps a full import unfiltered on the wire.
    expect(lastImportArgs()[5]).toBeNull();
  });

  it('sends the remaining indices once a section is unticked', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-section-0'));
    await clickImport();
    expect(lastImportArgs()[5]).toEqual([1]);
  });

  it('does not call the NEW root re-anchored when the old root is unticked', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    // B hangs off A; unticking A leaves B as the new ROOT, which is not a lost
    // anchor - so nothing is flagged.
    await userEvent.click(await screen.findByTestId('tml-section-0'));
    expect(screen.queryByTestId('tml-reanchored-notice')).toBeNull();

    // Unticking A's child instead cannot strand anything either.
    await userEvent.click(await screen.findByTestId('tml-section-0'));
    await userEvent.click(await screen.findByTestId('tml-section-1'));
    expect(screen.queryByTestId('tml-reanchored-notice')).toBeNull();
  });

  it('flags the section stranded by a deselected middle section', async () => {
    const three: TmlFile = {
      ...tmlFile,
      sections: [
        ...twoSections().sections,
        {
          name: 'C',
          shots: [shot({ id: 3, from_id: 2 })],
          surveyor: null,
          date: null,
          color: '#22c55e',
          is_branch_split: false,
          start_ariane_id: 2,
          end_ariane_id: 3,
          start_depth: 0,
        },
      ],
    };
    render(<TmlImportDialog tmlFile={three} filePath="/tmp/cave.tml" />);

    // Drop B: C hangs off B, and A is still the root, so C is stranded.
    await userEvent.click(await screen.findByTestId('tml-section-1'));
    expect(await screen.findByTestId('tml-reanchored-notice')).toBeInTheDocument();
    expect(screen.getByTestId('tml-section-reanchored-2')).toBeInTheDocument();
  });

  it('disables Import when every section is unticked', async () => {
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-select-all'));
    expect(await screen.findByTestId('tml-import-empty')).toBeInTheDocument();
    expect((screen.getByTestId('tml-import-confirm') as HTMLButtonElement).disabled).toBe(true);
    expect(executeTmlImport).not.toHaveBeenCalled();
  });

  it('sends an explicit origin placement for a file with no georeference', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    // No GPS anywhere, so the file option is not even offered.
    expect(screen.queryByTestId('tml-placement-file')).toBeNull();
    await clickImport();
    // arg 6 is rootPlacement: the choice is now stated rather than defaulted
    // into silently.
    expect(lastImportArgs()[6]).toEqual({ type: 'origin' });
  });

  it("offers the file's own coordinates and sends them as a zero datum", async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(
      <TmlImportDialog
        tmlFile={twoSections({ latitude: 20.5, longitude: -87.1 })}
        filePath="/tmp/cave.tml"
      />
    );

    expect((await screen.findByTestId('tml-placement-file')) as HTMLInputElement).toBeChecked();
    await clickImport();
    expect(lastImportArgs()[6]).toEqual({
      type: 'zero_datum',
      latitude: 20.5,
      longitude: -87.1,
    });
  });

  it('holds Import while typed coordinates do not parse, and releases it when they do', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={twoSections()} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-placement-manual'));
    // Chosen but empty: not an answer, so the button must not offer to send one.
    expect((screen.getByTestId('tml-import-confirm') as HTMLButtonElement).disabled).toBe(true);

    await userEvent.type(screen.getByTestId('tml-placement-latitude'), '91');
    await userEvent.type(screen.getByTestId('tml-placement-longitude'), '0');
    expect(screen.getByTestId('tml-placement-error')).toBeInTheDocument();
    expect((screen.getByTestId('tml-import-confirm') as HTMLButtonElement).disabled).toBe(true);

    await userEvent.clear(screen.getByTestId('tml-placement-latitude'));
    await userEvent.type(screen.getByTestId('tml-placement-latitude'), '20.5');
    expect(screen.queryByTestId('tml-placement-error')).toBeNull();

    await clickImport();
    expect(lastImportArgs()[6]).toEqual({
      type: 'zero_datum',
      latitude: 20.5,
      longitude: 0,
    });
  });
});

describe('TmlImportDialog placement is not walked over by other controls (#425)', () => {
  const geoFile: TmlFile = {
    ...tmlFile,
    sections: [
      {
        name: 'A',
        shots: [shot({ id: 1, from_id: 0, latitude: 20.5, longitude: -87.1 })],
        surveyor: null,
        date: null,
        color: '#3b82f6',
        is_branch_split: false,
        start_ariane_id: 0,
        end_ariane_id: 1,
        start_depth: 0,
      },
      {
        name: 'B',
        shots: [shot({ id: 2, from_id: 1 })],
        surveyor: null,
        date: null,
        color: '#ef4444',
        is_branch_split: false,
        start_ariane_id: 1,
        end_ariane_id: 2,
        start_depth: 0,
      },
    ],
  };

  it('keeps an explicit Origin choice when a section is unticked afterwards', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={geoFile} filePath="/tmp/cave.tml" />);

    // The file offers coordinates, so 'file' is the seeded default; override it.
    await userEvent.click(await screen.findByTestId('tml-placement-origin'));
    // Unticking B recomputes the root's anchor - which used to re-seed the
    // default and silently put the survey back on the file's coordinates.
    await userEvent.click(screen.getByTestId('tml-section-1'));

    expect((screen.getByTestId('tml-placement-origin') as HTMLInputElement)).toBeChecked();
    await clickImport();
    expect(executeTmlImport.mock.calls[executeTmlImport.mock.calls.length - 1]?.[6]).toEqual({
      type: 'origin',
    });
  });

  it('falls back to Origin when the section carrying the coordinates is unticked', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={geoFile} filePath="/tmp/cave.tml" />);

    expect((await screen.findByTestId('tml-placement-file')) as HTMLInputElement).toBeChecked();
    await userEvent.click(screen.getByTestId('tml-section-0'));

    // The option is gone with its section; leaving `choice` on it would disable
    // Import with nothing on screen explaining why.
    expect(screen.queryByTestId('tml-placement-file')).toBeNull();
    expect((screen.getByTestId('tml-placement-origin') as HTMLInputElement)).toBeChecked();
    await clickImport();
    expect(executeTmlImport.mock.calls[executeTmlImport.mock.calls.length - 1]?.[6]).toEqual({
      type: 'origin',
    });
  });
});

describe('TmlImportDialog root agreement (#425 review)', () => {
  /** Section 0 is emptied by the exclusion filter, so it is never created. */
  const firstSectionAllExcluded: TmlFile = {
    ...tmlFile,
    sections: [
      {
        name: 'Dropped',
        shots: [shot({ id: 1, from_id: 0, excluded: true })],
        surveyor: null,
        date: null,
        color: '#3b82f6',
        is_branch_split: false,
        start_ariane_id: 0,
        end_ariane_id: 1,
        start_depth: 0,
      },
      {
        name: 'Survivor',
        shots: [shot({ id: 2, from_id: 1 })],
        surveyor: null,
        date: null,
        color: '#ef4444',
        is_branch_split: false,
        start_ariane_id: 1,
        end_ariane_id: 2,
        start_depth: 0,
      },
    ],
  };

  it('anchors the placement step to the surviving section and does not also call it stranded', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={firstSectionAllExcluded} filePath="/tmp/cave.tml" />);

    // The step names the section the import will actually create...
    expect(await screen.findByTestId('tml-placement-step')).toHaveTextContent('Survivor');
    // ...and cannot simultaneously badge it as landing at the origin for want
    // of a connection. Both used to be on screen at once, because the preview
    // layout still called section 0 the root.
    expect(screen.queryByTestId('tml-reanchored-notice')).toBeNull();
    expect(screen.queryByTestId('tml-section-reanchored-1')).toBeNull();
  });

  it('sends the typed datum, which the backend applies to that same section', async () => {
    executeTmlImport.mockResolvedValue(ok([], [1]));
    render(<TmlImportDialog tmlFile={firstSectionAllExcluded} filePath="/tmp/cave.tml" />);

    await userEvent.click(await screen.findByTestId('tml-placement-manual'));
    await userEvent.type(screen.getByTestId('tml-placement-latitude'), '20.5');
    await userEvent.type(screen.getByTestId('tml-placement-longitude'), '-87.1');
    await clickImport();

    expect(executeTmlImport.mock.calls[executeTmlImport.mock.calls.length - 1]?.[6]).toEqual({
      type: 'zero_datum',
      latitude: 20.5,
      longitude: -87.1,
    });
  });

  it('does not offer coordinates the backend would refuse', async () => {
    // A crafted file with NaN on the root's first shot. `Math.abs(NaN) > 0` is
    // false, so a non-zero test alone reports it as an anchor - the same hole
    // the Rust null-island guard had.
    const crafted: TmlFile = {
      ...tmlFile,
      sections: [
        {
          ...firstSectionAllExcluded.sections[1]!,
          name: 'Crafted',
          shots: [shot({ id: 1, from_id: 0, latitude: Number.NaN, longitude: Number.NaN })],
          start_ariane_id: 0,
        },
      ],
    };
    render(<TmlImportDialog tmlFile={crafted} filePath="/tmp/cave.tml" />);

    await screen.findByTestId('tml-placement-step');
    expect(screen.queryByTestId('tml-placement-file')).toBeNull();
    expect(screen.getByTestId('tml-placement-origin')).toBeChecked();
  });
});

describe('TmlImportDialog reads the ids the wire actually delivers (#566)', () => {
  /**
   * Tauri's IPC is JSON and serde writes an `i64` as a JSON number, so the
   * shipped app hands this dialog plain numbers - and a lookup that only worked
   * for the bigints the generated types used to declare made every non-root
   * section look stranded the moment the dialog opened. The types say `number`
   * since #576 C6; this pins the dialog on a file built straight from them.
   */
  const wireIds: TmlFile = {
    ...tmlFile,
    sections: [
      {
        name: 'A',
        shots: [{ ...shot(), id: 1, from_id: 0 }],
        surveyor: null,
        date: null,
        color: '#3b82f6',
        is_branch_split: false,
        start_ariane_id: 0,
        end_ariane_id: 1,
        start_depth: 0,
      },
      {
        name: 'B',
        shots: [{ ...shot(), id: 2, from_id: 1 }],
        surveyor: null,
        date: null,
        color: '#ef4444',
        is_branch_split: false,
        start_ariane_id: 1,
        end_ariane_id: 2,
        start_depth: 0,
      },
    ],
  };

  it('does not call a connected section stranded on a file straight off the wire', async () => {
    render(<TmlImportDialog tmlFile={wireIds} filePath="/tmp/cave.tml" />);

    await screen.findByTestId('tml-placement-step');
    // B hangs off A and both are selected, so nothing is going to the origin.
    // Before the fix this notice was on screen for every multi-section file the
    // real app opened, contradicting an import that connected them fine.
    expect(screen.queryByTestId('tml-reanchored-notice')).toBeNull();
    expect(screen.queryByTestId('tml-section-reanchored-1')).toBeNull();
  });

  it('still flags a genuinely stranded section on wire ids', async () => {
    render(<TmlImportDialog tmlFile={wireIds} filePath="/tmp/cave.tml" />);

    // Deselecting A leaves B as the new root, which is not stranded...
    await userEvent.click(await screen.findByTestId('tml-section-0'));
    expect(screen.queryByTestId('tml-reanchored-notice')).toBeNull();
  });
});

/**
 * #576 NF-DMPS-1: a rejected `suggest_orphan_placements` was swallowed
 * ("non-fatal"), so the step that offers to attach unanchored sections was
 * skipped without a word. The DMP wizard says so (NF-AN4-N4); this importer
 * now does too, through the shared helper, and the import still completes.
 */
describe('TmlImportDialog orphan lookup failure (#576 NF-DMPS-1)', () => {
  it('says so in a warning toast and still completes the import', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    executeTmlImport.mockResolvedValue(ok([], [1]));
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'suggest_orphan_placements') throw new Error('orphan matcher unavailable (test)');
      return 0; // count_unanchored_sections
    });
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await clickImport();

    await waitFor(() => expect(closeDialog).toHaveBeenCalled()); // the import completed
    expect(showToast).toHaveBeenCalledWith(
      'Imported, but unattached sections could not be matched: orphan matcher unavailable (test)',
      'orphan-lookup-failed',
      'warning',
    );
    expect(openDialog).not.toHaveBeenCalledWith('orphan-placement', expect.anything());
  });
});

// #576 FZ-PARSE-5: a reading beyond the 10,000 km bound is read as missing by
// the parser, which names it on `TmlFile.warnings`. The preview must show those
// lines before Import is pressed, and show nothing when there are none.
describe('TmlImportDialog parser warnings (#576 FZ-PARSE-5)', () => {
  const WARNINGS = [
    'Shot 2: depth 1e308 m is beyond the 10,000 km limit no real survey reaches; read as missing.',
    'Shot 3: length 1.7977e308 m is beyond the 10,000 km limit no real survey reaches; read as missing.',
  ];

  it('lists every parser warning line in the preview', async () => {
    const file: TmlFile = { ...withShots([shot()]), warnings: WARNINGS };
    render(<TmlImportDialog tmlFile={file} filePath="/tmp/cave.tml" />);

    const block = await screen.findByTestId('tml-parse-warnings');
    for (const line of WARNINGS) expect(block).toHaveTextContent(line);
    expect(block.children).toHaveLength(WARNINGS.length);
    expect(executeTmlImport).not.toHaveBeenCalled();
  });

  it('renders no warnings block when the file carries no warnings', async () => {
    render(<TmlImportDialog tmlFile={withShots([shot()])} filePath="/tmp/cave.tml" />);

    await screen.findByTestId('tml-shot-count');
    expect(screen.queryByTestId('tml-parse-warnings')).toBeNull();
  });

  it('renders no warnings block for an empty warnings list', async () => {
    const file: TmlFile = { ...withShots([shot()]), warnings: [] };
    render(<TmlImportDialog tmlFile={file} filePath="/tmp/cave.tml" />);

    await screen.findByTestId('tml-shot-count');
    expect(screen.queryByTestId('tml-parse-warnings')).toBeNull();
  });
});
