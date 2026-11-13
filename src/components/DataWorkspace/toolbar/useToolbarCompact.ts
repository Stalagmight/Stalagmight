/**
 * Labels or icons for the Data toolbar's menu buttons, decided by measuring the
 * one-line toolbar row rather than by a fixed breakpoint: the buttons drop to
 * icon-only (label kept as the accessible name and tooltip) only once the
 * labelled row, with the find box at its narrowest, no longer fits.
 *
 * The row's need is the sum of its children's widths, the shrinkable find
 * column (`data-toolbar-shrink`) counted at its narrowest, plus the gaps and
 * the padding. When the labelled row overflows, the buttons go compact and the
 * width the labels take is learnt (labelled need - compact need, measured
 * across the flip); labels come back once the compact need plus that fits.
 * Every child is observed, and so are the find column's inputs (the column
 * itself keeps its width while its content changes), so a change of content
 * - the find box's match count, a filter count in a label - is re-measured
 * too, in either state.
 */
import { useLayoutEffect, useRef, useState, type RefObject } from 'react';

/** The text inputs of the shrinkable column: the only parts of it that grow. */
const shrinkInputs = (row: HTMLElement) => row.querySelectorAll<HTMLInputElement>('[data-toolbar-shrink] input[type="text"]');

/** The narrowest the shrinkable column can get: its width less the room its
 *  tightest text input has above that input's own minimum width. */
function shrinkableMin(col: HTMLElement): number {
  const width = col.getBoundingClientRect().width;
  let slack = Infinity;
  for (const input of col.querySelectorAll<HTMLInputElement>('input[type="text"]')) {
    const min = parseFloat(getComputedStyle(input).minWidth) || 0;
    slack = Math.min(slack, input.getBoundingClientRect().width - min);
  }
  return Number.isFinite(slack) ? width - Math.max(0, slack) : width;
}

/** Width the row's content needs at the least: children (the shrinkable one
 *  at its narrowest) + gaps + padding. Margins (the Map toggle's `ml-auto`)
 *  are spare room, not need. */
export function rowNeed(row: HTMLElement): number {
  const cs = getComputedStyle(row);
  const kids = [...row.children] as HTMLElement[];
  let need =
    (parseFloat(cs.paddingLeft) || 0) +
    (parseFloat(cs.paddingRight) || 0) +
    (parseFloat(cs.columnGap) || 0) * Math.max(0, kids.length - 1);
  for (const k of kids) need += k.dataset.toolbarShrink !== undefined ? shrinkableMin(k) : k.getBoundingClientRect().width;
  return need;
}

/** Sub-pixel slack, so a row that fits exactly does not flip. */
const EPS = 0.5;

export function useToolbarCompact(rowRef: RefObject<HTMLElement>): boolean {
  const [compact, setCompact] = useState(false);
  const state = useRef({ compact: false, labelExtra: 0, labelledNeed: null as number | null });

  useLayoutEffect(() => {
    const row = rowRef.current;
    if (!row || typeof ResizeObserver === 'undefined') return undefined;
    const measure = () => {
      const avail = row.getBoundingClientRect().width;
      if (avail <= 0) return; // not laid out (hidden, a test DOM)
      const s = state.current;
      const need = rowNeed(row);
      if (!s.compact) {
        if (need <= avail + EPS) return;
        s.compact = true;
        s.labelledNeed = need;
        setCompact(true);
        return;
      }
      // First measure after going compact: learn what the labels take.
      if (s.labelledNeed !== null) {
        s.labelExtra = Math.max(0, s.labelledNeed - need);
        s.labelledNeed = null;
      }
      if (need + s.labelExtra <= avail + EPS) {
        s.compact = false;
        setCompact(false);
      }
    };
    const ro = new ResizeObserver(() => {
      // Inputs come and go (the replace row): observing again is a no-op.
      for (const input of shrinkInputs(row)) ro.observe(input);
      measure();
    });
    ro.observe(row);
    for (const child of row.children) ro.observe(child);
    for (const input of shrinkInputs(row)) ro.observe(input);
    measure();
    return () => ro.disconnect();
    // Re-observe and re-measure after a flip: the children re-rendered.
  }, [rowRef, compact]);

  return compact;
}
