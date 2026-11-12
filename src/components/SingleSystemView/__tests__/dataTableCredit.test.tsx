/**
 * The data table's Surveyors / Explorers / Survey date columns: hidden by
 * default and offered in the columns menu, a clipboard form that round-trips,
 * grid bulk edits routed to one edit_shot_credit per shot, and the popover
 * editor's spreadsheet semantics (Enter / outside click commit once, Escape
 * cancels).
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import '../../../i18n';
import DataTable from '../DataTable';
import { CreditCellEditor } from '../CreditCellEditor';
import { formatPeopleCell, parseDateCell, parsePeopleCell } from '../dataTableCreditColumns';
import { buildShotUpdateBatch } from '../dataTableEdits';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import type { Shot } from '../../../types/project';

class NoopResizeObserver {
  observe() {}
  unobserve() {}
  disconnect() {}
}

const freshConfig = (): Config => structuredClone(defaultConfig) as Config;

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

beforeEach(() => {
  vi.stubGlobal('ResizeObserver', NoopResizeObserver);
  invokeIpcMock.mockReset();
  invokeIpcMock.mockImplementation((command: string) =>
    Promise.resolve(command === 'get_config' ? freshConfig() : undefined),
  );
  useProjectStore.setState({ config: freshConfig(), project: null });
  useUIStore.setState({ toasts: [] });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe('credit columns in the columns menu', () => {
  it('are hidden by default and can be shown', async () => {
    vi.useFakeTimers();
    try {
      render(<DataTable sections={[]} />);
      fireEvent.click(screen.getByTestId('data-table-columns-menu'));
      for (const key of ['surveyors', 'explorers', 'survey_date']) {
        expect(screen.getByTestId(`col-vis-${key}`)).not.toBeChecked();
      }
      fireEvent.click(screen.getByTestId('col-vis-surveyors'));
      expect(screen.getByTestId('col-vis-surveyors')).toBeChecked();
      await act(async () => {
        await vi.advanceTimersByTimeAsync(500);
      });
      const saves = invokeIpcMock.mock.calls.filter(([command]) => command === 'update_data_table_view');
      expect(saves[saves.length - 1]?.[1]).toMatchObject({
        hiddenColumns: ['explorers', 'section_id', 'shot_id', 'survey_date'],
      });
    } finally {
      vi.useRealTimers();
    }
  });
});

describe('credit cells on the clipboard', () => {
  it('round-trips people with and without an email, split on ";" only', () => {
    const people = [{ name: 'Doe, John' }, { name: 'Ann Diver', email: 'ann@caves.example' }];
    const text = formatPeopleCell(people);
    expect(text).toBe('Doe, John; Ann Diver <ann@caves.example>');
    expect(parsePeopleCell(text)).toEqual(people);
    expect(parsePeopleCell('  ')).toEqual([]);
    expect(parsePeopleCell('Ann; ann ; Bo')).toEqual([{ name: 'Ann' }, { name: 'Bo' }]);
  });

  it('accepts an ISO day, clears on empty and rejects anything else', () => {
    expect(parseDateCell('2026-03-02')).toBe('2026-03-02');
    expect(parseDateCell(' ')).toBeNull();
    expect(parseDateCell('2026-02-30')).toBeUndefined();
    expect(parseDateCell('March 2')).toBeUndefined();
  });
});

describe('credit cells in a bulk edit', () => {
  it('become one edit_shot_credit per shot beside the shot updates, skipping no-op cells', () => {
    const shots = [
      shot(11, { surveyors: [{ name: 'Ann' }], explorers: [], date: null }),
      shot(12),
    ];
    const commands = buildShotUpdateBatch(
      [
        { row: 0, colKey: 'surveyors', value: [{ name: 'Ann' }] }, // unchanged
        { row: 0, colKey: 'survey_date', value: '2026-03-02' },
        { row: 1, colKey: 'explorers', value: [{ name: 'Bo' }] },
        { row: 1, colKey: 'comment', value: 'sump' },
      ],
      shots,
      7,
    );
    expect(commands).toEqual([
      { type: 'edit_shot_credit', targets: { type: 'shots', shots: [[7, 11]] }, edit: { set_date: '2026-03-02' } },
      { type: 'edit_shot_credit', targets: { type: 'shots', shots: [[7, 12]] }, edit: { set_explorers: [{ name: 'Bo' }] } },
      { type: 'update_shot', section_id: 7, shot_id: 12, updates: { comment: 'sump' } },
    ]);
  });

  it('writes nothing for a clear of an already empty cell', () => {
    expect(buildShotUpdateBatch([{ row: 0, colKey: 'surveyors', value: [] }], [shot(1)], 1)).toEqual([]);
  });
});

describe('the credit cell popover', () => {
  function renderEditor(field: 'surveyors' | 'survey_date', credit?: Shot['credit']) {
    const onCommit = vi.fn();
    const onClose = vi.fn();
    const anchor = document.createElement('div');
    document.body.appendChild(anchor);
    render(<CreditCellEditor shot={shot(5, credit)} field={field} anchor={anchor} onCommit={onCommit} onClose={onClose} />);
    return { onCommit, onClose };
  }
  const input = () => screen.getByTestId('credit-cell-editor-people-input') as HTMLInputElement;

  it('commits ONE set of the whole list on Enter in the empty field', () => {
    const { onCommit, onClose } = renderEditor('surveyors', { surveyors: [{ name: 'Ann' }], explorers: [], date: null });
    fireEvent.change(input(), { target: { value: 'Bo' } });
    fireEvent.keyDown(input(), { key: 'Enter' }); // adds the pill
    expect(onCommit).not.toHaveBeenCalled();
    fireEvent.keyDown(input(), { key: 'Enter' }); // empty field: done
    expect(onCommit).toHaveBeenCalledTimes(1);
    expect(onCommit).toHaveBeenCalledWith({ set_surveyors: [{ name: 'Ann' }, { name: 'Bo' }] });
    expect(onClose).toHaveBeenCalled();
  });

  it('cancels on Escape (after the field let go of it) without writing', () => {
    const { onCommit, onClose } = renderEditor('surveyors');
    fireEvent.change(input(), { target: { value: 'Bo' } });
    fireEvent.keyDown(input(), { key: 'Enter' });
    fireEvent.keyDown(input(), { key: 'Escape' });
    expect(onCommit).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it('commits a name still being typed on an outside click', () => {
    const { onCommit } = renderEditor('surveyors');
    input().focus();
    fireEvent.change(input(), { target: { value: 'Cy Cave' } });
    fireEvent.mouseDown(document.body);
    expect(onCommit).toHaveBeenCalledWith({ set_surveyors: [{ name: 'Cy Cave' }] });
  });

  it('writes nothing when nothing changed', () => {
    const { onCommit, onClose } = renderEditor('surveyors', { surveyors: [{ name: 'Ann' }], explorers: [], date: null });
    fireEvent.click(screen.getByTestId('credit-cell-editor-done'));
    expect(onCommit).not.toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it('sets or clears the date of the shot', () => {
    const { onCommit } = renderEditor('survey_date', { surveyors: [], explorers: [], date: '2026-03-02' });
    const date = screen.getByTestId('credit-cell-editor-date');
    fireEvent.change(date, { target: { value: '' } });
    fireEvent.keyDown(date, { key: 'Enter' });
    expect(onCommit).toHaveBeenCalledWith({ set_date: null });
  });
});
