/**
 * The grid's right-click menu.
 *
 * Split out of `SpreadsheetGrid` (#157-style): props in, JSX out, no state of
 * its own. The row entries appear only when the host supplied the matching
 * callback - that absence IS the read-only gate, so a host that withholds
 * `onInsertRows` gets no Insert entry rather than a dead one.
 */
import { Fragment, type MutableRefObject } from 'react';
import type { GridLabels } from './gridLabels';
import type { GridExtraMenuItem } from './types';

export interface GridContextMenuProps {
  /** Viewport position of the click that opened it. */
  pos: { x: number; y: number };
  labels: GridLabels;
  /** Selection-shape suffix for Copy/Cut/Clear ('', ' 3 shots', ' 2 columns'). */
  shapeSuffix: string;
  /** Counted-row label for the insert/delete entries ('shot', '3 shots'). */
  rowsLabel: string;
  /** Selected row range, for the insert/delete callbacks. */
  fromRow: number;
  toRow: number;
  testIdPrefix: string;
  onCopy: () => void;
  onCut: () => void;
  onPaste: () => void;
  onClear: () => void;
  onFillDown: () => void;
  onInsertRows?: (beforeIndex: number, count: number) => void;
  onDeleteRows?: (fromRow: number, toRow: number) => boolean | void;
  /** Host entries appended after the built-in ones. */
  extraItems: GridExtraMenuItem[];
  /** Close the menu; every entry calls this after acting. */
  onClose: () => void;
  menuRef: MutableRefObject<HTMLDivElement | null>;
}

export function GridContextMenu({
  pos,
  labels,
  shapeSuffix,
  rowsLabel,
  fromRow,
  toRow,
  testIdPrefix,
  onCopy,
  onCut,
  onPaste,
  onClear,
  onFillDown,
  onInsertRows,
  onDeleteRows,
  extraItems,
  onClose,
  menuRef,
}: GridContextMenuProps) {
  const act = (fn: () => void) => () => {
    fn();
    onClose();
  };
  const rowCount = toRow - fromRow + 1;
  return (
    <div
      ref={menuRef}
      data-dropdown-open
      className="sg-context-menu"
      data-testid={`${testIdPrefix}-context-menu`}
      style={{ position: 'fixed', top: pos.y, left: pos.x, zIndex: 60 }}
      onMouseDown={(e) => e.stopPropagation()}
    >
      <button type="button" data-testid={`${testIdPrefix}-context-copy`} onClick={act(onCopy)}>
        {labels.copy(shapeSuffix)}
        <span className="sg-shortcut">{labels.shortcutCopy}</span>
      </button>
      <button type="button" data-testid={`${testIdPrefix}-context-cut`} onClick={act(onCut)}>
        {labels.cut(shapeSuffix)}
        <span className="sg-shortcut">{labels.shortcutCut}</span>
      </button>
      <button type="button" data-testid={`${testIdPrefix}-context-paste`} onClick={act(onPaste)}>
        {labels.paste}
        <span className="sg-shortcut">{labels.shortcutPaste}</span>
      </button>
      {(onInsertRows || onDeleteRows) && <div className="sg-context-sep" />}
      {onInsertRows && (
        <button
          type="button"
          data-testid={`${testIdPrefix}-context-insert-above`}
          onClick={act(() => onInsertRows(fromRow, rowCount))}
        >
          {labels.insertAbove(rowsLabel)}
        </button>
      )}
      {onInsertRows && (
        <button
          type="button"
          data-testid={`${testIdPrefix}-context-insert-below`}
          onClick={act(() => onInsertRows(toRow + 1, rowCount))}
        >
          {labels.insertBelow(rowsLabel)}
        </button>
      )}
      {onDeleteRows && (
        <button
          type="button"
          data-testid={`${testIdPrefix}-context-delete-rows`}
          onClick={act(() => onDeleteRows(fromRow, toRow))}
        >
          {labels.deleteRows(rowsLabel)}
        </button>
      )}
      <div className="sg-context-sep" />
      <button type="button" data-testid={`${testIdPrefix}-context-clear`} onClick={act(onClear)}>
        {labels.clear(shapeSuffix)}
        <span className="sg-shortcut">{labels.shortcutClear}</span>
      </button>
      <button
        type="button"
        data-testid={`${testIdPrefix}-context-fill-down`}
        onClick={act(onFillDown)}
      >
        {labels.fillDown}
        <span className="sg-shortcut">{labels.shortcutFillDown}</span>
      </button>
      {extraItems.length > 0 && <div className="sg-context-sep" />}
      {extraItems.map((item, index) => (
        // Index-keyed: two host entries may share a label.
        <Fragment key={`${index}:${item.label}`}>
          {item.separatorBefore && index > 0 && <div className="sg-context-sep" />}
          <button
            type="button"
            data-testid={item.testId ?? `${testIdPrefix}-context-extra-${index}`}
            disabled={item.disabled}
            aria-disabled={item.disabled || undefined}
            onClick={item.disabled ? undefined : act(item.onClick)}
          >
            {item.label}
          </button>
        </Fragment>
      ))}
    </div>
  );
}
