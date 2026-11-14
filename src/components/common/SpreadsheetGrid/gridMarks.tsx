/**
 * Find-match highlighting inside a cell's formatted text ({@link CellMarks}).
 *
 * `splitMarks` is pure (unit-tested); `MarkedText` renders its segments with
 * `<mark class="sg-mark">`, the focused match as `sg-mark sg-mark-current`.
 * Ranges are half-open `[start, end)` UTF-16 offsets into the text the cell
 * displays (`format(getValue())`). Out-of-range, empty, unsorted and
 * overlapping ranges are all tolerated: they are clamped, dropped, sorted and
 * merged, so a host whose index went stale for one render draws a harmless
 * partial highlight instead of throwing.
 */
import type { CellMarks } from './types';

export interface MarkSegment {
  text: string;
  /** 'plain' text, a 'match', or the 'current' (focused) match. */
  kind: 'plain' | 'match' | 'current';
}

type Span = [number, number];

function normalize(range: readonly [number, number], len: number): Span {
  const a = Math.max(0, Math.min(len, Math.floor(range[0])));
  const b = Math.max(0, Math.min(len, Math.floor(range[1])));
  return a <= b ? [a, b] : [b, a];
}

export function splitMarks(text: string, marks: CellMarks): MarkSegment[] {
  const len = text.length;
  const current = marks.current ? normalize(marks.current, len) : null;
  const spans: Span[] = marks.ranges.map((r) => normalize(r, len));
  if (current) spans.push(current);
  const sorted = spans.filter(([a, b]) => b > a).sort((x, y) => x[0] - y[0]);
  const merged: Span[] = [];
  for (const [a, b] of sorted) {
    const last = merged[merged.length - 1];
    if (last && a <= last[1]) last[1] = Math.max(last[1], b);
    else merged.push([a, b]);
  }
  const out: MarkSegment[] = [];
  const push = (from: number, to: number, kind: MarkSegment['kind']) => {
    if (to > from) out.push({ text: text.slice(from, to), kind });
  };
  let pos = 0;
  for (const [a, b] of merged) {
    push(pos, a, 'plain');
    if (current && current[1] > a && current[0] < b && current[1] > current[0]) {
      const c0 = Math.max(a, current[0]);
      const c1 = Math.min(b, current[1]);
      push(a, c0, 'match');
      push(c0, c1, 'current');
      push(c1, b, 'match');
    } else {
      push(a, b, 'match');
    }
    pos = b;
  }
  push(pos, len, 'plain');
  return out;
}

export function MarkedText({ text, marks }: { text: string; marks: CellMarks }) {
  return (
    <>
      {splitMarks(text, marks).map((seg, i) =>
        seg.kind === 'plain' ? (
          seg.text
        ) : (
          <mark
            // Segments are positional and re-derived on every render.
            key={`${i}:${seg.kind}`}
            className={seg.kind === 'current' ? 'sg-mark sg-mark-current' : 'sg-mark'}
          >
            {seg.text}
          </mark>
        ),
      )}
    </>
  );
}

/** Cell classes for a cell holding marks. '' when there is nothing to mark.
 *  A custom-render cell (`custom`) cannot draw <mark>s inside its content, so
 *  it also gets `sg-cell-match-outline`: the whole cell is outlined in the
 *  mark colour instead. */
export function markCellClass(marks: CellMarks | undefined, custom = false): string {
  if (!marks || (marks.ranges.length === 0 && !marks.current)) return '';
  const cls = marks.current ? ' sg-cell-match sg-cell-match-current' : ' sg-cell-match';
  return custom ? `${cls} sg-cell-match-outline` : cls;
}
