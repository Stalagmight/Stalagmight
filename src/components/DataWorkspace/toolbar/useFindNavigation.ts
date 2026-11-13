/**
 * Current-match bookkeeping for the find bar: next / previous, scrolling the
 * grid to a match (`requestFocus`), and keeping the current match on the same
 * text when the matches are recomputed (typing, edits, filters, replace).
 */
import { useCallback, useEffect, useMemo, useRef, type RefObject } from 'react';
import { useDataWorkspaceStore } from '../store';
import { orderColumns } from '../model/search';
import type { DataModel } from '../DataModelContext';
import type { DataColumnKey, DataView, SearchMatch, SearchState } from '../types';
import {
  firstFromRow,
  reconcileCurrent,
  rowPositions,
  spotOf,
  type MatchOrder,
  type MatchSpot,
} from './matchNav';

/** What makes two searches "the same pattern" (scope changes keep the current match). */
export function patternKey(s: Pick<SearchState, 'query' | 'matchCase' | 'wholeWord' | 'regex' | 'open'>): string {
  return `${s.open ? 1 : 0}${s.matchCase ? 1 : 0}${s.wholeWord ? 1 : 0}${s.regex ? 1 : 0}:${s.query}`;
}

/** A stale replace anchor (the edit changed nothing on screen) is dropped after this. */
const ANCHOR_TTL_MS = 2000;

export interface FindNavigation {
  next: () => void;
  prev: () => void;
  /** Scroll the grid to a match without changing the current index. */
  reveal: (m: SearchMatch) => void;
  /** After a replace: once the matches update, select the match at or after this spot. */
  setAnchor: (spot: MatchSpot | null) => void;
}

export function useFindNavigation(
  view: DataView,
  model: DataModel,
  /** Inputs that keep focus while the grid scrolls to a match (find, replace). */
  keepFocus: readonly RefObject<HTMLElement | null>[],
): FindNavigation {
  const columnOrder = useDataWorkspaceStore((s) => s.columnOrder);
  const viewRef = useRef(view);
  viewRef.current = view;
  const keepRef = useRef(keepFocus);
  keepRef.current = keepFocus;

  const colPos = useMemo(() => {
    const map = new Map<DataColumnKey, number>();
    orderColumns(model.columns, columnOrder).forEach((c, i) => map.set(c.key, i));
    return map;
  }, [model.columns, columnOrder]);

  const order = useMemo<MatchOrder>(
    () => ({
      rowPos: (key) => rowPositions(viewRef.current.rows).get(key),
      colPos: (key) => colPos.get(key) ?? Number.MAX_SAFE_INTEGER,
      surveyOrder: (key) => model.rowByKey.get(key)?.order,
    }),
    [colPos, model.rowByKey],
  );

  const reveal = useCallback((m: SearchMatch) => {
    const focused = keepRef.current.map((r) => r.current).find((el) => el !== null && el === document.activeElement);
    useDataWorkspaceStore.getState().requestFocus({ rowKey: m.rowKey, colKey: m.colKey, align: 'center' });
    if (!focused) return;
    // The grid may take DOM focus when it activates the cell; typing in the
    // find box must not be interrupted, so hand focus back - unless the user
    // has meanwhile moved within the toolbar (Ctrl+H to the replace box).
    const toolbar = focused.closest('[role="toolbar"]');
    requestAnimationFrame(() =>
      requestAnimationFrame(() => {
        const now = document.activeElement;
        if (now === focused || !focused.isConnected) return;
        if (toolbar && now && toolbar.contains(now)) return;
        focused.focus({ preventScroll: true });
      }),
    );
  }, []);

  const lastRef = useRef<{ matches: readonly SearchMatch[]; pattern: string }>({ matches: [], pattern: '' });
  const anchorRef = useRef<{ spot: MatchSpot; at: number } | null>(null);

  useEffect(() => {
    const st = useDataWorkspaceStore.getState();
    const last = lastRef.current;
    if (last.matches === view.matches) return;
    const pattern = patternKey(st.search);
    const prevMatch = last.matches[st.currentMatch];
    const anchor =
      anchorRef.current && Date.now() - anchorRef.current.at < ANCHOR_TTL_MS ? anchorRef.current.spot : null;
    anchorRef.current = null;
    const patternChanged = pattern !== last.pattern;
    const idx = reconcileCurrent({
      matches: view.matches,
      previous: prevMatch ? spotOf(prevMatch) : null,
      anchor,
      patternChanged,
      activeRowKey: st.activeRowKey,
      order,
    });
    lastRef.current = { matches: view.matches, pattern };
    if (idx !== st.currentMatch) st.setCurrentMatch(idx);
    const target = view.matches[idx];
    if (target && (patternChanged || anchor)) reveal(target);
  }, [view.matches, order, reveal]);

  const go = useCallback(
    (delta: 1 | -1) => {
      const matches = viewRef.current.matches;
      const n = matches.length;
      if (n === 0) return;
      const st = useDataWorkspaceStore.getState();
      let idx: number;
      if (st.currentMatch < 0 || st.currentMatch >= n) {
        const from = firstFromRow(matches, st.activeRowKey, order);
        idx = delta > 0 ? from : (from - 1 + n) % n;
      } else {
        idx = (st.currentMatch + delta + n) % n;
      }
      st.setCurrentMatch(idx);
      reveal(matches[idx]!);
    },
    [order, reveal],
  );

  const next = useCallback(() => go(1), [go]);
  const prev = useCallback(() => go(-1), [go]);
  const setAnchor = useCallback((spot: MatchSpot | null) => {
    anchorRef.current = spot ? { spot, at: Date.now() } : null;
  }, []);

  return useMemo(() => ({ next, prev, reveal, setAnchor }), [next, prev, reveal, setAnchor]);
}
