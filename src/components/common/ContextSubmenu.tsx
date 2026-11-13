import { useLayoutEffect, useRef, useState } from 'react';
import { MenuRow } from './MenuRow';
import { ChevronRightIcon } from './icons';
import { setSource } from '../../utils/telemetry';

/** A context-menu row that can open a nested flyout (`submenu`). */
export interface SubmenuItem {
  label: string;
  action: () => void;
  disabled?: boolean;
  hint?: string | undefined;
  submenu?: SubmenuItem[];
}

/** The row a flyout hangs off: its index in its own list and its rect. */
export interface SubmenuAnchor {
  index: number;
  left: number;
  right: number;
  top: number;
}

/** The anchor for `row`, the `index`-th row of its list. */
export function anchorOf(row: HTMLElement, index: number): SubmenuAnchor {
  const r = row.getBoundingClientRect();
  return { index, left: r.left, right: r.right, top: r.top };
}

const MAX_WIDTH_PX = 288;

/**
 * One level of a context menu's flyouts. `level` is 1 for the flyout of a
 * top-level row, 2 for a flyout opened from a level-1 row. `path` is the open
 * flyout chain (one anchor per level) and `setPath` replaces it: hovering a
 * row with a submenu opens it (and closes anything deeper), hovering a plain
 * row closes what is deeper than this level, and a click on a submenu row
 * toggles it.
 *
 * Placed beside its row on the right, flipped left when it would leave the
 * window, and lifted so its bottom stays on screen (it scrolls past the
 * window's height). Measured hidden first, then placed.
 */
export function SubmenuFlyout({
  level,
  anchor,
  items,
  path,
  setPath,
}: {
  level: number;
  anchor: SubmenuAnchor;
  items: SubmenuItem[];
  path: SubmenuAnchor[];
  setPath: (path: SubmenuAnchor[]) => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number; maxHeight: number } | null>(null);
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    let left = anchor.right - 2;
    if (left + width > window.innerWidth - 4) left = Math.max(4, anchor.left - width + 2);
    const maxHeight = window.innerHeight - 8;
    const top = Math.max(4, Math.min(anchor.top - 4, window.innerHeight - 4 - Math.min(height, maxHeight)));
    setPos({ left, top, maxHeight });
  }, [anchor.left, anchor.right, anchor.top]);

  const id = (base: string) => (level === 1 ? base : `${base}-${level}`);
  return (
    <div
      ref={ref}
      role="menu"
      className="fixed bg-panel border border-border rounded-lg shadow-xl py-1 min-w-[180px] overflow-y-auto"
      data-testid={id('context-menu-submenu')}
      style={
        pos
          ? { left: pos.left, top: pos.top, maxHeight: pos.maxHeight, maxWidth: MAX_WIDTH_PX }
          : { left: anchor.right, top: anchor.top, maxWidth: MAX_WIDTH_PX, visibility: 'hidden' }
      }
    >
      {items.map((item, index) => {
        const hasSubmenu = item.submenu !== undefined;
        const expanded = path[level]?.index === index;
        return (
          <MenuRow
            key={index}
            data-testid={`${id('context-menu-subitem')}-${index}`}
            onClick={(e) => {
              if (item.disabled) return;
              if (hasSubmenu) {
                setPath(expanded ? path.slice(0, level) : [...path.slice(0, level), anchorOf(e.currentTarget, index)]);
                return;
              }
              setSource('context-menu');
              item.action();
            }}
            onMouseEnter={(e) => {
              if (hasSubmenu && !item.disabled) {
                if (!expanded) setPath([...path.slice(0, level), anchorOf(e.currentTarget, index)]);
              } else if (path.length > level) {
                setPath(path.slice(0, level));
              }
            }}
            aria-haspopup={hasSubmenu ? 'menu' : undefined}
            aria-expanded={hasSubmenu ? expanded : undefined}
            disabled={item.disabled}
            title={item.hint}
            hint={item.hint}
            label={item.label}
            showIcons={false}
            className={`${item.disabled ? 'text-text-secondary cursor-not-allowed' : 'hover:bg-accent-10'} ${
              expanded ? 'bg-accent-10' : ''
            }`}
          >
            {hasSubmenu && <ChevronRightIcon size={14} className="text-text-secondary flex-shrink-0" />}
          </MenuRow>
        );
      })}
    </div>
  );
}
