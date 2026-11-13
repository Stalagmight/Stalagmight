/**
 * Labels or icons for the Data toolbar's menu buttons, decided by measuring the
 * one-line toolbar row rather than by a fixed breakpoint: the buttons drop to
 * icon-only (label kept as the accessible name and tooltip) only once the
 * labelled row, with the find box at its narrowest, no longer fits.
 *
 * While labelled, the row's need is measured from its children (the last
 * child's right edge plus the row's end padding), so the find box first shrinks
 * to its minimum; past that the row overflows and goes compact, remembering
 * the width the labelled row needed. While compact, labels come back once the
 * row is that wide again. A content change (a filter count in a label) is seen
 * through the observed children: if the labels come back and no longer fit,
 * the row goes compact again with the new need, so it never oscillates.
 */
import { useLayoutEffect, useRef, useState, type RefObject } from 'react';

/** Width the row's content needs: its last child's right edge + end padding. */
export function labelledNeed(row: HTMLElement): number {
  const last = row.lastElementChild as HTMLElement | null;
  if (!last) return 0;
  const box = row.getBoundingClientRect();
  const padEnd = parseFloat(getComputedStyle(row).paddingRight) || 0;
  let right = 0;
  for (const child of row.children) right = Math.max(right, child.getBoundingClientRect().right);
  return right - box.left + padEnd;
}

/** The next compact state: labelled until the measured need exceeds the room,
 *  compact until the room reaches the remembered need. */
export function nextCompact(compact: boolean, avail: number, need: number): boolean {
  if (avail <= 0) return compact; // not laid out (hidden, a test DOM)
  return need > avail + 0.5;
}

export function useToolbarCompact(rowRef: RefObject<HTMLElement>): boolean {
  const [compact, setCompact] = useState(false);
  const state = useRef({ compact: false, need: 0 });

  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row || typeof ResizeObserver === 'undefined') return undefined;
    const measure = () => {
      const avail = row.getBoundingClientRect().width;
      const s = state.current;
      // Labelled: measure what it needs now. Compact: compare with what the
      // labelled row needed when it last overflowed.
      if (!s.compact) s.need = labelledNeed(row);
      const next = nextCompact(s.compact, avail, s.need);
      if (next === s.compact) return;
      s.compact = next;
      setCompact(next);
    };
    const ro = new ResizeObserver(measure);
    ro.observe(row);
    for (const child of row.children) ro.observe(child);
    measure();
    return () => ro.disconnect();
    // Re-observe after a flip: the children re-render (labels on / off).
  }, [rowRef, compact]);

  return compact;
}
