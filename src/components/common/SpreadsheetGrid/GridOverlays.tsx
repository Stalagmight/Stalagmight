/**
 * Everything the grid draws OVER its rows: the selection rectangle (with the
 * move edges and fill handle), the copy/cut marquees, the fill preview, the
 * cut-and-drop target, the rejected-edit flash, the cell editor and the
 * column/row move drop lines.
 *
 * Split out of `SpreadsheetGrid` (#157-style): props in, JSX out, no state of
 * its own. All of it is positioned from row height + column offsets (the data
 * model), never from rendered cells, so it survives row virtualization.
 */
import type {
  CSSProperties,
  Dispatch,
  MouseEvent as ReactMouseEvent,
  MutableRefObject,
  RefObject,
  SetStateAction,
} from 'react';
import { ROW_HEIGHT } from './types';
import type { Rect } from './gridGeometry';

/** Absolute box of a cell rectangle inside the rows area (row 0 at y=0). */
export function rectOverlay(r: Rect, gutterWidth: number, offsets: number[]): CSSProperties {
  return {
    position: 'absolute',
    top: r.r0 * ROW_HEIGHT,
    left: gutterWidth + (offsets[r.c0] ?? 0),
    width: (offsets[r.c1 + 1] ?? 0) - (offsets[r.c0] ?? 0),
    height: (r.r1 - r.r0 + 1) * ROW_HEIGHT,
    pointerEvents: 'none',
  };
}

export interface GridRangeOverlaysProps {
  rect: Rect;
  /** Draw the selection rectangle at all (the grid has rows and columns). */
  hasCells: boolean;
  copiedRect: Rect | null;
  cutRect: Rect | null;
  fillPreview: Rect | null;
  cellMoveDrop: Rect | null;
  /** The rejected-edit flash, as a single-cell rect. */
  invalidRect: Rect | null;
  /** A drag is in progress: the selection rectangle tracks 1:1, no glide. */
  interacting: boolean;
  showMoveEdges: boolean;
  showFillHandle: boolean;
  gutterWidth: number;
  offsets: number[];
  testIdPrefix: string;
  onMoveBorderDown: (e: ReactMouseEvent) => void;
  onFillHandleDown: (e: ReactMouseEvent) => void;
  onFillHandleDoubleClick: () => void;
}

export function GridRangeOverlays(p: GridRangeOverlaysProps) {
  const box = (r: Rect) => rectOverlay(r, p.gutterWidth, p.offsets);
  const { rect } = p;
  return (
    <>
      {/* Marching-ants for a copied range; a pending cut renders its own
          (visually distinct) marquee instead. */}
      {p.copiedRect && !p.cutRect && (
        <div
          className="sg-marching-ants"
          data-testid={`${p.testIdPrefix}-copy-marquee`}
          style={{ ...box(p.copiedRect), zIndex: 3 }}
        />
      )}
      {p.cutRect && (
        <div
          className="sg-cut-marquee"
          data-testid={`${p.testIdPrefix}-cut-marquee`}
          style={{ ...box(p.cutRect), zIndex: 3 }}
        />
      )}

      {/* Fill-drag preview. */}
      {p.fillPreview && <div className="sg-fill-preview" style={{ ...box(p.fillPreview), zIndex: 4 }} />}

      {/* Cut-and-drop move target outline. */}
      {p.cellMoveDrop && (
        <div className="sg-move-target" data-testid="grid-move-target" style={{ ...box(p.cellMoveDrop), zIndex: 5 }} />
      )}

      {/* Selection rectangle overlay (survives row virtualization). */}
      {p.hasCells && (
        <div
          className={`sg-selection-overlay ${p.interacting ? 'sg-no-anim' : ''}`}
          style={{ ...box(rect), border: '2px solid var(--sg-blue)', boxSizing: 'border-box', zIndex: 3 }}
          data-testid={`${p.testIdPrefix}-selection-overlay`}
          data-rect={`${rect.r0},${rect.c0},${rect.r1},${rect.c1}`}
        >
          {/* Grab any edge (not the interior) to move the cells. */}
          {p.showMoveEdges && (
            <>
              <div className="sg-move-edge" style={{ top: -3, left: 0, right: 0, height: 6 }} onMouseDown={p.onMoveBorderDown} />
              <div className="sg-move-edge" style={{ bottom: -3, left: 0, right: 0, height: 6 }} onMouseDown={p.onMoveBorderDown} />
              <div className="sg-move-edge" style={{ top: 0, bottom: 0, left: -3, width: 6 }} onMouseDown={p.onMoveBorderDown} />
              <div className="sg-move-edge" style={{ top: 0, bottom: 0, right: -3, width: 6 }} onMouseDown={p.onMoveBorderDown} />
            </>
          )}
          {p.showFillHandle && (
            <div
              className="sg-fill-handle"
              data-testid={`${p.testIdPrefix}-fill-handle`}
              onMouseDown={p.onFillHandleDown}
              onDoubleClick={p.onFillHandleDoubleClick}
            />
          )}
        </div>
      )}

      {/* Rejected-edit flash (validate returned an error). Above the editor. */}
      {p.invalidRect && (
        <div
          className="sg-invalid-flash"
          data-testid={`${p.testIdPrefix}-invalid-flash`}
          style={{ ...box(p.invalidRect), zIndex: 25 }}
        />
      )}
    </>
  );
}

let editorMeasureCtx: CanvasRenderingContext2D | null = null;
/** Width of the widest line at the editor's font - drives the GS-style
 *  grow-rightward single-line editor. */
function measureEditorText(text: string): number {
  if (!editorMeasureCtx) editorMeasureCtx = document.createElement('canvas').getContext('2d');
  if (!editorMeasureCtx) return 0;
  editorMeasureCtx.font = '13px Arial, Roboto, sans-serif';
  let max = 0;
  for (const line of text.split('\n')) {
    max = Math.max(max, editorMeasureCtx.measureText(line).width);
  }
  return max;
}

export interface CellEditorState {
  row: number;
  colKey: string;
  draft: string;
  /** The draft as seeded from the model - committing an unchanged draft is a
   *  no-op (never rewrite stored data with a re-parsed display value). */
  initial: string;
}

export interface GridCellEditorProps {
  editing: CellEditorState;
  setEditing: Dispatch<SetStateAction<CellEditorState | null>>;
  colIdx: number;
  multiline: boolean;
  ariaLabel: string;
  editorRef: RefObject<HTMLTextAreaElement>;
  scrollRef: RefObject<HTMLDivElement | null>;
  /** Set by the grid's own refocus: the blur it causes must not commit. */
  suppressBlurCommitRef: MutableRefObject<boolean>;
  onBlurCommit: () => void;
  gutterWidth: number;
  offsets: number[];
  testIdPrefix: string;
}

/** The in-place editor: overlays the active cell so it fills the whole cell
 *  and sits above the selection overlay. */
export function GridCellEditor(p: GridCellEditorProps) {
  const { editing } = p;
  const cellRect = rectOverlay(
    { r0: editing.row, r1: editing.row, c0: p.colIdx, c1: p.colIdx },
    p.gutterWidth,
    p.offsets,
  );
  let style: CSSProperties;
  if (p.multiline) {
    // Multiline columns (comments) get a roomy wrap box anchored at the cell
    // instead of one squeezed into it.
    cellRect.width = Math.max(Number(cellRect.width) || 0, 280);
    cellRect.height = Math.max(Number(cellRect.height) || 0, 110);
    style = { ...cellRect, pointerEvents: 'auto', zIndex: 20 };
  } else {
    // Single-line editors never wrap: like Google Sheets they grow rightward
    // over the neighbours as the draft gets longer, capped at the scroller's
    // right edge.
    const el = p.scrollRef.current;
    const left = Number(cellRect.left) || 0;
    const maxWidth = el ? el.scrollLeft + el.clientWidth - left - 2 : Infinity;
    const wanted = measureEditorText(editing.draft) + 20;
    cellRect.width = Math.max(Number(cellRect.width) || 0, Math.min(wanted, maxWidth));
    style = { ...cellRect, pointerEvents: 'auto', zIndex: 20, whiteSpace: 'pre' };
  }
  return (
    <textarea
      ref={p.editorRef}
      data-testid={`${p.testIdPrefix}-cell-editor`}
      aria-label={p.ariaLabel}
      className="sg-editor"
      wrap={p.multiline ? 'soft' : 'off'}
      style={style}
      value={editing.draft}
      onChange={(ev) => p.setEditing({ ...editing, draft: ev.target.value })}
      onBlur={() => {
        // A blur caused by commitEdit's or cancelEdit's own refocus must NOT
        // commit - the draft is already committed/discarded.
        if (p.suppressBlurCommitRef.current) {
          p.suppressBlurCommitRef.current = false;
          return;
        }
        p.onBlurCommit();
      }}
      onKeyDown={(ev) => {
        if (['ArrowLeft', 'ArrowRight', 'Home', 'End'].includes(ev.key)) ev.stopPropagation();
      }}
    />
  );
}

/** Column- and row-move drop indicators (they scroll with the content). */
export function GridDropLines({
  colLineX,
  rowLineY,
  height,
  width,
}: {
  colLineX: number | null;
  rowLineY: number | null;
  /** Header + body height, for the column line. */
  height: number;
  /** Full content width, for the row line. */
  width: number;
}) {
  const line: CSSProperties = { position: 'absolute', zIndex: 40, pointerEvents: 'none' };
  return (
    <>
      {colLineX !== null && <div className="sg-drop-line" style={{ ...line, top: 0, left: colLineX, width: 2, height }} />}
      {rowLineY !== null && <div className="sg-drop-line" style={{ ...line, left: 0, top: rowLineY, height: 2, width }} />}
    </>
  );
}
