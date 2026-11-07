/**
 * The width map SpreadsheetGrid hands its host on a resize commit, exercised
 * through the real drag path (mousedown on the handle, window mousemove, window
 * mouseup) rather than through the hook.
 *
 * Why this tier: `useColumnWidths` prunes any key outside the `declaredKeys` it
 * is given (#357 item 6), so the whole hidden-column guarantee reduces to WHICH
 * key set SpreadsheetGrid passes - `orderedKeys` (declared, hidden included) and
 * not `visibleColumns`. A hook-level test cannot see that choice: it supplies
 * the key set itself, so it passes just as happily with the prune removed
 * entirely. Only a grid-level render pins the argument.
 */
import { describe, it, expect, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { SpreadsheetGrid } from '../SpreadsheetGrid';
import type { GridColumn } from '../types';
import { NoopResizeObserver } from '../../../../test/factories/dom';

interface Row {
  a: string;
  b: string;
  note: string;
}

const columns: GridColumn<Row>[] = [
  { key: 'a', header: 'A', width: 120, getValue: (r) => r.a },
  { key: 'b', header: 'B', width: 120, getValue: (r) => r.b },
  { key: 'note', header: 'Note', width: 120, getValue: (r) => r.note },
];

const rows: Row[] = [{ a: '1', b: '2', note: 'n' }];

/** Walk the real resize drag on `key`: press the handle, move `dx` px, release.
 *  mousemove/mouseup are bound to `window`, not to the handle.
 *
 *  `buttons: 1` is load-bearing: the move handler treats a buttonless move
 *  during a latched drag as "the mouseup happened outside the window" and jumps
 *  straight to the release path, so a default fireEvent.mouseMove would commit
 *  the ORIGINAL width and this helper would silently measure nothing. */
function dragResize(key: string, startX: number, dx: number) {
  fireEvent.mouseDown(screen.getByTestId(`grid-col-resize-${key}`), { clientX: startX, buttons: 1 });
  fireEvent.mouseMove(window, { clientX: startX + dx, buttons: 1 });
  fireEvent.mouseUp(window);
}

describe('SpreadsheetGrid column-width persistence', () => {
  it('keeps a hidden column width when a DIFFERENT column is resized', () => {
    // The regression this pins: passing `visibleColumns` (or `colKeys`) into
    // useColumnWidths instead of `orderedKeys` makes `note` read as an orphan
    // while it is hidden, so resizing `a` silently drops the width the user set
    // on `note` - and it comes back at the default the moment it is re-shown.
    // jsdom has no ResizeObserver; the grid measures its scroller with one on
    // mount. This suite never depends on it firing - `availWidth` stays 0, which
    // makes every rendered width the pinned/declared one.
    vi.stubGlobal('ResizeObserver', NoopResizeObserver);
    const onColumnWidthsChange = vi.fn();
    render(
      <SpreadsheetGrid
        columns={columns}
        rows={rows}
        rowKey={(_r: Row, i: number) => i}
        onEdit={() => {}}
        hiddenColumns={['note']}
        columnWidths={{ a: 120, note: 250 }}
        onColumnWidthsChange={onColumnWidthsChange}
      />,
    );

    // `note` is genuinely hidden - it has no header, so it cannot be resized
    // here and its width can only survive by being carried through untouched.
    expect(screen.queryByTestId('grid-header-note')).toBeNull();

    dragResize('a', 300, 40);

    expect(onColumnWidthsChange).toHaveBeenCalledTimes(1);
    expect(onColumnWidthsChange).toHaveBeenCalledWith({ a: 160, note: 250 });
    vi.unstubAllGlobals();
  });

  it('still prunes a width whose column this grid does not declare at all', () => {
    // The other half of the same argument: widening the key set to `orderedKeys`
    // must not turn the prune off. A key from a removed/renamed column is not
    // "hidden", it is an orphan, and #357 item 6 is that it was re-persisted
    // forever and grew config.toml monotonically.
    vi.stubGlobal('ResizeObserver', NoopResizeObserver);
    const onColumnWidthsChange = vi.fn();
    render(
      <SpreadsheetGrid
        columns={columns}
        rows={rows}
        rowKey={(_r: Row, i: number) => i}
        onEdit={() => {}}
        hiddenColumns={['note']}
        columnWidths={{ a: 120, note: 250, removed_in_v11: 300 }}
        onColumnWidthsChange={onColumnWidthsChange}
      />,
    );

    dragResize('a', 300, 40);

    expect(onColumnWidthsChange).toHaveBeenCalledWith({ a: 160, note: 250 });
    vi.unstubAllGlobals();
  });
});
