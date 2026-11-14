/**
 * The grid's sticky column-header row: labels, the frozen top-left corner above
 * the row-number gutter, and each column's resize handle.
 *
 * Split out of `SpreadsheetGrid` (#157-style): props in, JSX out, no state of
 * its own. Header drag-to-reorder and drag-to-select still live in the parent -
 * this only renders the targets and forwards the events.
 */
import { useLayoutEffect, useMemo, useRef, type MouseEvent as ReactMouseEvent, type ReactNode, type RefObject } from 'react';
import type { GridColumn } from './types';
import { DEFAULT_COL_WIDTH } from './types';
import type { Rect } from './gridGeometry';
import type { GridLabels } from './gridLabels';
import { GridSortButton, type SortDir } from './GridSortButton';
import { ChevronDownIcon, ChevronUpIcon } from '../icons';
import { useFirstVisibleRow } from './useFirstVisibleRow';

export interface GridHeaderRowProps<Row> {
  columns: GridColumn<Row>[];
  /** Resolved pixel width per visible column, parallel to {@link columns}. */
  widths: number[];
  /** Current selection rectangle, for the selected/active header tints. */
  rect: Rect;
  activeColIdx: number;
  rowCount: number;
  gutterWidth: number;
  headerHeight: number;
  /** Whether the scroller has moved, which draws the header's drop shadow. */
  scrolled: boolean;
  testIdPrefix: string;
  /** Present when the host owns column order - a fully-selected column then
   *  shows a grab cursor because it can be dragged to reorder. */
  reorderable: boolean;
  onHeaderMouseDown: (colIdx: number, e: ReactMouseEvent) => void;
  onHeaderMouseEnter: (colIdx: number) => void;
  onHeaderContextMenu?: (colKey: string, pos: { x: number; y: number }) => void;
  onStartResize: (e: ReactMouseEvent, key: string, startWidth: number) => void;
  onAutoFitColumn: (key: string) => void;
  /** Host-owned sort, drawn as the indicator + `aria-sort`. */
  sort?: { colKey: string; dir: SortDir } | null | undefined;
  /** Present when the host sorts: `sortable` columns get the sort button. */
  onSortChange?: ((colKey: string, dir: SortDir | null) => void) | undefined;
  labels: Pick<GridLabels, 'sortAscending' | 'sortDescending' | 'sortClear'>;
  /** Host content for the corner, given the first row visible below the
   *  header (-1 with no rows). Needs `scrollRef` to follow the scroll. */
  headerCorner?: ((firstVisibleRow: number) => ReactNode) | undefined;
  scrollRef?: RefObject<HTMLElement | null> | undefined;
}

/** The corner's host content. Owns the scroll subscription, so scrolling
 *  re-renders only this, and only when the first visible row changes. Left
 *  aligned and ellipsized; its text doubles as the tooltip. */
function GridHeaderCorner({
  render,
  scrollRef,
  rowCount,
  testIdPrefix,
}: {
  render: (firstVisibleRow: number) => ReactNode;
  scrollRef: RefObject<HTMLElement | null>;
  rowCount: number;
  testIdPrefix: string;
}) {
  const first = useFirstVisibleRow(scrollRef, rowCount);
  // Called again only for a new index (or a new host callback).
  const content = useMemo(() => render(first), [render, first]);
  const ref = useRef<HTMLSpanElement>(null);
  // The content is any node, so the tooltip is whatever text it rendered.
  useLayoutEffect(() => {
    const el = ref.current;
    const text = el?.textContent?.trim() ?? '';
    if (el && text) el.title = text;
    else el?.removeAttribute('title');
  });
  return (
    <span ref={ref} className="sg-corner-content" data-testid={`${testIdPrefix}-header-corner`} data-first-visible-row={first}>
      {content}
    </span>
  );
}

export function GridHeaderRow<Row>({
  columns,
  widths,
  rect,
  activeColIdx,
  rowCount,
  gutterWidth,
  headerHeight,
  scrolled,
  testIdPrefix,
  reorderable,
  onHeaderMouseDown,
  onHeaderMouseEnter,
  onHeaderContextMenu,
  onStartResize,
  onAutoFitColumn,
  sort,
  onSortChange,
  labels,
  headerCorner,
  scrollRef,
}: GridHeaderRowProps<Row>) {
  return (
    <div
      role="row"
      aria-rowindex={1}
      className={`sg-header-bar sticky top-0 z-30 flex bg-panel ${scrolled ? 'sg-scrolled' : ''}`}
      style={{ height: headerHeight }}
    >
      {/* Frozen top-left corner above the row-number gutter. */}
      <div
        className="sg-corner sticky left-0 z-30 flex items-center justify-center border-b border-r border-white/10 bg-panel"
        style={{ width: gutterWidth, minWidth: gutterWidth }}
      >
        {headerCorner && scrollRef && (
          <GridHeaderCorner render={headerCorner} scrollRef={scrollRef} rowCount={rowCount} testIdPrefix={testIdPrefix} />
        )}
      </div>
      {columns.map((c, i) => {
        const fullColSel = rect.r0 === 0 && rect.r1 === rowCount - 1;
        const colSelected = fullColSel && i >= rect.c0 && i <= rect.c1;
        const dir = sort && sort.colKey === c.key ? sort.dir : null;
        return (
          <div
            key={c.key}
            role="columnheader"
            aria-colindex={i + 1}
            aria-sort={dir === 'asc' ? 'ascending' : dir === 'desc' ? 'descending' : undefined}
            title={c.headerTitle}
            className={`sg-header relative flex items-center border-b border-r border-white/10 px-2 text-xs font-medium text-text-secondary ${
              i >= rect.c0 && i <= rect.c1 ? 'sg-header-selected text-text-primary' : ''
            } ${i === activeColIdx ? 'sg-header-active' : ''}${dir ? ' sg-header-sorted' : ''}${
              c.headerClassName ? ` ${c.headerClassName}` : ''
            }`}
            style={{
              width: widths[i],
              minWidth: widths[i],
              cursor: colSelected && reorderable ? 'grab' : 'pointer',
            }}
            onMouseDown={(e) => onHeaderMouseDown(i, e)}
            onMouseEnter={() => onHeaderMouseEnter(i)}
            onContextMenu={(e) => {
              if (!onHeaderContextMenu) return;
              e.preventDefault();
              onHeaderContextMenu(c.key, { x: e.clientX, y: e.clientY });
            }}
            data-testid={`${testIdPrefix}-header-${c.key}`}
          >
            <span className="flex-1 overflow-hidden text-ellipsis whitespace-nowrap">
              {c.header}
            </span>
            {/* After the label: autofit measures the header's FIRST span. */}
            {c.sortable && onSortChange ? (
              <GridSortButton
                colKey={c.key}
                header={c.header}
                dir={dir}
                testIdPrefix={testIdPrefix}
                labels={labels}
                onSortChange={onSortChange}
              />
            ) : dir ? (
              // Sorted by the host without a toggle here: indicator only.
              <span className="sg-sort-indicator" aria-hidden="true">
                {dir === 'asc' ? <ChevronUpIcon size={14} /> : <ChevronDownIcon size={14} />}
              </span>
            ) : null}
            {/* Drag this edge to resize the column. The handle normally
                straddles the boundary (right: -2px); on the last column that
                boundary is the content's right edge, so keep it inside the
                cell to avoid a few-px phantom horizontal scrollbar. */}
            <div
              className={`sg-col-resize${i === columns.length - 1 ? ' sg-col-resize-end' : ''}`}
              data-testid={`${testIdPrefix}-col-resize-${c.key}`}
              onMouseDown={(e) => onStartResize(e, c.key, widths[i] ?? DEFAULT_COL_WIDTH)}
              onClick={(e) => e.stopPropagation()}
              onDoubleClick={(e) => {
                e.stopPropagation();
                onAutoFitColumn(c.key);
              }}
              onDragStart={(e) => e.preventDefault()}
            />
          </div>
        );
      })}
    </div>
  );
}
