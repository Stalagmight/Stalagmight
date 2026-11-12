/**
 * The credit cell popover owns the keyboard (the data table's wiring: the
 * credit hook's columns, its popover, and the real grid under them).
 *
 * In the app (WebKitGTK, and Chromium too) the popover opened with the focus
 * still on the grid: `autoFocus` ran while the box was `visibility: hidden`
 * waiting to be placed, and neither engine focuses a hidden element. The grid
 * then took the keys - a typed letter was committed as a one-letter explorer
 * on the shot, and Escape left the popover open. jsdom focuses hidden elements,
 * so `focus` here follows the engines' rule.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';
import { useTranslation } from 'react-i18next';

vi.mock('@tanstack/react-virtual', () => ({
  useVirtualizer: ({ count }: { count: number }) => ({
    getVirtualItems: () =>
      Array.from({ length: count }, (_, index) => ({ index, key: index, start: 33 + index * 33, size: 33 })),
    getTotalSize: () => count * 33,
    scrollToIndex: vi.fn(),
  }),
}));
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(() => Promise.resolve()) }));

import '../../../i18n';
import { SpreadsheetGrid } from '../../common/SpreadsheetGrid';
import { useDataTableCredit } from '../useDataTableCredit';
import { useProjectStore } from '../../../store/projectStore';
import type { Shot } from '../../../types/project';

const SECTION = 7;

function shot(id: number, credit?: Shot['credit']): Shot {
  return {
    id,
    azimuth_in: 0,
    azimuth_out: 0,
    length: 1,
    depth_in: 0,
    depth_out: 0,
    pitch_in: null,
    pitch_out: null,
    left: null,
    right: null,
    up: null,
    down: null,
    comment: null,
    kind: 'Normal',
    marker_index: null,
    temperature: null,
    ...(credit ? { credit } : {}),
  } as Shot;
}

const rows = [shot(11, { surveyors: [{ name: 'Ann' }], explorers: [], date: null }), shot(12)];
const shotIndexAt = (rowIndex: number) => rowIndex;

/**
 * DataTable's wiring of the credit hook, around a grid of just its columns.
 * Without `cellIds` the popover cannot find its cell, as for a cell the grid
 * has virtualized away.
 */
function Table({ onEdit, cellIds }: { onEdit: (row: number, colKey: string, value: unknown) => void; cellIds: boolean }) {
  const { t } = useTranslation();
  const credit = useDataTableCredit({ t, sectionId: SECTION, rows, shotIndexAt, editingBlocked: false });
  return (
    <div onKeyDownCapture={credit.onKeyDownCapture}>
      {credit.editor}
      <SpreadsheetGrid<Shot>
        columns={credit.columns}
        rows={rows}
        rowKey={(s) => s.id}
        {...(cellIds
          ? { cellTestId: (s: Shot, _rowIndex: number, colKey: string) => `shot-cell-${SECTION}-${s.id}-${colKey}` }
          : {})}
        onEdit={onEdit}
        onBatchEdit={(edits) => edits.forEach((e) => onEdit(e.row, e.colKey, e.value))}
        height={200}
      />
    </div>
  );
}

const realFocus = HTMLElement.prototype.focus;
let editShotCredit: ReturnType<typeof vi.fn>;

beforeEach(() => {
  // WebKit and Chromium do not focus an element inside `visibility: hidden`.
  vi.spyOn(HTMLElement.prototype, 'focus').mockImplementation(function (this: HTMLElement, options?: FocusOptions) {
    for (let el: HTMLElement | null = this; el; el = el.parentElement) {
      if (el.style.visibility === 'hidden') return;
    }
    realFocus.call(this, options);
  });
  editShotCredit = vi.fn(() => Promise.resolve());
  useProjectStore.setState({ project: null, editShotCredit } as never);
});

afterEach(() => {
  vi.restoreAllMocks();
});

function renderTable({ cellIds = true } = {}) {
  const onEdit = vi.fn();
  render(<Table onEdit={onEdit} cellIds={cellIds} />);
  return { onEdit };
}

const scroller = () => document.querySelector<HTMLElement>('.sg-grid [tabindex="0"]')!;
const gridCell = (row: number, key: string) => screen.getByTestId(`grid-cell-${row}-${key}`);
const editor = () => screen.queryByTestId('credit-cell-editor');
const input = () => screen.getByTestId('credit-cell-editor-people-input') as HTMLInputElement;

/** A real click puts the keyboard on the grid (its mousedown focuses it). */
function click(row: number, key: string) {
  fireEvent.mouseDown(gridCell(row, key));
  fireEvent.mouseUp(gridCell(row, key));
}

function doubleClick(row: number, key: string) {
  click(row, key);
  click(row, key);
  fireEvent.doubleClick(gridCell(row, key));
}

describe('the credit cell popover and the keyboard', () => {
  it('typing a letter on a credit cell opens the popover with it, and never commits it as a person', () => {
    const { onEdit } = renderTable();
    click(1, 'explorers');
    expect(document.activeElement).toBe(scroller());
    fireEvent.keyDown(scroller(), { key: 'A' });

    expect(editor()).toHaveAttribute('data-field', 'explorers');
    expect(input()).toHaveValue('A');
    expect(document.activeElement).toBe(input());
    expect(onEdit).not.toHaveBeenCalled();
    expect(editShotCredit).not.toHaveBeenCalled();
    expect(screen.queryByTestId('grid-cell-editor')).toBeNull();
  });

  it('double-click opens the popover with the focus in its field, even when the grid refocuses itself after', () => {
    const { onEdit } = renderTable();
    doubleClick(0, 'surveyors');
    expect(editor()).toHaveAttribute('data-field', 'surveyors');
    expect(document.activeElement).toBe(input());

    // The grid grabs the focus back while the popover is open, as often as it
    // likes: it never keeps it, and a letter typed next goes to the field.
    act(() => scroller().focus());
    expect(document.activeElement).toBe(input());
    act(() => scroller().focus());
    expect(document.activeElement).toBe(input());
    fireEvent.change(input(), { target: { value: 'Cy' } });
    expect(input()).toHaveValue('Cy');
    expect(onEdit).not.toHaveBeenCalled();
    expect(editShotCredit).not.toHaveBeenCalled();
  });

  it('keys that still reach the grid while the popover is open never act on the grid', () => {
    const { onEdit } = renderTable();
    doubleClick(0, 'explorers');
    // Focus on the grid during a press (the guard stands down for presses),
    // so the next keys land on the grid itself.
    fireEvent.mouseDown(screen.getByTestId('credit-cell-editor-cancel'));
    act(() => scroller().focus());
    fireEvent.mouseUp(screen.getByTestId('credit-cell-editor-cancel'));
    expect(document.activeElement).toBe(scroller());

    fireEvent.keyDown(scroller(), { key: 'B' });
    expect(document.activeElement).toBe(input());
    act(() => scroller().focus());
    fireEvent.keyDown(scroller(), { key: 'Delete' });

    expect(onEdit).not.toHaveBeenCalled();
    expect(editShotCredit).not.toHaveBeenCalled();
    expect(editor()).not.toBeNull();

    // Escape that reaches the grid closes the popover too.
    fireEvent.mouseDown(screen.getByTestId('credit-cell-editor-cancel'));
    act(() => scroller().focus());
    fireEvent.mouseUp(screen.getByTestId('credit-cell-editor-cancel'));
    fireEvent.keyDown(scroller(), { key: 'Escape' });
    expect(editor()).toBeNull();
    expect(editShotCredit).not.toHaveBeenCalled();
  });

  it('Escape in the field closes the popover and hands the keyboard back to the grid', () => {
    renderTable();
    doubleClick(0, 'surveyors');
    fireEvent.keyDown(input(), { key: 'Escape' });

    expect(editor()).toBeNull();
    expect(document.activeElement).toBe(scroller());
    expect(editShotCredit).not.toHaveBeenCalled();
  });

  it('Enter on the empty field is Done: one edit of that shot', () => {
    renderTable();
    click(0, 'surveyors');
    fireEvent.keyDown(scroller(), { key: 'B' });
    fireEvent.change(input(), { target: { value: 'Bo' } });
    fireEvent.keyDown(input(), { key: 'Enter' }); // the pill
    fireEvent.keyDown(input(), { key: 'Enter' }); // done

    expect(editor()).toBeNull();
    expect(editShotCredit).toHaveBeenCalledTimes(1);
    expect(editShotCredit).toHaveBeenCalledWith(
      { type: 'shots', shots: [[SECTION, 11]] },
      { set_surveyors: [{ name: 'Ann' }, { name: 'Bo' }] },
    );
    expect(document.activeElement).toBe(scroller());
  });

  it('a press elsewhere in the grid is the user leaving, not a focus to take back', () => {
    renderTable();
    doubleClick(0, 'surveyors');
    click(1, 'surveyors');

    expect(editor()).toBeNull();
    expect(document.activeElement).toBe(scroller());
  });

  it('a popover that cannot find its cell closes instead of holding the keyboard', () => {
    const { onEdit } = renderTable({ cellIds: false });
    click(0, 'surveyors');
    fireEvent.keyDown(scroller(), { key: 'A' });
    expect(editor()).toBeNull();

    // The grid has its keys back.
    fireEvent.keyDown(scroller(), { key: 'Delete' });
    expect(onEdit).toHaveBeenCalledWith(0, 'surveyors', []);
  });

  it('the date cell opens its popover from a key with the date input focused', () => {
    const { onEdit } = renderTable();
    click(0, 'survey_date');
    fireEvent.keyDown(scroller(), { key: '2' });

    expect(editor()).toHaveAttribute('data-field', 'survey_date');
    expect(document.activeElement).toBe(screen.getByTestId('credit-cell-editor-date'));
    expect(onEdit).not.toHaveBeenCalled();
  });
});
