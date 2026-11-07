import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest';
import { act, fireEvent, render, screen } from '@testing-library/react';

vi.mock('@tanstack/react-virtual', () => import('../../../../test/factories/virtualizer'));

import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';

/**
 * The grid's context menu, reached by a finger (#338).
 *
 * On a touch device the menu was simply unreachable: every path into it is a
 * right click. The hold now synthesises the `contextmenu` the cell, gutter and
 * header handlers already listen for, so what these tests check is parity with
 * the right click - the menu opens AND the selection moves the same way.
 *
 * The press resolves against the element the touchstart was dispatched on, so
 * the tests dispatch on the cell they mean. That is the production mechanism
 * too, not a test shortcut: no `elementFromPoint` is involved, which is what
 * lets this run under a virtualizer mock where every cell has a zero-size box.
 *
 * jsdom synthesises no compatibility mouse events, so "the menu survives the
 * lift that opened it" is the e2e spec's claim. What this file pins is the
 * half jsdom can see: the lift is swallowed exactly when a menu opened.
 */

interface Row {
  id: number;
  value: number;
}

const columns: GridColumn<Row>[] = [
  { key: 'value', header: 'Value', getValue: (r) => r.value, editable: true, kind: 'number' },
];

const rowsOf = (n: number): Row[] => Array.from({ length: n }, (_, i) => ({ id: i, value: i * 10 }));

function renderGrid(rowCount = 5) {
  return render(
    <SpreadsheetGrid
      columns={columns}
      rows={rowsOf(rowCount)}
      rowKey={(row) => row.id}
      onEdit={vi.fn()}
      onDeleteRows={vi.fn()}
      height={200}
    />
  );
}

/**
 * jsdom has `TouchEvent` but no `Touch` constructor, and its `TouchEventInit`
 * accepts plain objects in `touches` - all the long-press helper reads.
 */
function touchAt(clientX: number, clientY: number): Touch {
  return { identifier: 0, clientX, clientY } as unknown as Touch;
}

function dispatchTouch(target: EventTarget, type: string, touches: Touch[]): TouchEvent {
  const event = new TouchEvent(type, { bubbles: true, cancelable: true, touches });
  target.dispatchEvent(event);
  return event;
}

const startPress = (el: Element, x = 40, y = 60) =>
  dispatchTouch(el, 'touchstart', [touchAt(x, y)]);

const hold = (ms = 500) =>
  act(() => {
    vi.advanceTimersByTime(ms);
  });

const lift = (el: Element) => dispatchTouch(el, 'touchend', []);

const menu = () => screen.queryByTestId('grid-context-menu');

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe('SpreadsheetGrid long press', () => {
  it('opens the context menu on a cell, and moves the selection there', () => {
    renderGrid();
    startPress(screen.getByTestId('grid-cell-2-value'));
    hold();

    expect(menu()).not.toBeNull();
    // Right-click parity: pressing outside the selection takes it with you.
    expect(screen.getByTestId('grid-cell-2-value')).toHaveAttribute('data-active-cell', 'true');
  });

  it('swallows the lift that ends a press which opened the menu', () => {
    renderGrid();
    const cell = screen.getByTestId('grid-cell-2-value');
    startPress(cell);
    hold();

    expect(lift(cell).defaultPrevented).toBe(true);
  });

  it('opens the menu from the row gutter, with the row selected', () => {
    renderGrid();
    startPress(screen.getByTestId('grid-gutter-3'));
    hold();

    expect(menu()).not.toBeNull();
    expect(screen.getByTestId('grid-gutter-3').className).toContain('sg-gutter-selected');
  });

  it('leaves a tap alone', () => {
    renderGrid();
    const cell = screen.getByTestId('grid-cell-1-value');
    startPress(cell);
    hold(200);

    expect(menu()).toBeNull();
    expect(lift(cell).defaultPrevented).toBe(false);
  });

  it('cancels the press once the finger travels past 10 px', () => {
    // What separates a hold from a scroll, and the grid scrolls by finger.
    renderGrid();
    const cell = screen.getByTestId('grid-cell-1-value');
    startPress(cell, 100, 100);
    dispatchTouch(cell, 'touchmove', [touchAt(100, 115)]);
    hold();

    expect(menu()).toBeNull();
    expect(lift(cell).defaultPrevented).toBe(false);
  });

  it('stays out of the way inside the cell editor', () => {
    // The editor is the grid's one text-entry island: there a long press means
    // the native caret/paste gesture. The press must not even synthesise the
    // contextmenu - that no menu opens is weaker, since the editor overlay
    // has no onContextMenu handler above it to open one anyway.
    const onContextMenu = vi.fn();
    document.addEventListener('contextmenu', onContextMenu);
    renderGrid();
    fireEvent.doubleClick(screen.getByTestId('grid-cell-1-value'));
    const editor = screen.getByTestId('grid-cell-editor');
    startPress(editor);
    hold();

    expect(onContextMenu).not.toHaveBeenCalled();
    expect(menu()).toBeNull();
    expect(lift(editor).defaultPrevented).toBe(false);
    document.removeEventListener('contextmenu', onContextMenu);
  });

  it('closes again when a menu entry is picked', () => {
    renderGrid();
    const cell = screen.getByTestId('grid-cell-2-value');
    startPress(cell);
    hold();
    lift(cell);

    fireEvent.click(screen.getByTestId('grid-context-copy'));
    expect(menu()).toBeNull();
  });
});
