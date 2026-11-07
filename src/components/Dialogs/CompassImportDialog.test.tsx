/**
 * Component test (#110) - Compass import warning surfacing.
 *
 * CompassImportDialog's warning-surfacing block is a copy-paste sibling of TML's
 * and can drift independently, so it gets its own coverage: the deferred-save
 * partial-success warning must surface before the orphan step, while routine
 * per-shot validation warnings on a fully-saved import must NOT modal or skip
 * orphan placement. Helper filtering is unit-tested in saveFeedback.test.ts.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }),
);
vi.mock('../../utils/ipc');

import CompassImportDialog from './CompassImportDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult, CompassFile, ValidationWarning } from '../../types/commands';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';
import { invokeIpcMock } from '../../test/factories/ipc';

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

const ZERO_INVALID_COUNTS = {
  non_excluded_corrected: 0,
  non_excluded_uncorrected: 0,
  excluded_corrected: 0,
  excluded_uncorrected: 0,
};

const compassFile: CompassFile = {
  cave_name: 'Test Cave',
  survey_count: 0,
  shot_count: 0,
  excluded_shot_count: 0,
  invalid_shot_counts: ZERO_INVALID_COUNTS,
  has_backsights: false,
  datum: null,
  utm_zone: null,
  has_georeference: false,
  declination_mode: 'File',
  length_unit: null,
  surveys: [],
  warnings: [],
};

const ok = (warnings: ValidationWarning[], section_ids: number[] = []): CommandResult =>
  ({ success: true, error: null, warnings, section_ids } as CommandResult);

/** What the backend reports for a file whose every survey was dropped (#411). */
const EMPTY_RECEIPT: ImportReceipt = {
  source_format: 'Compass',
  file_name: 'cave.dat',
  sections_found: 3,
  sections_imported: 0,
  shot_count: 0,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: ['3 surveys had no shots after filtering and were dropped'],
  info: [],
  zero_datum_created: null,
};

/**
 * The wire shape that CRASHED this dialog in #411: `section_ids` was
 * `#[serde(skip_serializing_if = "Vec::is_empty")]`, so an import that created
 * no section omitted the key while the generated type promised an array. #441
 * removed the skip, so the backend always writes the key and this shape is no
 * longer reachable from the shipped binary - the literal is kept as the
 * regression record for #411 and pins the `?? []` that survives it.
 */
const okWithoutSectionIds = (receipt: ImportReceipt): CommandResult =>
  ({ success: true, error: null, warnings: [], import_receipt: receipt } as unknown as CommandResult);

let executeCompassImport: ReturnType<typeof vi.fn>;
let openDialog: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  closeDialog = vi.fn();
  executeCompassImport = vi.fn();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue([]); // suggest_orphan_placements → no suggestions
  useProjectStore.setState({
    executeCompassImport,
    calculateFileHash: vi.fn(async () => 'hash'),
    checkDuplicateImport: vi.fn(async () => false),
    startLoading: vi.fn(() => 0),
    setLoadingProgress: vi.fn(),
    resetLoading: vi.fn(),
  });
  useUIStore.setState({
    openDialog,
    closeDialog,
    setView: vi.fn(),
    triggerFitToBounds: vi.fn(),
  });
});

async function clickImport() {
  const btn = (await screen.findByTestId('compass-import-confirm')) as HTMLButtonElement;
  await waitFor(() => expect(btn.disabled).toBe(false));
  await userEvent.click(btn);
}

const orphanCalled = () =>
  invokeIpcMock.mock.calls.some(([cmd]) => cmd === 'suggest_orphan_placements');

describe('CompassImportDialog warning surfacing (#110)', () => {
  it('surfaces the deferred-save warning before orphan placement, and skips orphan', async () => {
    executeCompassImport.mockResolvedValue(ok([SAVE_WARNING], [1]));
    render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.dat" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('error', expect.objectContaining({ title: 'Import Warning' })),
    );
    expect(orphanCalled()).toBe(false);
  });

  it('does NOT modal a routine validation warning on a saved import - orphan still runs', async () => {
    executeCompassImport.mockResolvedValue(ok([VALIDATION_WARNING], [1]));
    render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.dat" />);

    await clickImport();

    await waitFor(() => expect(orphanCalled()).toBe(true));
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.objectContaining({ title: 'Import Warning' }));
  });

  it('shows the receipt when the import created nothing and section_ids is absent (#411)', async () => {
    executeCompassImport.mockResolvedValue(okWithoutSectionIds(EMPTY_RECEIPT));
    render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.dat" />);

    await clickImport();

    await waitFor(() =>
      expect(openDialog).toHaveBeenCalledWith('import-receipt', { receipt: EMPTY_RECEIPT }),
    );
    // Not a transport failure: reading the absent key used to throw, and the
    // catch reported the empty import as a broken IPC call.
    expect(openDialog).not.toHaveBeenCalledWith('error', expect.anything());
    expect(orphanCalled()).toBe(false);
  });
});

/**
 * #437 (+ review of #550): the header "Shots" stat must show what the
 * backend will actually land under BOTH live toggles - include-excluded AND
 * apply-corrections - with a footnote naming what was dropped and why. The
 * per-survey rows use the same arithmetic and must sum to the header, in
 * all four toggle combinations.
 *
 * Fixture: survey A has a shot that is invalid only WITHOUT corrections and
 * a `#|X#`-flagged shot invalid either way; survey B has a shot invalid
 * only WITH corrections. Totals: excluded_shot_count 2,
 * invalid_shot_counts { non_excluded_corrected: 1, non_excluded_uncorrected:
 * 2, excluded_corrected: 1, excluded_uncorrected: 1 }.
 *
 * Injection: flip `landingShotCountFor`'s
 * `includeExcluded ? 0 : excludedShotCount` term at CompassImportDialog.tsx
 * and the default-toggle assertion goes red; flip `invalidShotCountFor`'s
 * `applyCorrections ? ... : ...` selection and the corrections-toggle
 * assertion goes red.
 */
describe('CompassImportDialog honest shot count (#437, #550)', () => {
  const twoSurveyFile: CompassFile = {
    ...compassFile,
    shot_count: 6,
    excluded_shot_count: 2,
    invalid_shot_counts: {
      non_excluded_corrected: 1,
      non_excluded_uncorrected: 2,
      excluded_corrected: 1,
      excluded_uncorrected: 1,
    },
    surveys: [
      {
        name: 'A',
        date: '2026-01-01',
        team: null,
        shot_count: 3,
        excluded_shot_count: 1,
        invalid_shot_counts: {
          non_excluded_corrected: 0,
          non_excluded_uncorrected: 1,
          excluded_corrected: 1,
          excluded_uncorrected: 1,
        },
        declination: 0,
      },
      {
        name: 'B',
        date: '2026-01-01',
        team: null,
        shot_count: 3,
        excluded_shot_count: 1,
        invalid_shot_counts: {
          non_excluded_corrected: 1,
          non_excluded_uncorrected: 1,
          excluded_corrected: 0,
          excluded_uncorrected: 0,
        },
        declination: 0,
      },
    ],
  };

  it('shows the landing count with an excluded+invalid footnote by default (corrections on)', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    // 6 raw - 2 excluded - 1 invalid(non_excluded_corrected) = 3 landing.
    expect(await screen.findByTestId('compass-shot-count')).toHaveTextContent('3');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent('2 excluded, 1 invalid');

    // A: 3 - 1 excluded - 0 invalid = 2; B: 3 - 1 excluded - 1 invalid = 1.
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('2');
    expect(screen.getByTestId('compass-survey-shots-1')).toHaveTextContent('1');
    expect(2 + 1).toBe(3);
  });

  it('shows the invalid-only footnote (no excluded part) once include-excluded is checked', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-include-excluded'));

    // 6 raw - 0 excluded (included) - (1 non_excluded_corrected + 1 excluded_corrected) = 4.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('4');
    const footnote = screen.getByTestId('compass-shot-count-footnote');
    expect(footnote).toHaveTextContent('2 invalid');
    expect(footnote).not.toHaveTextContent('excluded');

    // A: 3 - 0 - (0 + 1) = 2; B: 3 - 0 - (1 + 0) = 2.
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('2');
    expect(screen.getByTestId('compass-survey-shots-1')).toHaveTextContent('2');
    expect(2 + 2).toBe(4);
  });

  it('switches to the uncorrected invalid counter once apply-corrections is unchecked', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-apply-corrections'));

    // 6 raw - 2 excluded - 2 invalid(non_excluded_uncorrected) = 2 landing.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('2');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent('2 excluded, 2 invalid');
  });

  it('combines include-excluded and apply-corrections off correctly', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-include-excluded'));
    await userEvent.click(screen.getByTestId('compass-apply-corrections'));

    // 6 raw - 0 excluded (included) - (2 non_excluded_uncorrected + 1 excluded_uncorrected) = 3.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('3');
    const footnote = screen.getByTestId('compass-shot-count-footnote');
    expect(footnote).toHaveTextContent('3 invalid');
    expect(footnote).not.toHaveTextContent('excluded');
  });
});

/** #576 NF-DMPS-1: the orphan lookup failing is said, not swallowed (see the TML sibling). */
describe('CompassImportDialog orphan lookup failure (#576 NF-DMPS-1)', () => {
  it('says so in a warning toast and still completes the import', async () => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    vi.spyOn(console, 'warn').mockImplementation(() => {});
    executeCompassImport.mockResolvedValue(ok([], [1]));
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'suggest_orphan_placements') throw new Error('orphan matcher unavailable (test)');
      return 0; // count_unanchored_sections
    });
    render(<CompassImportDialog compassFile={compassFile} filePath="/tmp/cave.dat" />);

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
