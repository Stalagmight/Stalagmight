/**
 * The zero-datum GPS anchor survives a shot-table edit (#24).
 *
 * `stationIndex` is one piece of state written by two pickers and read by the
 * backend config builder, and every shot-table edit renumbers the stations
 * underneath it. Whichever end owns the translation, ONE of the two orderings
 * is wrong unless the pick is re-anchored as the edit happens:
 *
 *   - translating at build time (what this dialog used to do, via
 *     `adjustStationForDeletedShots`) is correct for pick-then-delete and wrong
 *     for delete-then-pick, because the dropdown value is already post-edit;
 *   - a bare clamp at build time simply inverts that - correct for
 *     delete-then-pick, wrong for pick-then-delete.
 *
 * So both orderings are tested here, at the dialog level, against the index the
 * backend actually receives. A one-sided fix fails one of them. There is no
 * warning to catch it downstream: `set_zero_datum` only rejects an out-of-range
 * index, so an anchor one shot down the passage imports silently.
 *
 * `.tsx` so vitest runs it under jsdom.
 *
 * The shot table is a virtualized SpreadsheetGrid, and jsdom gives the
 * virtualizer no layout to measure - so without this mock it renders zero rows
 * and every row-level query below fails. Same shim the grid's own component
 * tests use.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { useState } from 'react';

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

// SectionPreview draws on a MapLibre canvas, which jsdom cannot run. The pick
// it feeds is exercised through the dropdown instead.
vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));

// The shot table reads validation settings and decimal precision off the store.
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: null, config: null }),
}));

// A faithful-enough stand-in for DialogWrapper's own dirty guard (see
// DialogWrapper.tsx's `requestClose`/`showConfirm`), so the tests below
// exercise the real wiring - Cancel calling `requestClose`, not `onCancel`
// directly - rather than asserting against a mock that cannot fail.
export const mockDialogWrapperSpy = vi.fn();
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({
    children,
    isDirty,
    onClose,
    ...props
  }: {
    children: React.ReactNode | ((p: { requestClose: () => void }) => React.ReactNode);
    isDirty?: boolean;
    onClose: () => void;
    [k: string]: unknown;
  }) => {
    mockDialogWrapperSpy({ isDirty, onClose, ...props });
    const [showConfirm, setShowConfirm] = useState(false);
    const requestClose = () => {
      if (isDirty) setShowConfirm(true);
      else onClose();
    };
    return (
      <div>
        {typeof children === 'function' ? children({ requestClose }) : children}
        {showConfirm && (
          <div>
            <button
              type="button"
              data-testid="dirty-dialog-keep-editing"
              onClick={() => setShowConfirm(false)}
            >
              Keep editing
            </button>
            <button
              type="button"
              data-testid="dirty-dialog-discard"
              onClick={() => {
                setShowConfirm(false);
                onClose();
              }}
            >
              Discard
            </button>
          </div>
        )}
      </div>
    );
  },
}));

// Date picker and colour swatches are irrelevant here and pull in more DOM.
vi.mock('../../common/SectionMetadataForm', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../common/SectionMetadataForm')>()),
  SectionMetadataForm: () => null,
}));

import ZeroDatumConfigDialog, { type ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { buildZeroDatumSectionConfig } from '../../../utils/importSectionConfig';
import type {
  DmpFile,
  DmpSection,
  DmpShot,
  ImportPlacement,
  ImportSectionConfig,
} from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';

/** Ten importable shots, so the dialog offers stations 0..10. */
const SHOT_COUNT = 10;
const PICK = 5;

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
    name: 'Anchor',
    direction: 'in',
    date: '2026-01-01',
    shots: Array.from({ length: SHOT_COUNT }, dmpShot),
    content_hash: 'hash-anchor',
  };
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyors: [{ name: 'S' }],
  defaultExplorers: [],
  defaultDate: '2026-01-01',
} as unknown as DmpSelectionConfig;

const onConfirm = vi.fn<(result: ZeroDatumConfigResult) => void>();

function renderDialog() {
  render(
    <ZeroDatumConfigDialog
      dmpFile={dmpFile()}
      selectionConfig={selectionConfig}
      sectionIndex={0}
      filePath="/tmp/anchor.dmp"
      onCancel={vi.fn()}
      onBack={vi.fn()}
      onConfirm={onConfirm}
    />,
  );
}

/** Open the station dropdown and choose `Station ${index}`. */
function pickStation(index: number) {
  fireEvent.click(screen.getByRole('combobox'));
  fireEvent.click(screen.getByText(`Station ${index}`));
}

/** Materialize the trailing blank row, which is how a shot gets added.
 *  Fills a COMPLETE shot: Confirm refuses a row that holds something but is not
 *  a usable measurement, so a length-only row would block the dialog rather than
 *  exercise what these cases are about. */
function appendShot() {
  const fill = (colKey: string, value: string) => {
    fireEvent.doubleClick(screen.getByTestId(`shot-editor-cell-${SHOT_COUNT}-${colKey}`));
    const editor = screen.getByTestId('shot-editor-cell-editor');
    fireEvent.change(editor, { target: { value } });
    fireEvent.keyDown(editor, { key: 'Enter' });
  };
  fill('length', '5');
  fill('_azimuth_avg', '90');
  fill('depth_in', '-1');
  fill('depth_out', '-2');
}

function deleteRow(visibleIndex: number) {
  // Delete goes through the grid's row context menu, like the data table's -
  // the per-row X column is gone.
  fireEvent.contextMenu(screen.getByTestId(`shot-editor-cell-${visibleIndex}-length`));
  fireEvent.click(screen.getByTestId('shot-editor-context-delete-rows'));
}

/** Select a whole run by its row gutters, then delete it in one gesture. */
function deleteRowRange(from: number, to: number) {
  const gutters = document.querySelectorAll('.sg-gutter');
  fireEvent.mouseDown(gutters[from]!);
  fireEvent.mouseDown(gutters[to]!, { shiftKey: true });
  fireEvent.mouseUp(gutters[to]!, { shiftKey: true });
  fireEvent.contextMenu(screen.getByTestId(`shot-editor-cell-${to}-length`));
  fireEvent.click(screen.getByTestId('shot-editor-context-delete-rows'));
}

function insertBeforeRow(visibleIndex: number) {
  // Insert now goes through the grid's own context menu ("above" = before).
  fireEvent.contextMenu(screen.getByTestId(`shot-editor-cell-${visibleIndex}-length`));
  fireEvent.click(screen.getByTestId('shot-editor-context-insert-above'));
}

/** Confirm the dialog and return the config the backend would receive. */
function confirmedConfig(): ImportSectionConfig {
  fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
    target: { value: '20.2, -87.4' },
  });
  fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));

  expect(onConfirm).toHaveBeenCalledTimes(1);
  return buildZeroDatumSectionConfig(onConfirm.mock.calls[0]![0]);
}

/** The `station_index` the backend would receive. */
function confirmedStationIndex(): number {
  const placement = confirmedConfig().placement;
  expect(placement.type).toBe('zero_datum');
  return (placement as Extract<ImportPlacement, { type: 'zero_datum' }>).station_index;
}

describe('ZeroDatumConfigDialog station pick', () => {
  beforeAll(() => {
    // jsdom does not implement scrollIntoView; the Select calls it on open.
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('offers one station per surviving shot, plus the far end', () => {
    renderDialog();
    fireEvent.click(screen.getByRole('combobox'));
    expect(screen.getAllByRole('option')).toHaveLength(SHOT_COUNT + 1);
  });

  it('keeps the pick on its physical station when a leading shot is deleted after it', () => {
    // Pick-then-delete. Station 5 sits 5 shots along; removing the first shot
    // renumbers it to 4. A build-time clamp alone leaves it at 5, anchoring the
    // GPS fix one shot further down the passage than the user chose.
    renderDialog();
    pickStation(PICK);
    deleteRow(0);
    expect(confirmedStationIndex()).toBe(PICK - 1);
  });

  it('sends a pick made AFTER the delete verbatim', () => {
    // Delete-then-pick. The dropdown is rebuilt from the survivors, so Station 5
    // already means station 5 of the 9 shots being sent. Translating again here
    // is what moved the anchor.
    renderDialog();
    deleteRow(0);
    pickStation(PICK);
    expect(confirmedStationIndex()).toBe(PICK);
  });

  /**
   * An INSERTED row is blank, and a blank row is dropped from the payload, so it
   * renumbers nothing yet - the pick must not move. This used to assert a shift
   * of +1, from when every non-deleted row counted; keeping that would have put
   * the picker one ahead of the section actually sent and anchored the datum to
   * the wrong station.
   */
  it('leaves the pick alone when a BLANK shot is inserted before it', () => {
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    expect(confirmedStationIndex()).toBe(PICK);
  });

  it('shifts the pick once that inserted row is given data', () => {
    // The shift is owed at the moment the row joins the payload, not before.
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    // Fill it COMPLETELY - a half-filled row is blocked by the completeness
    // gate, which is its own correct behaviour and would mask this assertion.
    for (const [col, value] of [
      ['length', '12'],
      ['_azimuth_avg', '90'],
      ['depth_in', '-1'],
      ['depth_out', '-2'],
    ] as const) {
      fireEvent.doubleClick(screen.getByTestId(`shot-editor-cell-0-${col}`));
      const editor = screen.getByTestId('shot-editor-cell-editor');
      fireEvent.change(editor, { target: { value } });
      fireEvent.keyDown(editor, { key: 'Enter' });
    }
    expect(confirmedStationIndex()).toBe(PICK + 1);
  });

  it('leaves the pick alone when the edit lands past it', () => {
    // Station 5 is upstream of row 7, so nothing it addresses is renumbered.
    renderDialog();
    pickStation(PICK);
    deleteRow(7);
    expect(confirmedStationIndex()).toBe(PICK);
  });

  it('materializes the trailing blank row from ONE averaged azimuth, adding one station', () => {
    // The averaged azimuth column fans out to both legs, so this is TWO
    // `onShotChange` calls in one React event - on a row that does not exist in
    // the render either of them sees, because the same event materializes it.
    // The value must land (it is the primary way a shot is added) and the row
    // must join the payload once, not once per leg.
    renderDialog();
    pickStation(SHOT_COUNT);
    fireEvent.doubleClick(screen.getByTestId(`shot-editor-cell-${SHOT_COUNT}-_azimuth_avg`));
    const editor = screen.getByTestId('shot-editor-cell-editor');
    fireEvent.change(editor, { target: { value: '90' } });
    fireEvent.keyDown(editor, { key: 'Enter' });

    expect(screen.getByTestId(`shot-editor-cell-${SHOT_COUNT}-_azimuth_avg`).textContent).toContain(
      '90',
    );
    // The pick sat at the old far end and stays there; the new far end is one
    // beyond it. Two stations would mean the row was counted twice.
    expect(screen.getByRole('combobox').textContent).toContain(`Station ${SHOT_COUNT}`);
    fireEvent.click(screen.getByRole('combobox'));
    expect(screen.getAllByRole('option')).toHaveLength(SHOT_COUNT + 2);
  });

  it('keeps a pick at the far end pinned to the far end when a shot is appended', () => {
    renderDialog();
    pickStation(SHOT_COUNT);
    // A shot is added by typing into the trailing blank row, which is the
    // convention across the app's grids - there is no Add button any more.
    appendShot();
    // Appending extends the section; the old last station keeps its number and
    // the new far end is one beyond it.
    expect(confirmedStationIndex()).toBe(SHOT_COUNT);
  });

  it('does not drop a just-committed field edit when the next gesture is a delete', () => {
    // The mutators derive the new shot array and the re-anchored pick from ONE
    // snapshot, so the two can never disagree - but that means they no longer
    // use a functional `setShots`. Pressing a row's delete button blurs whatever
    // input was focused, so a blur-commit lands immediately before the delete;
    // pin that the commit survives it.
    renderDialog();
    // The grid has one editor overlaying the active cell rather than an input
    // per cell: open it on row 2's length, type, and let the blur commit it.
    fireEvent.doubleClick(screen.getByTestId('shot-editor-cell-2-length'));
    const editor = screen.getByTestId('shot-editor-cell-editor');
    fireEvent.change(editor, { target: { value: '42' } });
    fireEvent.blur(editor);
    deleteRow(0);

    const shots = confirmedConfig().shots;
    expect(shots).toHaveLength(SHOT_COUNT - 1);
    expect(shots[1]?.length).toBe(42);
  });

  it('follows the pick down as the section is emptied out from under it', () => {
    renderDialog();
    pickStation(PICK);
    // Delete the five shots the pick sits beyond, one at a time - the visible
    // row 0 is a different physical shot on each pass.
    for (let i = 0; i < PICK; i++) deleteRow(0);
    expect(confirmedStationIndex()).toBe(0);
  });
});

/**
 * The pick tracks the EMITTED station order, so every transition that moves a row
 * in or out of the payload has to move it - and only those.
 *
 * The first version handled one: a blank row gaining data. Clearing it back to
 * blank left the pick shifted while the row had left the payload, and deleting a
 * range subtracted uncounted rows too. Both anchored the GPS datum to the wrong
 * station on Confirm.
 */
describe('ZeroDatumConfigDialog station pick across blank transitions', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  /** Commit `value` (empty clears) into one cell of row 0. */
  const editRow0 = (col: string, value: string) => {
    fireEvent.doubleClick(screen.getByTestId(`shot-editor-cell-0-${col}`));
    const editor = screen.getByTestId('shot-editor-cell-editor');
    fireEvent.change(editor, { target: { value } });
    fireEvent.keyDown(editor, { key: 'Enter' });
  };

  /** The four required cells, in the given order. Fill ALL of them - a
   *  half-filled row is blocked by the completeness gate. */
  const REQUIRED = ['length', '_azimuth_avg', 'depth_in', 'depth_out'] as const;
  const VALUES: Record<string, string> = {
    length: '12',
    _azimuth_avg: '90',
    depth_in: '-1',
    depth_out: '-2',
  };
  const fillRow0 = (order: readonly string[] = REQUIRED) => {
    for (const col of order) editRow0(col, VALUES[col] ?? '');
  };
  const clearRow0 = (order: readonly string[] = REQUIRED) => {
    for (const col of order) editRow0(col, '');
  };

  it('returns the pick when a filled row is cleared back to blank', () => {
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    fillRow0();
    clearRow0();
    // In and back out: the row is absent from the payload again, so the pick is
    // where it started. Leaving it shifted anchored a station downstream.
    expect(confirmedStationIndex()).toBe(PICK);
  });

  it('does not move the pick when a BLANK row is deleted before it', () => {
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    deleteRow(0);
    expect(confirmedStationIndex()).toBe(PICK);
  });

  /**
   * The transition is owed ONCE per row, not once per cell.
   *
   * The averaged azimuth column fans out to both legs, so committing it is two
   * `onShotChange` calls in ONE React event - and it is the cell that takes this
   * row into the payload. Judged against the render's rows, both calls saw the
   * same blank row and each claimed the shift, walking the pick two stations for
   * one row (a pasted row, five cells, walked it further still).
   */
  it('shifts the pick ONCE when the cell that fills the row writes two fields', () => {
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    fillRow0(['_azimuth_avg', 'length', 'depth_in', 'depth_out']);
    expect(confirmedStationIndex()).toBe(PICK + 1);
  });

  /** The mirror: the row leaves the payload on the SECOND of that column's two
   *  calls, and the first one had already reported "still counted". */
  it('returns the pick when the cell that empties the row writes two fields', () => {
    renderDialog();
    pickStation(PICK);
    insertBeforeRow(0);
    fillRow0();
    clearRow0(['length', 'depth_in', 'depth_out', '_azimuth_avg']);
    expect(confirmedStationIndex()).toBe(PICK);
  });
});

/**
 * A pick that sat inside a deleted run has nowhere to shift TO: the stations it
 * addressed are gone. It collapses to the run's start, which is the boundary the
 * deleted span closed up to.
 *
 * Applying the whole run's delta instead subtracted more stations than preceded
 * the pick - deleting stations 2..7 with 5 picked gave 5 - 6, clamped to 0, so
 * the GPS anchor jumped to the head of the section.
 */
describe('ZeroDatumConfigDialog pick inside a deleted run', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('collapses a pick inside the run to the run start, not to zero', () => {
    renderDialog();
    pickStation(5);
    deleteRowRange(1, 6); // rows 1..6 -> stations 2..7
    expect(confirmedStationIndex()).toBe(1);
  });

  it('still shifts a pick that sits after the run by the whole run', () => {
    renderDialog();
    pickStation(8);
    deleteRowRange(1, 3); // three stations removed upstream of the pick
    expect(confirmedStationIndex()).toBe(5);
  });

  it('leaves a pick before the run alone', () => {
    renderDialog();
    pickStation(1);
    deleteRowRange(4, 6);
    expect(confirmedStationIndex()).toBe(1);
  });
});

/**
 * The dirty guard (#F9): DialogWrapper's own confirm-on-close only fires when
 * `isDirty` is true, so a stub that always passes `true` (or always `false`)
 * would either nag on an untouched dialog or silently drop a filled-in one -
 * every case below pairs "still clean" with "now dirty" so a constant answer
 * fails at least one.
 */
describe('ZeroDatumConfigDialog dirty guard', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  function lastIsDirty(): boolean {
    const calls = mockDialogWrapperSpy.mock.calls;
    const last = calls[calls.length - 1]?.[0] as { isDirty?: boolean } | undefined;
    return Boolean(last?.isDirty);
  }

  it('starts clean', () => {
    renderDialog();
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty when GPS text is entered', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '2' } });
    expect(lastIsDirty()).toBe(true);
  });

  it('returns to clean when the GPS text is cleared back out', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '2' } });
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '' } });
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty when the section is renamed', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('section-name-input'), { target: { value: 'New Name' } });
    expect(lastIsDirty()).toBe(true);
  });

  it('goes dirty on a shot-table edit', () => {
    renderDialog();
    deleteRow(0);
    expect(lastIsDirty()).toBe(true);
  });

  it('stays clean when the pick matches the initial (station 0, unchanged)', () => {
    // `stationIndex` starts at 0; re-picking the same station is not a change
    // to the confirm payload, so it must not arm the guard. Not routed
    // through the shared `pickStation` helper: with 0 already selected, the
    // dropdown's own closed-state label and the freshly opened option both
    // read "Station 0", so the option is picked by role instead of text.
    renderDialog();
    fireEvent.click(screen.getByRole('combobox'));
    fireEvent.click(screen.getByRole('option', { name: 'Station 0' }));
    expect(lastIsDirty()).toBe(false);
  });

  it('goes dirty on a changed pick - stationIndex IS sent in the confirm payload', () => {
    renderDialog();
    pickStation(PICK);
    expect(lastIsDirty()).toBe(true);
  });
});

/**
 * Cancel and Back must route through the SAME guard `isDirty` feeds - a dirty
 * flag that only ever reaches a spied-on prop and never gates an exit is not
 * a guard. Every "confirm pending" case below is paired with the accepted
 * discard, so a stub that skips straight to the callback (or one that never
 * calls it at all) fails one half of the pair.
 */
describe('ZeroDatumConfigDialog Cancel/Back dirty guard', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  function makeDirty() {
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '2' } });
  }

  it('Cancel calls onCancel immediately when clean', () => {
    const onCancel = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={onCancel}
        onBack={vi.fn()}
        onConfirm={onConfirm}
      />,
    );
    fireEvent.click(screen.getByTestId('zero-datum-config-cancel'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('Cancel does NOT call onCancel while dirty until the discard confirm is accepted', () => {
    const onCancel = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={onCancel}
        onBack={vi.fn()}
        onConfirm={onConfirm}
      />,
    );
    makeDirty();
    fireEvent.click(screen.getByTestId('zero-datum-config-cancel'));
    expect(onCancel).not.toHaveBeenCalled();
    expect(screen.getByTestId('dirty-dialog-discard')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('dirty-dialog-discard'));
    expect(onCancel).toHaveBeenCalledTimes(1);
  });

  it('Cancel is cancellable: "keep editing" leaves onCancel uncalled', () => {
    const onCancel = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={onCancel}
        onBack={vi.fn()}
        onConfirm={onConfirm}
      />,
    );
    makeDirty();
    fireEvent.click(screen.getByTestId('zero-datum-config-cancel'));
    fireEvent.click(screen.getByTestId('dirty-dialog-keep-editing'));
    expect(onCancel).not.toHaveBeenCalled();
  });

  it('Back calls onBack immediately when clean', () => {
    const onBack = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={vi.fn()}
        onBack={onBack}
        onConfirm={onConfirm}
      />,
    );
    fireEvent.click(screen.getByTestId('zero-datum-config-back'));
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it('Back does NOT call onBack while dirty until its own discard confirm is accepted', () => {
    // NOTE for whoever integrates Package A's state-preserving Back: this is
    // the "small isolated hunk" the coordinator asked to flag - if Back no
    // longer discards anything, this confirm (and this test) can go.
    const onBack = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={vi.fn()}
        onBack={onBack}
        onConfirm={onConfirm}
      />,
    );
    makeDirty();
    fireEvent.click(screen.getByTestId('zero-datum-config-back'));
    expect(onBack).not.toHaveBeenCalled();
    expect(screen.getByTestId('zero-datum-config-back-confirm-discard')).toBeInTheDocument();
    fireEvent.click(screen.getByTestId('zero-datum-config-back-confirm-discard'));
    expect(onBack).toHaveBeenCalledTimes(1);
  });

  it('Back is cancellable: dismissing its confirm leaves onBack uncalled', () => {
    const onBack = vi.fn();
    render(
      <ZeroDatumConfigDialog
        dmpFile={dmpFile()}
        selectionConfig={selectionConfig}
        sectionIndex={0}
        filePath="/tmp/anchor.dmp"
        onCancel={vi.fn()}
        onBack={onBack}
        onConfirm={onConfirm}
      />,
    );
    makeDirty();
    fireEvent.click(screen.getByTestId('zero-datum-config-back'));
    fireEvent.click(screen.getByTestId('zero-datum-config-back-confirm-cancel'));
    expect(onBack).not.toHaveBeenCalled();
  });
});

describe('ZeroDatumConfigDialog GPS hints', () => {
  beforeAll(() => {
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('shows the empty-state hint before anything is typed', () => {
    renderDialog();
    expect(screen.getByTestId('zero-datum-gps-empty-hint')).toBeInTheDocument();
  });

  it('hides the empty-state hint once GPS text is entered', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '48.8584, 2.2945' } });
    expect(screen.queryByTestId('zero-datum-gps-empty-hint')).not.toBeInTheDocument();
  });

  it('warns on a plausibly-transposed pair', () => {
    renderDialog();
    // True fix would be lat 12, lon 77 (Bengaluru); entered swapped.
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '77, 12' } });
    expect(screen.getByTestId('zero-datum-gps-transposed-warning')).toBeInTheDocument();
  });

  it('does not warn on a plausible, non-extreme pair', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '48.8584, 2.2945' } });
    expect(screen.queryByTestId('zero-datum-gps-transposed-warning')).not.toBeInTheDocument();
  });

  it('does not block Confirm - the warning is advisory only', () => {
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '77, 12' } });
    expect(screen.getByTestId('zero-datum-gps-transposed-warning')).toBeInTheDocument();
    expect(screen.getByTestId('zero-datum-config-confirm')).not.toBeDisabled();
  });

  it('shows the swap hint ALONGSIDE the range error when the first field is an impossible latitude', () => {
    // Regression: 120 fails `parseDD`'s range check before any GpsCoordinates
    // exist, so the coords-based check alone never ran and the hint was
    // silently absent next to the error.
    renderDialog();
    fireEvent.change(screen.getByTestId('zero-datum-gps-input'), { target: { value: '120, 45' } });
    expect(screen.getByTestId('zero-datum-gps-error')).toBeInTheDocument();
    expect(screen.getByTestId('zero-datum-gps-transposed-warning')).toBeInTheDocument();
  });
});
