/**
 * One rendered grid row: the frozen gutter cell plus either the per-column
 * cells or, for a group-header row, a single full-width band.
 *
 * Memoized. Everything that changes on a selection / hover / edit move arrives
 * as per-row PRIMITIVES (`selC0`/`selC1` are -1 outside the selected rows,
 * `activeColIdx` is -1 off the active row, ...), so a selection change re-renders
 * only the rows whose own state changed instead of every visible row. Event
 * handlers come through one stable {@link GridRowHandlers} object that the grid
 * points at its latest callbacks, so they never break the memo either.
 *
 * Host callbacks (`rowClassName`, `cellMarks`, `rowStyle`, ...) are called per
 * rendered row/cell and must be O(1).
 */
import { memo, type CSSProperties, type MouseEvent as ReactMouseEvent, type ReactNode } from 'react';
import type { CellMarks, GridColumn } from './types';
import { ROW_HEIGHT } from './types';
import { columnApplies, type GroupRowContent } from './gridApplicable';
import { MarkedText, markCellClass } from './gridMarks';

export interface GridRowHandlers {
  cellMouseDown: (rowIdx: number, colIdx: number, e: ReactMouseEvent) => void;
  cellMouseEnter: (rowIdx: number, colIdx: number) => void;
  cellContextMenu: (rowIdx: number, colIdx: number, e: ReactMouseEvent) => void;
  cellDoubleClick: (rowIdx: number, colIdx: number) => void;
  gutterMouseDown: (rowIdx: number, e: ReactMouseEvent) => void;
  gutterMouseEnter: (rowIdx: number) => void;
  gutterContextMenu: (rowIdx: number, e: ReactMouseEvent) => void;
  /** Pointer over a group band: hover + drag-select extension. */
  bandMouseEnter: (rowIdx: number) => void;
}

export interface GridRowProps<Row> {
  row: Row;
  rowIdx: number;
  /** Row top inside the rows area (px). */
  top: number;
  totalWidth: number;
  gutterWidth: number;
  /** Visible columns, and their resolved widths (parallel arrays). */
  columns: GridColumn<Row>[];
  widths: number[];
  testIdPrefix: string;
  /** Selected column span on this row, or -1/-1 when the row is outside the
   *  selection rectangle's rows. */
  selC0: number;
  selC1: number;
  isActiveRow: boolean;
  /** Visible index of the active cell's column on the active row, else -1. */
  activeColIdx: number;
  /** Draw the thin ring on the active cell (multi-cell selection). */
  activeRing: boolean;
  /** Column key of the open editor on this row, else null. */
  editingColKey: string | null;
  hovered: boolean;
  /** The gutter shows a grab cursor (whole selected rows can be dragged). */
  gutterGrab: boolean;
  gridHasWritableColumns: boolean;
  isMissing: (rowIdx: number, colKey: string) => boolean;
  handlers: GridRowHandlers;
  rowClassName?: ((row: Row, rowIndex: number) => string | undefined) | undefined;
  cellClassName?: ((row: Row, rowIndex: number, colKey: string) => string | undefined) | undefined;
  rowLabel?: ((row: Row, rowIndex: number) => ReactNode) | undefined;
  rowTestId?: ((row: Row, index: number) => string | undefined) | undefined;
  cellTestId?: ((row: Row, index: number, colKey: string) => string | undefined) | undefined;
  rowStyle?: ((row: Row, rowIndex: number) => CSSProperties | undefined) | undefined;
  cellMarks?: ((row: Row, rowIndex: number, colKey: string) => CellMarks | undefined) | undefined;
  groupRowContent?: GroupRowContent<Row> | undefined;
}

const alignClass = (align: GridColumn<unknown>['align']) =>
  align === 'right' ? 'justify-end' : align === 'center' ? 'justify-center' : '';

function hasTint(style: CSSProperties | undefined): boolean {
  if (!style) return false;
  const tint = (style as Record<string, unknown>)['--sg-row-tint'];
  return tint !== undefined && tint !== null && tint !== '';
}

function GridRowInner<Row>(props: GridRowProps<Row>) {
  const {
    row, rowIdx, top, totalWidth, gutterWidth, columns, widths, testIdPrefix,
    selC0, selC1, isActiveRow, activeColIdx, activeRing, editingColKey, hovered, gutterGrab,
    gridHasWritableColumns, isMissing, handlers,
    rowClassName, cellClassName, rowLabel, rowTestId, cellTestId, rowStyle, cellMarks, groupRowContent,
  } = props;

  const groupContent = groupRowContent?.(row, rowIdx);
  const isGroup = groupContent !== undefined && groupContent !== null && groupContent !== false;
  const extraRowClass = rowClassName?.(row, rowIdx);
  const hostStyle = rowStyle?.(row, rowIdx);
  const rowSelected = selC0 >= 0;

  return (
    <div
      role="row"
      aria-rowindex={rowIdx + 2}
      className={`sg-row absolute left-0 flex${hovered ? ' sg-row-hover' : ''}${hasTint(hostStyle) ? ' sg-row-tinted' : ''}${
        isGroup ? ' sg-row-group' : ''
      }${extraRowClass ? ` ${extraRowClass}` : ''}`}
      // The grid's own geometry wins over anything the host style sets.
      style={{ ...hostStyle, top, height: ROW_HEIGHT, width: totalWidth }}
      data-testid={`${testIdPrefix}-row-${rowIdx}`}
      data-active={isActiveRow ? 'true' : undefined}
    >
      {/* Frozen row-number gutter; click to select, drag a selected row to
          move it (mirrors column move). */}
      <div
        role="rowheader"
        className={`sg-gutter sticky left-0 z-20 flex items-center justify-center border-b border-r border-white/10 bg-panel text-text-secondary ${
          rowSelected ? 'sg-gutter-selected' : ''
        } ${isActiveRow ? 'sg-gutter-active' : ''}`}
        style={{ width: gutterWidth, minWidth: gutterWidth, cursor: gutterGrab ? 'grab' : 'pointer' }}
        data-testid={`${testIdPrefix}-gutter-${rowIdx}`}
        onMouseDown={(e) => handlers.gutterMouseDown(rowIdx, e)}
        onMouseEnter={() => handlers.gutterMouseEnter(rowIdx)}
        onContextMenu={(e) => handlers.gutterContextMenu(rowIdx, e)}
      >
        <span className="flex h-full w-full items-center justify-center" data-testid={rowTestId?.(row, rowIdx)}>
          {rowLabel ? rowLabel(row, rowIdx) : rowIdx + 1}
        </span>
      </div>
      {isGroup ? (
        // One band over every column. A click selects the row like the gutter
        // does; interactive content must stop mousedown/click propagation.
        <div
          role="gridcell"
          aria-colindex={1}
          aria-colspan={columns.length}
          aria-selected={rowSelected}
          className="sg-group-band flex items-center border-b"
          style={{ width: totalWidth - gutterWidth, minWidth: totalWidth - gutterWidth }}
          data-testid={`${testIdPrefix}-group-${rowIdx}`}
          onMouseDown={(e) => handlers.gutterMouseDown(rowIdx, e)}
          onMouseEnter={() => handlers.bandMouseEnter(rowIdx)}
          onContextMenu={(e) => handlers.gutterContextMenu(rowIdx, e)}
        >
          {/* Sticky just right of the gutter, so the label stays in view while
              the grid scrolls horizontally. */}
          <div className="sg-group-content" style={{ left: gutterWidth }}>
            {groupContent}
          </div>
        </div>
      ) : (
        columns.map((col, c) => {
          const inSel = c >= selC0 && c <= selC1;
          const isActive = c === activeColIdx;
          const applicable = columnApplies(col, row, rowIdx);
          let cls: string;
          let content: ReactNode = null;
          if (!applicable) {
            // Blank and quiet: no getValue/format/render, no host classes.
            cls = ' sg-cell-na';
          } else {
            const missing = isMissing(rowIdx, col.key);
            // An applicable cell that can't be edited while the grid has
            // editable columns at all (read-only column, frozen geometry,
            // read-only mode) reads as visibly locked. Fully read-only grids
            // (the raw viewer) stay uniform instead.
            const editable = col.isEditable ? col.isEditable(row, rowIdx) : !!col.editable;
            const locked = gridHasWritableColumns && !editable;
            const extraCellClass = cellClassName?.(row, rowIdx, col.key);
            const marks = cellMarks?.(row, rowIdx, col.key);
            cls = `${missing ? ' sg-cell-missing' : ''}${locked ? ' sg-cell-locked' : ''}${markCellClass(marks)}${
              extraCellClass ? ` ${extraCellClass}` : ''
            }`;
            if (editingColKey !== col.key) {
              if (col.render) {
                content = col.render(row, rowIdx);
              } else {
                const text = missing
                  ? ''
                  : col.format
                    ? col.format(col.getValue(row, rowIdx), row)
                    : String(col.getValue(row, rowIdx) ?? '');
                // Plain text truncates with an ellipsis + tooltip.
                content = (
                  <span className="sg-cell-text" title={text || undefined}>
                    {marks && text ? <MarkedText text={text} marks={marks} /> : text}
                  </span>
                );
              }
            }
          }
          return (
            <div
              key={col.key}
              id={`sg-cell-${rowIdx}-${col.key}`}
              role="gridcell"
              aria-colindex={c + 1}
              aria-selected={inSel}
              className={`sg-cell flex items-center overflow-hidden border-b border-r border-white/5 px-2 text-xs ${alignClass(
                col.align,
              )}${inSel && !isActive ? ' sg-cell-sel' : ''}${cls}`}
              style={{
                width: widths[c],
                minWidth: widths[c],
                // Thin ring on the active cell inside a multi-cell range.
                boxShadow: isActive && activeRing ? 'inset 0 0 0 1px var(--sg-blue)' : undefined,
              }}
              data-testid={`${testIdPrefix}-cell-${rowIdx}-${col.key}`}
              data-in-selection={inSel ? 'true' : undefined}
              data-active-cell={isActive ? 'true' : undefined}
              onMouseDown={(e) => handlers.cellMouseDown(rowIdx, c, e)}
              onMouseEnter={() => handlers.cellMouseEnter(rowIdx, c)}
              onContextMenu={(e) => handlers.cellContextMenu(rowIdx, c, e)}
              onDoubleClick={(e) => {
                e.stopPropagation();
                handlers.cellDoubleClick(rowIdx, c);
              }}
            >
              {/* Content always renders; the editor overlays the active cell
                  when editing so it fills the whole cell. */}
              <div
                className={`flex min-w-0 flex-1 self-stretch items-center overflow-hidden ${alignClass(col.align)}`}
                data-testid={cellTestId?.(row, rowIdx, col.key)}
              >
                {content}
              </div>
            </div>
          );
        })
      )}
    </div>
  );
}

/** Memoized row. Typed as a generic component (React.memo drops generics). */
export const GridRow = memo(GridRowInner) as typeof GridRowInner;
