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

vi.mock('../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
  setSource: vi.fn(),
}));
vi.mock('../../utils/ipc', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/ipc')>()),
  invokeIpc: vi.fn(),
}));

import CompassImportDialog from './CompassImportDialog';
import { invokeIpc } from '../../utils/ipc';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult, CompassFile, ValidationWarning } from '../../types/commands';
import type { ImportReceipt } from '../../types/ipc/ImportReceipt';

const invokeIpcMock = vi.mocked(invokeIpc);

const SAVE_WARNING: ValidationWarning = {
  code: 'SYSTEM',
  message:
    'Imported, but could not save the new project to disk: disk full. ' +
    'Your changes are unsaved - please save manually.',
  severity: 'warning',
};
const VALIDATION_WARNING: ValidationWarning = {
  code: 'LONG_SHOT',
  message: 'Shot Main: 4 \u2192 5 length 142.0m above maximum threshold 120.0m',
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
  skipped_shot_counts: ZERO_INVALID_COUNTS,
  has_backsights: false,
  datum: null,
  utm_zone: null,
  has_georeference: false,
  declination_mode: 'File',
  length_unit: null,
  surveys: [],
  warnings: [],
};

const ok = (warnings: ValidationWarning[], line_ids: number[] = []): CommandResult =>
  ({ success: true, error: null, warnings, line_ids } as CommandResult);

/** What the backend reports for a file whose every survey was dropped (#411). */
const EMPTY_RECEIPT: ImportReceipt = {
  source_format: 'Compass',
  file_name: 'cave.dat',
  lines_found: 3,
  lines_imported: 0,
  shot_count: 0,
  closures_requested: 0,
  closures_created: 0,
  unit_conversion: null,
  warnings: ['3 surveys had no shots after filtering and were dropped'],
  info: [],
  zero_datum_created: null,
};

/**
 * The wire shape that CRASHED this dialog in #411: `line_ids` was
 * `#[serde(skip_serializing_if = "Vec::is_empty")]`, so an import that created
 * no line omitted the key while the generated type promised an array. #441
 * removed the skip, so the backend always writes the key and this shape is no
 * longer reachable from the shipped binary - the literal is kept as the
 * regression record for #411 and pins the `?? []` that survives it.
 */
const okWithoutLineIds = (receipt: ImportReceipt): CommandResult =>
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

  it('shows the receipt when the import created nothing and line_ids is absent (#411)', async () => {
    executeCompassImport.mockResolvedValue(okWithoutLineIds(EMPTY_RECEIPT));
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
 * #437 (+ review of #550, #818): the header "Shots" stat must show what the
 * backend will actually land under the include-excluded toggle (zero-length
 * shots are no longer dropped, they land as 1 mm ties), with a footnote
 * naming the excluded shots and the tie count, which still follows BOTH live
 * toggles. The per-survey rows use the same arithmetic and must sum to the
 * header, in all four toggle combinations.
 *
 * Fixture: survey A has a shot that is invalid only WITHOUT corrections and
 * a `#|X#`-flagged shot invalid either way; survey B has a shot invalid
 * only WITH corrections. Totals: excluded_shot_count 2,
 * invalid_shot_counts { non_excluded_corrected: 1, non_excluded_uncorrected:
 * 2, excluded_corrected: 1, excluded_uncorrected: 1 }.
 *
 * Injection: flip `landingShotCountFor`'s
 * `includeExcluded ? 0 : excludedShotCount` term at CompassImportDialog.tsx
 * and the default-toggle assertion goes red; re-add a
 * `- noLengthShotCountFor(invalid_shot_counts...)` subtraction and the count
 * assertions go red; flip `noLengthShotCountFor`'s `applyCorrections ? ... : ...` selection and
 * the corrections-toggle footnote assertion goes red.
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
        skipped_shot_counts: ZERO_INVALID_COUNTS,
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
        skipped_shot_counts: ZERO_INVALID_COUNTS,
        declination: 0,
      },
    ],
  };

  it('shows the landing count with an excluded+tie footnote by default (corrections on)', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    // 6 raw - 2 excluded = 4 landing; the 1 non_excluded_corrected tie lands.
    expect(await screen.findByTestId('compass-shot-count')).toHaveTextContent('4');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent(
      '2 excluded, 1 shot with no length, imported as a 1 mm tie'
    );

    // A: 3 - 1 excluded = 2; B: 3 - 1 excluded = 2.
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('2');
    expect(screen.getByTestId('compass-survey-shots-1')).toHaveTextContent('2');
    expect(2 + 2).toBe(4);
  });

  it('shows the tie-only footnote (no excluded part) once include-excluded is checked', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-include-excluded'));

    // 6 raw - 0 excluded (included) = 6; ties (1 + 1 corrected) all land.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('6');
    const footnote = screen.getByTestId('compass-shot-count-footnote');
    expect(footnote).toHaveTextContent('2 shots with no length, imported as 1 mm ties');
    expect(footnote).not.toHaveTextContent('excluded');

    // A: 3 - 0 = 3; B: 3 - 0 = 3.
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('3');
    expect(screen.getByTestId('compass-survey-shots-1')).toHaveTextContent('3');
    expect(3 + 3).toBe(6);
  });

  it('switches to the uncorrected invalid counter once apply-corrections is unchecked', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-apply-corrections'));

    // 6 raw - 2 excluded = 4 landing; 2 non_excluded_uncorrected ties.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('4');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent(
      '2 excluded, 2 shots with no length, imported as 1 mm ties'
    );
  });

  it('combines include-excluded and apply-corrections off correctly', async () => {
    render(<CompassImportDialog compassFile={twoSurveyFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-include-excluded'));
    await userEvent.click(screen.getByTestId('compass-apply-corrections'));

    // 6 raw - 0 excluded (included) = 6; 2 + 1 uncorrected ties land.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('6');
    const footnote = screen.getByTestId('compass-shot-count-footnote');
    expect(footnote).toHaveTextContent('3 shots with no length, imported as 1 mm ties');
    expect(footnote).not.toHaveTextContent('excluded');
  });
});

/**
 * A station tied to itself with no length is skipped by the converter, not
 * tied (#818 keeps that skip), so the landing count drops it under the live
 * toggles, the per-survey row too, and the footnote names it.
 *
 * Fixture: 5 shots, 1 excluded, no ties; skipped { non_excluded_corrected 1,
 * non_excluded_uncorrected 2, excluded_corrected 1, excluded_uncorrected 0 }.
 *
 * Injection: drop the `- noLengthShotCountFor(skippedCounts...)` term from
 * `landingShotCountFor` and every count goes red; read the skipped counters
 * without the toggles (or the tie counters instead) and the corrections or
 * include-excluded case goes red; drop the `skippedSuffix` part and the
 * footnote assertions go red.
 */
describe('CompassImportDialog skipped stations tied to themselves (#818)', () => {
  const skipped = {
    non_excluded_corrected: 1,
    non_excluded_uncorrected: 2,
    excluded_corrected: 1,
    excluded_uncorrected: 0,
  };
  const selfTieFile: CompassFile = {
    ...compassFile,
    survey_count: 1,
    shot_count: 5,
    excluded_shot_count: 1,
    skipped_shot_counts: skipped,
    surveys: [
      {
        name: 'A',
        date: '2026-01-01',
        team: null,
        shot_count: 5,
        excluded_shot_count: 1,
        invalid_shot_counts: ZERO_INVALID_COUNTS,
        skipped_shot_counts: skipped,
        declination: 0,
      },
    ],
  };

  it('leaves out the skipped shot by default (corrections on)', async () => {
    render(<CompassImportDialog compassFile={selfTieFile} filePath="/tmp/cave.dat" />);

    // 5 raw - 1 excluded - 1 skipped = 3.
    expect(await screen.findByTestId('compass-shot-count')).toHaveTextContent('3');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent(
      '1 excluded, 1 skipped: a station tied to itself'
    );
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('3');
  });

  it('switches to the uncorrected skipped counter once apply-corrections is unchecked', async () => {
    render(<CompassImportDialog compassFile={selfTieFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-apply-corrections'));

    // 5 raw - 1 excluded - 2 skipped = 2.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('2');
    expect(screen.getByTestId('compass-shot-count-footnote')).toHaveTextContent(
      '1 excluded, 2 skipped: stations tied to themselves'
    );
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('2');
  });

  it('adds the excluded skipped counter once include-excluded is checked', async () => {
    render(<CompassImportDialog compassFile={selfTieFile} filePath="/tmp/cave.dat" />);

    await userEvent.click(screen.getByTestId('compass-include-excluded'));

    // 5 raw - 0 excluded (included) - (1 + 1) skipped = 3.
    expect(screen.getByTestId('compass-shot-count')).toHaveTextContent('3');
    const footnote = screen.getByTestId('compass-shot-count-footnote');
    expect(footnote).toHaveTextContent('2 skipped: stations tied to themselves');
    expect(footnote).not.toHaveTextContent('excluded');
    expect(screen.getByTestId('compass-survey-shots-0')).toHaveTextContent('3');
  });
});
