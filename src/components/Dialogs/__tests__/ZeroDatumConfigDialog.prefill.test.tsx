/**
 * #394 F10: stepping BACK out of the import wizard returns to this dialog, and
 * it must come back holding what the user typed.
 *
 * The wizard withdraws the sections it imported on the way back - the forward
 * pass re-imports from scratch, so keeping them would duplicate - which means
 * the anchor really is configured again from here. Without the carry the user
 * would lose the GPS, the name and every metadata field as well, turning
 * "adjust one field and continue" into "start over"; with an empty GPS box the
 * Confirm button is not even enabled.
 *
 * The oracle is deliberately BOTH ends: the values on screen, and the payload a
 * confirm with no further typing produces. On-screen alone would pass on a
 * prefill that never reached the state the payload is built from.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';

// jsdom gives the shot grid's virtualizer no layout to measure, so without this
// it renders zero rows. Same shim the grid's own component tests use.
vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({
        index,
        key: index,
        start: 33 + index * 33,
        size: 33,
      })),
    getTotalSize: () => count * 33,
    scrollToIndex: () => {},
  }),
}));
import { render, screen, fireEvent } from '@testing-library/react';

// Partial mock: `src/i18n/index.ts` (pulled in by the number formatters) needs
// the real `initReactI18next`.
vi.mock('react-i18next', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react-i18next')>()),
  useTranslation: () => ({ t: (k: string) => k }),
}));

// SectionPreview draws on a MapLibre canvas, which jsdom cannot run.
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));

// The shot table reads validation settings and decimal precision off the store.
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

// The body is a RENDER PROP (#526), so a mock that drops `children` into JSX
// renders nothing at all - React logs "Functions are not valid as a React child"
// and every query below fails. Same shape as the sibling suites, and it takes
// the `isDirty` spy while it is here: the clean/dirty contract had never been
// exercised against a PREFILLED dialog, which is exactly what this merge creates.
const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({
    children,
    isDirty,
    ...props
  }: {
    children: React.ReactNode | ((p: { requestClose: () => void }) => React.ReactNode);
    isDirty?: boolean;
    [k: string]: unknown;
  }) => {
    mockDialogWrapperSpy({ isDirty, ...props });
    return <div>{typeof children === 'function' ? children({ requestClose: () => {} }) : children}</div>;
  },
}));

/** What the dialog last told DialogWrapper about its dirtiness. */
function lastIsDirty(): boolean {
  const calls = mockDialogWrapperSpy.mock.calls;
  const last = calls[calls.length - 1]?.[0] as { isDirty?: boolean } | undefined;
  return Boolean(last?.isDirty);
}

import ZeroDatumConfigDialog, { type ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { convertToEditableShots } from '../../../hooks/useImportState';
import type { DmpFile, DmpSection, DmpShot } from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

function dmpShot(): DmpShot {
  return {
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
    temperature: 0,
    hour: 0,
    minute: 0,
    second: 0,
    marker_index: 0,
  };
}

function dmpFile(): DmpFile {
  const section: DmpSection = {
    version: 1,
    name: 'FileName',
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: 3 }, dmpShot),
    content_hash: 'hash-anchor',
  };
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

/** Defaults that differ from every carried value below, so a prefill that
 *  silently fell back to them is visible rather than coincidentally equal. */
const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyors: [{ name: 'DefaultSurveyor' }],
  defaultExplorers: [{ name: 'DefaultExplorer' }],
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

/** What the user entered before they pressed Back. */
const carried = (): ZeroDatumConfigResult => ({
  sectionIndex: 0,
  stationIndex: 0,
  gps: { latitude: 20.25, longitude: -87.5, depth_reference: 0 },
  metadata: {
    name: 'Typed Name',
    surveyors: [{ name: 'Typed Surveyor' }],
    explorers: [{ name: 'Typed Explorer', email: 'te@example.org' }],
    date: '2025-12-24',
    color: '#123456',
    direction: 'out',
    quality: 1,
    reverse: true,
  },
  shots: [],
});

const onConfirm = vi.fn<(result: ZeroDatumConfigResult) => void>();

function renderDialog(initialConfig?: ZeroDatumConfigResult, onBack: () => void = vi.fn()) {
  render(
    <ZeroDatumConfigDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      sectionIndex={0}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={onBack}
      onConfirm={onConfirm}
      initialConfig={initialConfig}
    />,
  );
}

/** The carried rows, with one value edited so a restore is distinguishable. */
function editedCarriedShots() {
  const shots = convertToEditableShots(dmpFile().sections[0]!.shots);
  shots[1] = { ...shots[1]!, length: 42 };
  return shots;
}

describe('ZeroDatumConfigDialog prefill after Back (#394 F10)', () => {
  beforeAll(() => {
    // jsdom does not implement scrollIntoView; the Select calls it on open.
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  /**
   * Injection: drop the `initialConfig?...` seeds from the `useState` calls and
   * the GPS box is empty, the name reads "FileName" and the surveyor
   * "DefaultSurveyor".
   */
  it('opens holding the GPS, name and surveyor the user entered', () => {
    renderDialog(carried());

    // Decimal degrees, whatever was originally typed: `gps` is the parsed pair
    // and the source text cannot be re-derived from it.
    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('20.25, -87.5');
    // Parsed, not merely echoed - so Confirm is live on arrival.
    expect(screen.getByTestId('zero-datum-gps-parsed')).toBeInTheDocument();
    expect(screen.queryByTestId('zero-datum-gps-error')).not.toBeInTheDocument();

    expect(screen.getByTestId('section-name-input')).toHaveValue('Typed Name');
    expect(screen.getByTestId('section-surveyors-pill-0-label')).toHaveTextContent('Typed Surveyor');
    expect(screen.getByTestId('section-explorers-pill-0-label')).toHaveTextContent('Typed Explorer');
    expect(screen.getByTestId('section-reverse-checkbox')).toBeChecked();
  });

  /**
   * The prefill has to reach the STATE the payload is built from, not just the
   * inputs' displayed values.
   *
   * Injection: seed only the GPS and this confirms with the file/selection
   * defaults for colour, direction and date.
   */
  it('confirms with the carried values when nothing is retyped', () => {
    renderDialog(carried());

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const result = onConfirm.mock.calls[0]![0];
    expect(result.gps).toMatchObject({ latitude: 20.25, longitude: -87.5 });
    expect(result.metadata).toMatchObject({
      name: 'Typed Name',
      surveyors: [{ name: 'Typed Surveyor' }],
      explorers: [{ name: 'Typed Explorer', email: 'te@example.org' }],
      date: '2025-12-24',
      color: '#123456',
      direction: 'out',
      reverse: true,
    });
  });

  /**
   * A first visit has no previous answers, so nothing above may leak into it.
   *
   * Injection: seed unconditionally (drop the `initialConfig ?` guards) and this
   * renders the carried values for a dialog that was never confirmed.
   */
  it('falls back to the file and selection defaults on a first visit', () => {
    renderDialog(undefined);

    expect(screen.getByTestId('zero-datum-gps-input')).toHaveValue('');
    expect(screen.getByTestId('section-name-input')).toHaveValue('FileName');
    expect(screen.getByTestId('section-surveyors-pill-0-label')).toHaveTextContent('DefaultSurveyor');
    expect(screen.getByTestId('section-explorers-pill-0-label')).toHaveTextContent('DefaultExplorer');
    expect(screen.getByTestId('section-reverse-checkbox')).not.toBeChecked();
  });

  /**
   * The SHOTS and the STATION PICK come back too, and they have to come back
   * together.
   *
   * `stationIndex` is numbered over the EDITED rows, so seeding the metadata
   * while letting the reducer rebuild from the file - which is what the first
   * cut of this prefill did - re-anchors the datum at station 0 on the way
   * forward. Nothing downstream catches it: `set_zero_datum` only rejects an
   * out-of-range index, so the whole survey silently relocates by however far
   * the user's real anchor was down the passage.
   *
   * Injection: pass `section` to `initialShotEditState` instead of the carried
   * state and this confirms with `stationIndex` 0 and the file's 5m rows.
   */
  it('restores the edited rows and the station they were picked against', () => {
    const editedShots = editedCarriedShots();

    renderDialog({ ...carried(), stationIndex: 2, shots: editedShots });

    fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

    expect(onConfirm).toHaveBeenCalledTimes(1);
    const result = onConfirm.mock.calls[0]![0];
    expect(result.stationIndex).toBe(2);
    expect(result.shots[1]?.length).toBe(42);
    // The rows the user is LOOKING at are the carried ones too, not the file's.
    expect(screen.getByTestId('shot-editor-cell-1-length')).toHaveTextContent('42');
  });

  /**
   * A field the user CLEARED stays cleared. `??` per field would resurrect the
   * file/selection default underneath an emptied one, which reads to the user as
   * the dialog undoing their deletion.
   *
   * Injection: seed each role with `initialConfig?.metadata.surveyors ||
   * selectionConfig.defaultSurveyors`-style fallbacks and the emptied people
   * come back as "DefaultSurveyor" / "DefaultExplorer".
   */
  it('keeps a field the user cleared cleared', () => {
    renderDialog({ ...carried(), metadata: { ...carried().metadata, surveyors: [], explorers: [], date: null } });

    expect(screen.queryByTestId('section-surveyors-pill-0')).not.toBeInTheDocument();
    expect(screen.queryByTestId('section-explorers-pill-0')).not.toBeInTheDocument();
    expect(screen.getByTestId('section-date-input')).toHaveValue('');
  });

  /**
   * The dirty guard (#526) x the prefill (#394 F10). Every #526 term compares
   * against what the dialog OPENED with, and on a re-entry that is the CARRIED
   * value - so a reopened, untouched dialog is CLEAN. Otherwise both this
   * dialog's Back confirm and DialogWrapper's "Unsaved Changes" would fire over
   * zero edits, the exact nag #526's own tests exist to prevent.
   *
   * The clean half is paired with a dirty half in every case below, because a
   * constant-`false` regression would pass the clean half alone.
   */
  describe('the dirty guard against a prefilled dialog', () => {
    /**
     * Injection A: restore `gpsInput.trim() !== ''` -> starts dirty (the seeded
     * GPS box reads as an edit).
     * Injection B: restore `useRef(section?.name ?? '')` -> starts dirty
     * ('Typed Name' !== 'FileName').
     */
    it('a dialog reopened with initialConfig starts CLEAN and goes dirty only on further edits', () => {
      renderDialog(carried());

      expect(lastIsDirty()).toBe(false);

      fireEvent.change(screen.getByTestId('section-name-input'), {
        target: { value: 'Edited' },
      });

      expect(lastIsDirty()).toBe(true);
    });

    /**
     * The only test that pins the `initialStationIndexRef` capture, which
     * auto-merged and so has no marker to draw review to.
     *
     * Injection: `useRef(0)` instead of `useRef(stationIndex)` -> the clean half
     * fails, because the carried pick reads as a user change.
     */
    it('a reopened dialog with a carried NON-ZERO station pick still starts clean', () => {
      renderDialog({ ...carried(), stationIndex: 2, shots: editedCarriedShots() });

      expect(lastIsDirty()).toBe(false);

      fireEvent.click(screen.getByTestId('zero-datum-station-select'));
      fireEvent.click(screen.getByText('Station 1'));

      expect(lastIsDirty()).toBe(true);
    });

    /**
     * The behavioural statement of the `dirty: false` seed - the half that
     * `tsc` cannot make. Carried rows are what the dialog opened with, not an
     * edit.
     *
     * Injection: `dirty: true` in `initialShotEditState`'s prefill branch -> the
     * clean half fails.
     */
    it('a reopened dialog whose carried rows were edited is not reported as a shot edit', () => {
      renderDialog({ ...carried(), stationIndex: 2, shots: editedCarriedShots() });

      expect(lastIsDirty()).toBe(false);

      fireEvent.contextMenu(screen.getByTestId('shot-editor-cell-1-length'));
      fireEvent.click(screen.getByTestId('shot-editor-context-delete-rows'));

      expect(lastIsDirty()).toBe(true);
    });

    /**
     * The executable statement of the reconciliation's one behavioural
     * consequence: a reopened, untouched dialog leaves via Back with no prompt.
     * Since #534, that is no longer a silent discard of the carried config -
     * App.tsx relays `previousZeroDatumConfig` through 'zero-datum-selection'
     * and back on a same-section forward pass (see ZeroDatumConfigDialog.back.test.tsx)
     * - so no prompt is exactly right here: there is nothing typed in THIS
     * visit to lose.
     *
     * Injection: make `isDirty` unconditionally true and the first half fails;
     * make it unconditionally false and the second half does.
     */
    it('Back on an untouched reopened dialog leaves immediately; after an edit it confirms first', () => {
      const onBack = vi.fn();
      renderDialog(carried(), onBack);

      fireEvent.click(screen.getByTestId('zero-datum-config-back'));

      expect(onBack).toHaveBeenCalledTimes(1);
      expect(
        screen.queryByTestId('zero-datum-config-back-confirm-discard'),
      ).not.toBeInTheDocument();
    });

    it('Back after an edit on a reopened dialog confirms before discarding', () => {
      const onBack = vi.fn();
      renderDialog(carried(), onBack);

      // A name typed but not added yet is an edit too (PeopleInput's onCommitPending).
      fireEvent.change(screen.getByTestId('section-surveyors-input'), {
        target: { value: 'Someone Else' },
      });
      fireEvent.click(screen.getByTestId('zero-datum-config-back'));

      expect(onBack).not.toHaveBeenCalled();
      const discard = screen.getByTestId('zero-datum-config-back-confirm-discard');

      fireEvent.click(discard);

      expect(onBack).toHaveBeenCalledTimes(1);
    });
  });
});
