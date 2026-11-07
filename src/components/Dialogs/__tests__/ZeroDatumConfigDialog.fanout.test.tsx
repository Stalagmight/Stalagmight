/**
 * One row, one shift - however many cells the editor writes for it (#24).
 *
 * `ShotTableEditor` calls `onShotChange` once per CELL: an averaged azimuth is
 * two calls, a pasted row about five, and they all land in ONE React event. The
 * dialog owes the station pick a shift when a row JOINS the payload and another
 * when it leaves, and while that transition was judged against the rows of the
 * render those calls shared, every call in the event saw the same blank row and
 * claimed the shift for itself: pasting one row above station 9 left the GPS
 * anchor on station 11, and clearing a row back to blank never gave the station
 * back. The rows and the pick now move together in one reducer, so the second
 * cell of an edit sees what the first one did.
 *
 * The shot table is stubbed here on purpose. The subject is the HOST's contract
 * with it - the fan-out, in one tick, at whatever width - and firing that
 * through a real grid can only reach the two-call shape (the sibling suite does
 * exactly that, end to end). `.tsx` so vitest runs it under jsdom.
 */
import { describe, it, expect, vi, beforeAll, beforeEach } from 'vitest';
import { act, render, screen, fireEvent } from '@testing-library/react';

// Real i18n: the station options ("Station N") picked below render from en.json.
import '../../../i18n';

vi.mock('maplibre-gl', () => ({ default: {} }));
vi.mock('../SectionPreview', () => ({ default: () => null }));
vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());
vi.mock('../../common/SectionMetadataForm', () => ({ SectionMetadataForm: () => null }));

const captured = vi.hoisted(() => ({ props: null as unknown }));
vi.mock('../../common/ShotTableEditor', () => ({
  ShotTableEditor: (props: unknown) => {
    captured.props = props;
    return null;
  },
}));

import ZeroDatumConfigDialog, { type ZeroDatumConfigResult } from '../ZeroDatumConfigDialog';
import { buildZeroDatumSectionConfig } from '../../../utils/importSectionConfig';
import type { EditableShot, ShotTableEditorProps } from '../../common/ShotTableEditor';
import type {
  DmpFile,
  DmpSection,
  ImportPlacement,
} from '../../../types/commands';
import type { DmpSelectionConfig } from '../DmpSelectionDialog';
import { makeDmpSection, makeDmpShots } from '../../../test/factories/dmp';

const SHOT_COUNT = 10;
const PICK = 5;

function dmpFile(): DmpFile {
  const section: DmpSection = makeDmpSection({ shots: makeDmpShots(SHOT_COUNT) });
  return { version: 1, sections: [section] } as unknown as DmpFile;
}

const selectionConfig = {
  selectedIndices: new Set([0]),
  defaultSurveyor: 'S',
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

/** The props the dialog last handed the shot table. */
function editor(): ShotTableEditorProps {
  if (captured.props === null) throw new Error('ShotTableEditor was never rendered');
  return captured.props as ShotTableEditorProps;
}

function pickStation(index: number) {
  fireEvent.click(screen.getByRole('combobox'));
  fireEvent.click(screen.getByText(`Station ${index}`));
}

/** The cells the editor writes for one filled row, in editor order. A complete
 *  row: anything less is refused by the completeness gate before Confirm. */
const FILLED: Array<[keyof EditableShot, number]> = [
  ['length', 12],
  ['azimuth_in', 90],
  ['azimuth_out', 90],
  ['depth_in', -1],
  ['depth_out', -2],
];
/** The same cells emptied - how the grid reports a cleared required cell. */
const CLEARED: Array<[keyof EditableShot, number]> = FILLED.map(([f]) => [f, NaN]);

/** Fire a whole row's worth of cell callbacks in ONE tick, as the editor does. */
function writeRows(rows: number[], cells: Array<[keyof EditableShot, number]>) {
  act(() => {
    for (const row of rows) {
      for (const [field, value] of cells) editor().onShotChange(row, field, value);
    }
  });
}

/** The `station_index` the backend would receive. */
function confirmedStationIndex(): number {
  fireEvent.change(screen.getByTestId('zero-datum-gps-input'), {
    target: { value: '20.2, -87.4' },
  });
  fireEvent.click(screen.getByTestId('zero-datum-config-confirm'));
  expect(onConfirm).toHaveBeenCalledTimes(1);
  const { placement } = buildZeroDatumSectionConfig(onConfirm.mock.calls[0]![0]);
  expect(placement.type).toBe('zero_datum');
  return (placement as Extract<ImportPlacement, { type: 'zero_datum' }>).station_index;
}

describe('ZeroDatumConfigDialog per-cell fan-out', () => {
  beforeAll(() => {
    // jsdom does not implement scrollIntoView; the Select calls it on open.
    Element.prototype.scrollIntoView = vi.fn();
  });

  beforeEach(() => {
    vi.clearAllMocks();
    captured.props = null;
  });

  it('shifts the pick once for a row filled five cells at a time', () => {
    renderDialog();
    pickStation(PICK);
    act(() => editor().onInsertShots?.(0, 1, 'before'));
    writeRows([0], FILLED);
    // One row joined the payload upstream of the pick, so the pick's physical
    // station is now numbered one higher. Five cells used to mean five shifts.
    expect(confirmedStationIndex()).toBe(PICK + 1);
  });

  it('gives the station back when the row is cleared in one event', () => {
    renderDialog();
    pickStation(PICK);
    act(() => editor().onInsertShots?.(0, 1, 'before'));
    writeRows([0], FILLED);
    writeRows([0], CLEARED);
    // In and back out: the row is absent from the payload again, so the pick is
    // where it started. Judged per cell, no single call ever saw the row become
    // blank and the shift was never given back.
    expect(confirmedStationIndex()).toBe(PICK);
  });

  it('shifts by the number of ROWS a multi-row paste fills, not the number of cells', () => {
    renderDialog();
    pickStation(PICK);
    act(() => editor().onInsertShots?.(0, 2, 'before'));
    writeRows([0, 1], FILLED);
    expect(confirmedStationIndex()).toBe(PICK + 2);
  });

  it('leaves the pick alone when the filled rows land past it', () => {
    renderDialog();
    pickStation(PICK);
    act(() => editor().onInsertShots?.(7, 2, 'after'));
    writeRows([8, 9], FILLED);
    expect(confirmedStationIndex()).toBe(PICK);
  });

  it('counts a row appended past the end without moving the pick', () => {
    renderDialog();
    pickStation(SHOT_COUNT);
    act(() => editor().onAddShot());
    writeRows([SHOT_COUNT], FILLED);
    // The old far end keeps its number; the new one is beyond it.
    expect(editor().shots).toHaveLength(SHOT_COUNT + 1);
    expect(confirmedStationIndex()).toBe(SHOT_COUNT);
  });
});
