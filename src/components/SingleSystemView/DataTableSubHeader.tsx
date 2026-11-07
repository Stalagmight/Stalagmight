// The data table's sub-header chrome (#576 D2 split it out of DataTable.tsx at
// this boundary): the `title | centered pill | controls` row, the pill-as-trigger
// picker it hosts and the raw-table name pill. Presentational only - the table
// owns every piece of state these render.
import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react';
import { useUIStore } from '../../store/uiStore';
import { useAnchoredFixedMenu } from '../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { ChevronDownIcon } from '../common/icons';
import { useGrayscalePaint } from '../../hooks/useGrayscalePaint';

/** Pill-as-trigger picker used in the sub-header: the whole pill (rendered via
 *  {@link renderTrigger}, which embeds the chevron) toggles the dropdown.
 *  Closes on outside click and Escape (and declares data-dropdown-open so the
 *  global Escape guard yields to it). The list anchors centered under the pill. */
export function SubHeaderPickerMenu({
  triggerTestId,
  listTestId,
  title,
  options,
  onPick,
  onHoverOption,
  onHoverTrigger,
  renderTrigger,
}: {
  triggerTestId?: string;
  listTestId: string;
  title: string;
  options: { value: string; label: string; disabled?: boolean; dot?: string }[];
  onPick: (value: string) => void;
  onHoverOption?: (value: string | null) => void;
  onHoverTrigger?: (over: boolean) => void;
  renderTrigger: (state: { open: boolean }) => ReactNode;
}) {
  const [open, setOpen] = useState(false);
  const paint = useGrayscalePaint();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const toggle = () => setOpen((v) => !v);
  // Position the list with `position:fixed` so it escapes the Dock's
  // overflow-hidden clip box when it flips up (the data-table sub-header sits
  // low in the window); the hook flips/clamps within the viewport.
  const listStyle = useAnchoredFixedMenu(open, triggerRef, listRef, {
    align: 'center',
    gap: 4,
    width: 208, // px; the hook also sets this as the list width (single source of truth)
    maxHeightCap: 256, // matches the old max-h-64 - stay compact + scroll on a tall window
    // The raw-table submenu feeds async rawTables through `options`: it opens on
    // a single disabled "loading" row, then swaps to N rows once the load
    // resolves. Signal that resize so the hook re-places instead of sticking to
    // the stale on-open placement. The length prefix disambiguates the loading
    // placeholder ([{value:''}], length 1) from an empty result ([], length 0) -
    // both would otherwise join to the same '' key.
    remeasureKey: `${options.length}:${options.map((o) => o.value).join('|')}`,
  });
  // Close on outside mousedown / external scroll / resize (shared hook). The
  // list is position:fixed at a captured rect, so any scroll/resize would
  // otherwise leave it floating at a stale spot. Escape is handled separately
  // below.
  useDismissableLayer({
    open,
    onDismiss: () => setOpen(false),
    contains: [triggerRef, listRef],
  });
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') setOpen(false);
    };
    window.addEventListener('keydown', onKey);
    return () => {
      window.removeEventListener('keydown', onKey);
    };
  }, [open]);
  return (
    // `min-w-0` down the whole trigger chain: the pill lives in a grid track that
    // is allowed to shrink, and every box between that track and the truncating
    // label has to be allowed to shrink with it (#429).
    <div className="relative min-w-0" onMouseDown={(e) => e.stopPropagation()}>
      <button
        ref={triggerRef}
        type="button"
        onClick={toggle}
        onMouseEnter={onHoverTrigger ? () => onHoverTrigger(true) : undefined}
        onMouseLeave={onHoverTrigger ? () => onHoverTrigger(false) : undefined}
        className="flex min-w-0 max-w-full cursor-pointer items-center"
        title={title}
        data-testid={triggerTestId}
      >
        {renderTrigger({ open })}
      </button>
      {open && (
        <div
          ref={listRef}
          data-dropdown-open
          className="overflow-auto rounded border border-border bg-panel p-1 text-xs shadow-lg"
          style={listStyle}
          data-testid={listTestId}
          onMouseLeave={() => onHoverOption?.(null)}
        >
          {options.map((o) => (
            <button
              key={o.value}
              data-testid={`${listTestId}-option-${encodeURIComponent(o.value)}`}
              disabled={o.disabled}
              className="flex w-full items-center gap-1.5 rounded px-2 py-0.5 text-left hover:bg-accent-10 disabled:opacity-50"
              onMouseEnter={() => onHoverOption?.(o.value)}
              onClick={() => {
                onHoverOption?.(null);
                setOpen(false);
                onPick(o.value);
              }}
            >
              {o.dot !== undefined && (
                <span
                  className="h-2 w-2 shrink-0 rounded-full"
                  style={{ backgroundColor: paint(o.dot) }}
                  aria-hidden="true"
                />
              )}
              <span className="truncate">{o.label}</span>
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * The middle cell of {@link SubHeaderRow}: holds the centered pill, and shrinks
 * to nothing before it will push on the controls either side of it.
 *
 * Centering here is a property of the ROW, not of this box - the two side columns
 * are equal `1fr`, so the middle track lands on the header's true midpoint. The
 * previous cut instead spanned the full width with `absolute inset-x-0` and a
 * `pointer-events-none` band, which centered just as well but took no part in the
 * layout: at 1024x640 the pill simply grew over the "Shots" button and its
 * `pointer-events-auto` wrapper swallowed the clicks (#429).
 *
 * Still no CSS `transform` anywhere on this path. A transformed ancestor would
 * make the picker's `position:fixed` list resolve against this box instead of the
 * viewport, re-introducing the Dock-clip that fix escapes (#162).
 */
export function CenteredPillBand({ children }: { children: ReactNode }) {
  return <div className="flex min-w-0 justify-center">{children}</div>;
}

/**
 * The data table's sub-header row: `title | centered pill | controls`.
 *
 * A three-column grid rather than a flex row, because the pill has to center on
 * the full width while the side groups keep their natural size. The outer columns
 * are `1fr` with their default `auto` minimum, so they never shrink below their
 * content; the middle one is `minmax(0, auto)`, so it is the one that gives way
 * and the pill truncates instead of overlapping (#429).
 */
export function SubHeaderRow({ children }: { children: ReactNode }) {
  return (
    <div
      className="relative grid grid-cols-[1fr_minmax(0,auto)_1fr] items-center gap-3 px-3 py-1 border-b border-border shrink-0 text-xs"
      data-testid="data-table-sub-header"
    >
      {children}
    </div>
  );
}

/** The chevron that sits INSIDE the sub-header pill, dim until the menu opens. */
export function PillChevron({ open, testId }: { open: boolean; testId: string }) {
  return (
    <span className={`ml-0.5 ${open ? 'text-text' : 'text-text-secondary'}`} data-testid={testId}>
      <ChevronDownIcon size={12} />
    </span>
  );
}

/** Raw-table name pill: neutral gray sibling of the section EntityBlob, with
 *  the same syntactic-level gate (plain text below 'full'). `trailing` (the
 *  picker chevron) renders inside the pill. */
export function RawTablePill({ name, trailing }: { name: string; trailing?: ReactNode }) {
  const level = useUIStore((s) => s.syntacticLevel);
  if (!syntacticLevelAtLeast(level, 'full')) {
    return (
      <span
        className="inline-flex min-w-0 max-w-full items-center font-medium"
        data-testid="data-table-raw-name"
      >
        <span className="truncate">{name}</span>
        {trailing}
      </span>
    );
  }
  return (
    <span
      className="sh-pill inline-flex min-w-0 max-w-full items-center gap-1 align-middle rounded-full px-2 py-0.5 text-xs leading-none text-text-primary tabular-nums"
      style={{ '--o': 'var(--cc-text-secondary)' } as CSSProperties}
      data-testid="data-table-raw-name"
    >
      <span className="truncate">{name}</span>
      {trailing}
    </span>
  );
}
