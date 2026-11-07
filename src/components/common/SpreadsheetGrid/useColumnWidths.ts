import { useCallback, useEffect, useRef, useState, type MutableRefObject } from 'react';
import { useLatest } from '../../../hooks/useLatest';
import { clampRound } from '../../../utils/clamp';
import { DEFAULT_COL_WIDTH, type GridColumn } from './types';

/**
 * Column-width state and layout for SpreadsheetGrid: the manual overrides, the
 * leftover-width distribution that reads them, and the commit/revert lifecycle
 * that mutates them.
 *
 * This is its own module to keep the width rules next to the constants the Rust side
 * mirrors. It did NOT shrink SpreadsheetGrid.tsx - see ts-file-size-baseline.json for
 * that file's current figure; the extraction bought headroom for future width changes
 * rather than recovering any. Deliberately no LOC numbers quoted here: an earlier
 * revision of this comment claimed a figure the baseline in the same commit
 * contradicted. The ratchet's hard 2000-LOC cap cannot be raised by a baseline bump,
 * and SpreadsheetGrid.tsx is close to it, so further width logic belongs in this file
 * rather than back in the component.
 *
 * The state deliberately lives here rather than being fully controlled by the
 * host: a resize drag fires on every mousemove, so lifting it would re-render the
 * host per pixel and, where the host persists to disk, write per pixel too.
 * Instead the grid owns the live value and hands it over once the drag lands.
 */

/** Smallest width the grid can render legibly. */
export const MIN_COL_WIDTH = 40;

/** Widest column a manual drag will persist. */
export const MAX_COL_WIDTH = 2000;

/** Cap for double-click autofit, which is automatic and so should not be able to
 *  produce a monster column from a single long cell. */
export const AUTOFIT_MAX_COL_WIDTH = 600;

// MIN_COL_WIDTH / MAX_COL_WIDTH are mirrored by MIN_COLUMN_WIDTH /
// MAX_COLUMN_WIDTH in src-tauri/src/config/column_widths.rs, which clamps the same
// range when a persisted width is read back. The mirror is now ENFORCED by
// src/test/__tests__/configBoundsMirror.test.ts, which parses the Rust constants
// and fails on drift - a one-sided bump used to give a silent snap-back (the drag
// commits locally, the backend clamps on save, and the post-save loadConfig feeds
// the clamped value back down with no error anywhere).

export type ColumnWidths = Record<string, number>;

/** Shallow equality, so an unchanged host prop doesn't clobber a drag in
 *  progress (React hands us a fresh object identity on every render). */
function sameWidths(a: ColumnWidths, b: ColumnWidths): boolean {
  const keys = Object.keys(a);
  if (keys.length !== Object.keys(b).length) return false;
  return keys.every((k) => a[k] === b[k]);
}

/**
 * Rendered width per visible column: each column's own width, then any leftover
 * container space split across the `grow` columns so the grid fills its panel.
 *
 * Resizing a `grow` column PINS it: it stops absorbing leftover, so the edge moves
 * in both directions and the grid can end up narrower than its panel, which is
 * ordinary spreadsheet behaviour. The earlier "an override is only the grow FLOOR"
 * rule made a narrowing drag a silent no-op - the width the drag took off came
 * straight back to the same column as leftover, so the handle looked dead at every
 * drag distance while a width the user never saw was still written to config.toml.
 *
 * An override pins, full stop - the rendered width is NOT then capped at the room
 * the panel has. A per-window cap was tried and reverted: because `startResize`
 * seeds the drag from the RENDERED width, capping made a widening drag on a `grow`
 * column freeze at the panel edge while `commitWidths` still persisted the raw
 * dragged value, so the handle went dead and a width the user never saw reached
 * config.toml - the exact pair of symptoms this rule was written to remove, brought
 * back in the widening direction. Overflowing instead is what every other manually
 * sized column already does, and it keeps rendered == persisted.
 *
 * The cost is that a wide drag on a big monitor can leave a horizontal scrollbar
 * on a smaller one. That is bounded by MAX_COL_WIDTH and is recoverable by
 * dragging back, which the frozen-handle behaviour was not.
 */
export function computeColumnWidths<Row>(
  columns: GridColumn<Row>[],
  overrides: ColumnWidths,
  availWidth: number,
  gutterWidth: number,
): number[] {
  const declared = columns.map((c) => c.width ?? DEFAULT_COL_WIDTH);
  // An override is the final width for that column, grow or not. Only columns
  // WITHOUT an override take a share of the leftover below.
  const pinned = columns.map((c, i) => overrides[c.key] ?? declared[i] ?? DEFAULT_COL_WIDTH);
  const growIdx = columns
    .map((c, i) => (c.grow && overrides[c.key] === undefined ? i : -1))
    .filter((i) => i >= 0);
  if (growIdx.length === 0 || availWidth <= 0) return pinned;
  const leftover = availWidth - gutterWidth - pinned.reduce((sum, w) => sum + w, 0);
  if (leftover <= 0) return pinned;
  const add = Math.floor(leftover / growIdx.length);
  const rem = leftover - add * growIdx.length;
  const out = pinned.slice();
  growIdx.forEach((i, n) => {
    out[i] = (out[i] ?? 0) + add + (n < rem ? 1 : 0);
  });
  return out;
}

export interface ColumnWidthsController {
  /** Current overrides, for rendering. */
  widths: ColumnWidths;
  /** Latest overrides without waiting for a re-render - drag handlers build the
   *  next map from this, not from the state value they closed over. */
  widthsRef: MutableRefObject<ColumnWidths>;
  /** Update locally only. For continuous drag feedback. */
  applyWidths: (next: ColumnWidths) => void;
  /** Update and notify the host. For a resize that has landed. */
  commitWidths: (next: ColumnWidths) => void;
  /** Drop every uncommitted change, restoring the last map the host and grid
   *  agreed on. For a drag that was abandoned rather than released. */
  revertWidths: () => void;
}

/**
 * @param declaredKeys Every column this grid declares, HIDDEN ONES INCLUDED.
 *   Widths for anything outside this set are dropped on commit (see
 *   `commitWidths`). Passing only the visible columns would delete a hidden
 *   column's width and lose it the moment the user re-shows the column.
 */
export function useColumnWidths(
  columnWidths: ColumnWidths | undefined,
  onColumnWidthsChange: ((widths: ColumnWidths) => void) | undefined,
  declaredKeys: string[],
): ColumnWidthsController {
  const [widths, setWidths] = useState<ColumnWidths>(() => columnWidths ?? {});
  const widthsRef = useRef<ColumnWidths>(widths);
  // The last map the host and grid agreed on: what we handed the host, or what we
  // last adopted from it. Doubles as the echo filter and the revert target.
  const settledRef = useRef<ColumnWidths>(widths);

  const applyWidths = useCallback((next: ColumnWidths) => {
    widthsRef.current = next;
    setWidths(next);
  }, []);

  // Held in a ref, not a dep: the hosts build `declaredKeys` inline, so it is a
  // fresh array every render and putting it in `commitWidths`'s dep list would
  // give the callback a new identity per render - which the grid passes straight
  // down to the resize handlers. useLatest refreshes the ref on every commit, so
  // a width commit (a resize end) always reads the columns as they are on screen.
  const declaredRef = useLatest(declaredKeys);

  const commitWidths = useCallback(
    (next: ColumnWidths) => {
      // Whole integer pixels only: the persisted config stores u32, and clampRound
      // also absorbs a non-finite width rather than letting NaN reach the backend
      // and be silently rejected.
      //
      // Keys outside the declared set are DROPPED here. This map is spread whole
      // on every commit and the host persists it whole, so without a prune a key
      // that stops naming a real column - renamed, removed, or left behind by an
      // older build - is re-persisted forever and `config.toml` grows
      // monotonically (#357 item 6). The entry-count and key-length ceilings in
      // `config/column_widths.rs` bound the damage but never shrink it, because
      // they cannot tell a stale key from a live one; only the grid knows that.
      //
      // Pruning happens on the next COMMIT, i.e. the next resize in this grid -
      // opening the table does not clean the map, and it should not: a grid whose
      // columns are still being built would read as "every key is an orphan".
      const declared = new Set(declaredRef.current);
      const settled = Object.fromEntries(
        Object.entries(next)
          .filter(([k]) => declared.has(k))
          .map(([k, w]) => [k, clampRound(w, MIN_COL_WIDTH, MAX_COL_WIDTH)]),
      );
      applyWidths(settled);
      // Nothing to tell the host if this is already the map it agreed to. A bare
      // mousedown/mouseup on a resize handle sets the drag flag without moving
      // anything, and a drag returned to where it started ends up here too; both
      // used to spread the whole width map outward for no change.
      if (sameWidths(settled, settledRef.current)) return;
      settledRef.current = settled;
      onColumnWidthsChange?.(settled);
    },
    [applyWidths, onColumnWidthsChange, declaredRef],
  );

  // An abandoned drag has already written its in-progress width into the live map -
  // that is what makes the column follow the pointer - so clearing the drag flag is
  // not enough to undo it. Left in place it would both render a width that was
  // never saved AND ride along on the next commit, which spreads the whole map, so
  // an unrelated resize would persist it. Restore the settled map instead.
  const revertWidths = useCallback(() => {
    if (sameWidths(widthsRef.current, settledRef.current)) return;
    applyWidths(settledRef.current);
  }, [applyWidths]);

  // Adopt host widths that did not originate here, e.g. hydrated from config
  // after mount. Skipping our own echo keeps the grid from fighting the host.
  useEffect(() => {
    const next = columnWidths ?? {};
    if (sameWidths(next, settledRef.current)) return;
    settledRef.current = next;
    applyWidths(next);
  }, [columnWidths, applyWidths]);

  return { widths, widthsRef, applyWidths, commitWidths, revertWidths };
}
