/**
 * A station's link chip row (spanning its shot-only cells): as many whole
 * chips as there is room for, then a "+N" pill listing every link, instead of
 * a chip clipped mid-word.
 *
 * Chips are measured once at their natural width (all of them rendered, each
 * `shrink-0`), per set of chip labels; the row width is tracked with a
 * ResizeObserver, so resizing or hiding columns re-fits without re-measuring.
 * The first chip always shows - truncating inside - so a narrow span never
 * shows a bare "+N".
 */
import { cloneElement, useEffect, useLayoutEffect, useRef, useState, type CSSProperties, type ReactElement } from 'react';
import { useTranslation } from 'react-i18next';
import { Tooltip } from '../common/Tooltip';

/** `gap-1` between chips. */
export const CHIP_GAP = 4;
/** Room the "+N" pill takes (two digits at most in practice). */
export const MORE_PILL_WIDTH = 32;

/**
 * How many leading chips fit in `avail` px: all of them when they fit,
 * otherwise as many as fit beside the "+N" pill, but at least one.
 */
export function fitChips(widths: readonly number[], avail: number, gap = CHIP_GAP, pill = MORE_PILL_WIDTH): number {
  const n = widths.length;
  if (n === 0) return 0;
  const total = widths.reduce((s, w) => s + w, 0) + gap * (n - 1);
  if (total <= avail) return n;
  let used = 0;
  let k = 0;
  for (let i = 0; i < n; i++) {
    const next = used + (i > 0 ? gap : 0) + widths[i]!;
    if (next + gap + pill > avail) break;
    used = next;
    k = i + 1;
  }
  return Math.max(1, k);
}

export interface LinkChip {
  key: string;
  /** The chip's full text, for the "+N" pill's list. */
  label: string;
  node: ReactElement<{ className?: string }>;
}

export function LinkChipRow({ chips, testId, moreTestId }: { chips: readonly LinkChip[]; testId: string; moreTestId: string }) {
  const { t } = useTranslation();
  const rowRef = useRef<HTMLSpanElement>(null);
  const sig = chips.map((c) => c.label).join('\u0000');
  const [measured, setMeasured] = useState<{ sig: string; widths: number[] } | null>(null);
  const [avail, setAvail] = useState(0);
  const widths = measured && measured.sig === sig ? measured.widths : null;

  // Measure while every chip is rendered at its natural width.
  useLayoutEffect(() => {
    const el = rowRef.current;
    if (!el || widths !== null) return;
    setMeasured({ sig, widths: [...el.children].map((c) => (c as HTMLElement).offsetWidth) });
    setAvail(el.clientWidth);
  });
  useEffect(() => {
    const el = rowRef.current;
    if (!el || typeof ResizeObserver === 'undefined') return undefined;
    const ro = new ResizeObserver(() => setAvail(el.clientWidth));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // 0 = not laid out (hidden, or a test DOM): show them all.
  const shown = widths && avail > 0 ? fitChips(widths, avail) : chips.length;
  const hidden = chips.length - shown;
  return (
    <span ref={rowRef} className="flex w-full min-w-0 items-center gap-1 overflow-hidden" data-testid={testId}>
      {chips.slice(0, shown).map((c, i) =>
        // The one chip shown beside a "+N" may be too wide itself: let it shrink
        // (its label truncates) rather than overflow.
        hidden > 0 && i === 0 && shown === 1
          ? cloneElement(c.node, { key: c.key, className: (c.node.props.className ?? '').replace(/\bshrink-0\b/, 'shrink') })
          : cloneElement(c.node, { key: c.key }),
      )}
      {hidden > 0 && (
        <Tooltip
          className="shrink-0"
          content={chips.map((c) => c.label).join('\n')}
          tooltipTestId={`${moreTestId}-list`}
        >
          <span
            className="sh-pill inline-flex cursor-default items-center rounded-full px-2 py-0.5 text-xs leading-none tabular-nums text-text-secondary"
            style={{ '--o': 'var(--cc-text-secondary)' } as CSSProperties}
            aria-label={t('dataWorkspace.links.moreAria', { count: hidden })}
            data-testid={moreTestId}
          >
            {`+${hidden}`}
          </span>
        </Tooltip>
      )}
    </span>
  );
}
