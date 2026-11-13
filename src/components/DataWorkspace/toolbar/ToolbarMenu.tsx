/**
 * The Data workspace toolbar's dropdown: a flat toolbar button and a
 * trigger-anchored menu, built on the same pieces as ShotColumnsMenu and the
 * data table's sub-header pickers (`useAnchoredFixedMenu` placement, the
 * `useDismissableLayer` outside-click / scroll / resize dismissal, Escape,
 * `data-dropdown-open` so the global Escape handler yields).
 */
import { useEffect, useRef, useState, type ReactNode } from 'react';
import { useAnchoredFixedMenu } from '../../../hooks/useAnchoredFixedMenu';
import { useDismissableLayer } from '../../../hooks/useDismissableLayer';
import { ChevronDownIcon } from '../../common/icons';

/** Classes for a flat toolbar button: open (accent fill), active (it narrows
 *  the view right now) or idle. */
export function toolbarButtonClass(state: { open?: boolean; active?: boolean; disabled?: boolean }): string {
  const base =
    'inline-flex shrink-0 items-center gap-1 whitespace-nowrap rounded px-2 py-1 text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-50';
  if (state.open) return `${base} bg-accent text-accent-on`;
  if (state.active) return `${base} bg-accent-20 text-text-primary hover:bg-accent-30`;
  return `${base} text-text-primary hover:bg-accent-10`;
}

/** A small square icon button (inside the find box): on = accent wash. */
export function iconButtonClass(on: boolean): string {
  const base =
    'inline-flex h-6 min-w-6 shrink-0 items-center justify-center rounded px-1 text-xs transition-colors disabled:cursor-not-allowed disabled:opacity-40';
  return on ? `${base} bg-accent-20 text-text-primary` : `${base} text-text-secondary hover:bg-accent-10 hover:text-text-primary`;
}

/** The menu surface (same as ShotColumnsMenu's dropdown). */
export const MENU_SURFACE_CLASS = 'overflow-auto rounded border border-border bg-panel p-1 text-xs shadow-lg';

/** A small uppercase heading inside a menu (DisplayMenu's category style). */
export function MenuHeading({ children }: { children: ReactNode }) {
  return (
    <div className="px-2 pb-0.5 pt-1.5 text-[11px] font-medium uppercase tracking-wide text-text-secondary">
      {children}
    </div>
  );
}

export function MenuDivider() {
  return <div className="my-1 border-t border-border" role="separator" />;
}

interface ToolbarMenuProps {
  testId: string;
  menuTestId: string;
  label: ReactNode;
  icon?: ReactNode;
  title: string;
  /** This menu narrows / changes the view right now (tints the trigger). */
  active?: boolean;
  width: number;
  align?: 'right' | 'center';
  maxHeightCap?: number;
  /** Re-place the menu when its content height changes while open. */
  remeasureKey?: string | number;
  /** Accessible name when the label is visually short. */
  ariaLabel?: string;
  /** Render the trigger as a small icon button (no label, no chevron). */
  iconOnly?: boolean;
  children: ReactNode | ((close: () => void) => ReactNode);
}

export function ToolbarMenu({
  testId,
  menuTestId,
  label,
  icon,
  title,
  active = false,
  width,
  align = 'right',
  maxHeightCap = 560,
  remeasureKey,
  ariaLabel,
  iconOnly = false,
  children,
}: ToolbarMenuProps) {
  const [open, setOpen] = useState(false);
  const btnRef = useRef<HTMLButtonElement | null>(null);
  const menuRef = useRef<HTMLDivElement | null>(null);
  const close = () => setOpen(false);
  const style = useAnchoredFixedMenu(open, btnRef, menuRef, {
    align,
    gap: 4,
    width,
    maxHeightCap,
    ...(remeasureKey !== undefined ? { remeasureKey } : {}),
  });
  useDismissableLayer({
    open,
    onDismiss: close,
    contains: [btnRef, menuRef],
    // Anchored to a toolbar button that never scrolls: a grid scroll leaves it right.
    survivesUnrelatedScroll: true,
  });
  useEffect(() => {
    if (!open) return undefined;
    const onKey = (e: KeyboardEvent) => {
      // A nested Select closes itself first (it prevents the default).
      if (e.key !== 'Escape' || e.defaultPrevented) return;
      e.preventDefault();
      setOpen(false);
      btnRef.current?.focus();
    };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [open]);

  return (
    <div className="relative" onMouseDown={(e) => e.stopPropagation()}>
      <button
        ref={btnRef}
        type="button"
        className={iconOnly ? iconButtonClass(open || active) : toolbarButtonClass({ open, active })}
        onClick={() => setOpen((v) => !v)}
        title={title}
        aria-label={ariaLabel}
        aria-haspopup="true"
        aria-expanded={open}
        data-active={active ? 'true' : undefined}
        data-testid={testId}
      >
        {icon}
        {!iconOnly && label}
        {!iconOnly && <ChevronDownIcon size={10} />}
      </button>
      {open && (
        <div ref={menuRef} className={MENU_SURFACE_CLASS} style={style} data-dropdown-open data-testid={menuTestId}>
          {typeof children === 'function' ? children(close) : children}
        </div>
      )}
    </div>
  );
}
