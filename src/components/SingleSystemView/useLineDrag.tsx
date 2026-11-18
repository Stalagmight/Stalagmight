/**
 * Drag lines onto a Section (#327): press a line row, move, release over a
 * group header (or any row of that group). Pointer events, not HTML5
 * drag-and-drop: WebKitGTK's DnD needs a native drag session that the Linux
 * webview runs unreliably, while pointer events behave the same in every
 * webview and in Playwright. Mouse and pen only - a touch press keeps the row's
 * long-press context menu, which offers "Move to Section" instead.
 *
 * The drop target is found under the pointer with `elementFromPoint` and the
 * rows' `data-drop-group` attribute, so it works across virtualized rows that
 * mount and unmount while the list auto-scrolls.
 */
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import type { LineId } from '../../types/project';
import { parseDropGroup, type GroupId } from './sectionsTree';

/** How far (px) the pointer travels before a press becomes a drag. */
const DRAG_THRESHOLD_PX = 5;
/** Distance (px) from the list's top / bottom edge that scrolls it. */
const EDGE_SCROLL_ZONE_PX = 28;
const EDGE_SCROLL_STEP_PX = 10;

export interface LineDragState {
  lineIds: LineId[];
  x: number;
  y: number;
  /** The group a release would move the lines to; null when none would. */
  target: GroupId | null;
}

interface Options {
  /** False while editing is blocked: no drag starts. */
  enabled: boolean;
  /** The scrolling list, for edge auto-scroll and to scope the drop search. */
  listRef: RefObject<HTMLElement | null>;
  /** The lines a drag started on `lineId` carries. */
  carry: (lineId: LineId) => LineId[];
  /** Whether releasing `lineIds` on `group` would change anything. */
  canDrop: (lineIds: readonly LineId[], group: GroupId) => boolean;
  onDrop: (lineIds: LineId[], group: GroupId) => void;
}

export function useLineDrag({ enabled, listRef, carry, canDrop, onDrop }: Options) {
  const [drag, setDrag] = useState<LineDragState | null>(null);
  const optsRef = useRef({ carry, canDrop, onDrop });
  optsRef.current = { carry, canDrop, onDrop };
  const cleanupRef = useRef<(() => void) | null>(null);
  // True for the click the browser sends right after a drop, so it does not
  // also select the row the drag started on.
  const justDroppedRef = useRef(false);

  useEffect(() => () => cleanupRef.current?.(), []);

  const onPointerDown = useCallback(
    (lineId: LineId, e: ReactPointerEvent) => {
      if (!enabled || e.button !== 0 || e.pointerType === 'touch') return;
      cleanupRef.current?.();
      const start = { x: e.clientX, y: e.clientY };
      let state: LineDragState | null = null;
      let last = start;
      let frame: number | null = null;

      const targetAt = (x: number, y: number, lineIds: readonly LineId[]): GroupId | null => {
        const list = listRef.current;
        const el = document.elementFromPoint(x, y);
        if (!list || !el || !list.contains(el)) return null;
        const group = parseDropGroup(el.closest('[data-drop-group]')?.getAttribute('data-drop-group'));
        return group !== null && optsRef.current.canDrop(lineIds, group) ? group : null;
      };
      const update = () => {
        if (!state) return;
        state = { ...state, x: last.x, y: last.y, target: targetAt(last.x, last.y, state.lineIds) };
        setDrag(state);
      };
      // Scroll while the pointer rests near an edge, re-reading the target as
      // rows slide under it.
      const edgeScroll = () => {
        frame = null;
        const list = listRef.current;
        if (!state || !list) return;
        const rect = list.getBoundingClientRect();
        const step =
          last.y < rect.top + EDGE_SCROLL_ZONE_PX ? -EDGE_SCROLL_STEP_PX
            : last.y > rect.bottom - EDGE_SCROLL_ZONE_PX ? EDGE_SCROLL_STEP_PX
              : 0;
        if (step === 0) return;
        list.scrollTop += step;
        update();
        frame = requestAnimationFrame(edgeScroll);
      };

      const onMove = (ev: PointerEvent) => {
        last = { x: ev.clientX, y: ev.clientY };
        if (!state) {
          if (Math.hypot(last.x - start.x, last.y - start.y) < DRAG_THRESHOLD_PX) return;
          state = { lineIds: optsRef.current.carry(lineId), x: last.x, y: last.y, target: null };
        }
        update();
        if (frame === null) frame = requestAnimationFrame(edgeScroll);
      };
      const finish = (drop: boolean) => {
        const finished = state;
        cleanup();
        if (!finished) return;
        justDroppedRef.current = true;
        setTimeout(() => {
          justDroppedRef.current = false;
        }, 0);
        if (drop && finished.target !== null) optsRef.current.onDrop(finished.lineIds, finished.target);
      };
      const onUp = (ev: PointerEvent) => {
        last = { x: ev.clientX, y: ev.clientY };
        if (state) state = { ...state, target: targetAt(last.x, last.y, state.lineIds) };
        finish(true);
      };
      const onCancel = () => finish(false);
      const onKey = (ev: KeyboardEvent) => {
        if (ev.key !== 'Escape' || !state) return;
        ev.preventDefault();
        ev.stopPropagation();
        finish(false);
      };
      const cleanup = () => {
        document.removeEventListener('pointermove', onMove);
        document.removeEventListener('pointerup', onUp);
        document.removeEventListener('pointercancel', onCancel);
        document.removeEventListener('keydown', onKey, true);
        if (frame !== null) cancelAnimationFrame(frame);
        frame = null;
        state = null;
        cleanupRef.current = null;
        setDrag(null);
      };
      document.addEventListener('pointermove', onMove);
      document.addEventListener('pointerup', onUp);
      document.addEventListener('pointercancel', onCancel);
      document.addEventListener('keydown', onKey, true);
      cleanupRef.current = cleanup;
    },
    [enabled, listRef],
  );

  // A drag in flight when editing becomes blocked (a mode switch) is dropped.
  useEffect(() => {
    if (!enabled) cleanupRef.current?.();
  }, [enabled]);

  const consumeDropClick = useCallback(() => justDroppedRef.current, []);

  return { drag, onPointerDown, consumeDropClick };
}

/** The label that follows the pointer during a drag: what is moving, and
 *  where to. Portalled so no transformed ancestor offsets it. */
export function LineDragGhost({ drag, targetName }: { drag: LineDragState; targetName: string | null }) {
  const { t } = useTranslation();
  const count = drag.lineIds.length;
  const text = targetName === null
    ? t('caveSections.drag.noTarget', { count })
    : t('caveSections.drag.moveTo', { count, name: targetName });
  return createPortal(
    <div
      className={`fixed z-50 pointer-events-none px-2 py-1 rounded shadow-lg text-xs border ${
        targetName === null ? 'bg-panel border-border text-text-secondary' : 'bg-accent text-accent-on border-accent'
      }`}
      style={{ left: drag.x + 14, top: drag.y + 10 }}
      role="status"
      data-testid="sections-drag-ghost"
    >
      {text}
    </div>,
    document.body,
  );
}
