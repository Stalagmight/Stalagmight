/**
 * AppMenu - a portaled "hamburger" dropdown menu for the global view header.
 *
 * Modeled on Select.tsx (portal to document.body with position:fixed, flip /
 * clamp positioning, click-outside + scroll/resize close) and the row styling
 * of ContextMenu.tsx (icon gating at the 'icons' syntactic level). Supports one
 * level of submenu via a second portaled flyout, right-aligned keyboard-shortcut
 * hints, and full keyboard navigation.
 */
import {
  useState,
  useRef,
  useEffect,
  useLayoutEffect,
  useCallback,
  useId,
  type ReactNode,
  type CSSProperties,
} from 'react';
import { createPortal } from 'react-dom';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { syntacticLevelAtLeast } from '../../theme/syntacticPalette';
import { formatShortcut, isMac } from '../../utils/shortcuts';
import { IconButton } from './Button';
import { MenuRow, MenuSeparator } from './MenuRow';
import { MenuIcon, ChevronRightIcon, CheckIcon } from './icons';

export interface AppMenuItem {
  /** Stable id; doubles as the data-testid suffix for the row. */
  id: string;
  label: string;
  /** Leading icon, rendered only at syntactic level >= 'icons'. */
  icon?: ReactNode;
  /** Fixed icon tint (CSS color). Applied via inline color on the icon wrapper. */
  iconColor?: string;
  /** Give the row a dark-red background (e.g. the Save row after a save failed). */
  dangerBg?: boolean;
  /** Shortcut action key (e.g. 'new_file'); its first binding is shown right-aligned. */
  action?: string;
  onClick?: () => void;
  disabled?: boolean;
  /** Native tooltip on the row (e.g. why it is disabled). */
  title?: string;
  separator?: boolean;
  /** When set, renders an on/off check on the right (a toggle item). */
  checked?: boolean;
  /** One level of nested items. Mutually exclusive with onClick. */
  submenu?: AppMenuItem[];
}

interface AppMenuProps {
  items: AppMenuItem[];
  /** Tint the hamburger trigger icon red (e.g. when a save has failed). */
  triggerDanger?: boolean;
  /** Disable the trigger so the menu can't be opened (e.g. during placement mode):
   *  the file/project actions that moved into this menu were each disabled on the
   *  old toolbar, so the menu must be unavailable too - otherwise Close Project /
   *  New / Open / Save As stay clickable mid-placement. */
  disabled?: boolean;
  /** Shrink the trigger button (tighter padding) when the toolbar is running out
   *  of horizontal room - the final condensation stage. */
  compact?: boolean;
  'aria-label'?: string;
  'data-testid'?: string;
  /** Test id for the menu container. Each row's own testid is its `item.id`
   *  verbatim (set by renderRow), keeping the legacy/e2e selectors stable. */
  menuTestId?: string;
}

interface Rect {
  left: number;
  right: number;
  top: number;
  bottom: number;
  width: number;
}

interface Pos {
  top: number;
  left: number;
  /** Cap so a tall menu fits the viewport and scrolls instead of overflowing. */
  maxHeight: number;
}

/** Gap (px) kept between the menu and the viewport edges / the trigger. */
const VIEWPORT_MARGIN = 8;
const TRIGGER_GAP = 2;

/** Rows a roving-focus arrow key may land on: separators and disabled rows are
 *  skipped. Exported because SpeleoStatusMenu renders the same `AppMenuItem[]`
 *  through the same keyboard model. */
export function focusableIds(items: AppMenuItem[]): string[] {
  return items.filter((i) => !i.separator && !i.disabled).map((i) => i.id);
}

/** Move the roving focus one step, clamping at the ends (shared with SpeleoStatusMenu). */
export function stepId(ids: string[], current: string | null, dir: 1 | -1): string | null {
  if (ids.length === 0) return null;
  if (current === null) return dir === 1 ? (ids[0] ?? null) : (ids[ids.length - 1] ?? null);
  const idx = ids.indexOf(current);
  if (idx === -1) return ids[0] ?? null;
  const next = idx + dir;
  if (next < 0 || next >= ids.length) return current; // clamp at ends
  return ids[next] ?? null;
}

export function AppMenu({
  items,
  triggerDanger = false,
  disabled = false,
  compact = false,
  'aria-label': ariaLabel = 'Menu',
  'data-testid': triggerTestId = 'app-menu-trigger',
  menuTestId = 'app-menu',
}: AppMenuProps) {
  const syntacticLevel = useUIStore((s) => s.syntacticLevel);
  const shortcuts = useProjectStore((s) => s.config?.shortcuts) as
    | Record<string, string[]>
    | undefined;
  const showIcons = syntacticLevelAtLeast(syntacticLevel, 'icons');
  const mac = isMac();

  const [isOpen, setIsOpen] = useState(false);
  const [triggerRect, setTriggerRect] = useState<Rect | null>(null);
  const [menuPos, setMenuPos] = useState<Pos | null>(null);
  const [activeId, setActiveId] = useState<string | null>(null);

  const [openSubmenuId, setOpenSubmenuId] = useState<string | null>(null);
  const [submenuParentRect, setSubmenuParentRect] = useState<Rect | null>(null);
  const [submenuPos, setSubmenuPos] = useState<Pos | null>(null);
  const [submenuActiveId, setSubmenuActiveId] = useState<string | null>(null);

  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const submenuRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef<Record<string, HTMLButtonElement | null>>({});
  const submenuItemRefs = useRef<Record<string, HTMLButtonElement | null>>({});

  const baseId = useId();
  const menuId = `${baseId}-menu`;
  const submenuDomId = `${baseId}-submenu`;

  const bindingFor = useCallback(
    (action?: string): string | null => {
      if (!action || !shortcuts) return null;
      const b = shortcuts[action];
      return Array.isArray(b) && b[0] ? b[0] : null;
    },
    [shortcuts],
  );

  const openMenu = useCallback(() => {
    const el = triggerRef.current;
    if (el) {
      const r = el.getBoundingClientRect();
      setTriggerRect({ left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width });
    }
    setIsOpen(true);
    setActiveId(focusableIds(items)[0] ?? null);
  }, [items]);

  const closeMenu = useCallback((focusTrigger = false) => {
    setIsOpen(false);
    setOpenSubmenuId(null);
    setSubmenuParentRect(null);
    setSubmenuPos(null);
    setSubmenuActiveId(null);
    setMenuPos(null);
    if (focusTrigger) triggerRef.current?.focus();
  }, []);

  const openSubmenu = useCallback((item: AppMenuItem) => {
    if (item.disabled || !item.submenu) return;
    const el = itemRefs.current[item.id];
    if (el) {
      const r = el.getBoundingClientRect();
      setSubmenuParentRect({ left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width });
    }
    setOpenSubmenuId(item.id);
    setSubmenuActiveId(focusableIds(item.submenu)[0] ?? null);
  }, []);

  const closeSubmenu = useCallback(() => {
    setOpenSubmenuId(null);
    setSubmenuParentRect(null);
    setSubmenuPos(null);
    setSubmenuActiveId(null);
  }, []);

  const activateLeaf = useCallback(
    (item: AppMenuItem) => {
      if (item.disabled) return;
      // Checkable rows (a toggle, e.g. "Dev Stats") stay open so the user can see
      // the check flip and toggle repeatedly; plain action rows close the menu and
      // return focus to the trigger (WAI-ARIA menu-button pattern), so keyboard
      // activation doesn't drop focus to <body>. An onClick that opens a dialog
      // runs after and moves focus into the dialog on mount.
      if (item.checked === undefined) closeMenu(true);
      item.onClick?.();
    },
    [closeMenu],
  );

  // Position the top-level menu under the trigger, flipping up when there's more
  // room above. Always cap the height to the chosen side's available space so a
  // menu taller than the viewport scrolls (maxHeight + overflow) instead of
  // overflowing off-screen. Measured before maxHeight is applied (the hidden
  // fallback style has none), so `height` is the natural content height.
  useLayoutEffect(() => {
    if (!isOpen || !triggerRect) {
      setMenuPos(null);
      return;
    }
    const el = menuRef.current;
    if (!el) return;
    const height = el.getBoundingClientRect().height;
    const spaceBelow = window.innerHeight - triggerRect.bottom - TRIGGER_GAP - VIEWPORT_MARGIN;
    const spaceAbove = triggerRect.top - TRIGGER_GAP - VIEWPORT_MARGIN;
    // Open downward if it fits there, or if below has at least as much room.
    const openDown = height <= spaceBelow || spaceBelow >= spaceAbove;
    let top: number;
    let maxHeight: number;
    if (openDown) {
      top = triggerRect.bottom + TRIGGER_GAP;
      maxHeight = spaceBelow;
    } else {
      maxHeight = spaceAbove;
      top = triggerRect.top - TRIGGER_GAP - Math.min(height, maxHeight);
    }
    setMenuPos({ top: Math.max(VIEWPORT_MARGIN, top), left: triggerRect.left, maxHeight });
    // `items` is a dependency so that if the menu's content height changes while
    // it stays open (a future variable-height row), the side/maxHeight cap is
    // recomputed against the new height instead of staying stale. Both callers
    // memoize their items array, so this doesn't re-run on unrelated renders.
  }, [isOpen, triggerRect, items]);

  // Position the submenu flyout to the right of its parent row (flip left / clamp),
  // also capping its height to the viewport so a long submenu scrolls.
  useLayoutEffect(() => {
    if (!openSubmenuId || !submenuParentRect) {
      setSubmenuPos(null);
      return;
    }
    const el = submenuRef.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    let left = submenuParentRect.right - 2;
    if (left + rect.width > window.innerWidth) {
      left = Math.max(0, submenuParentRect.left - rect.width + 2);
    }
    const maxHeight = window.innerHeight - VIEWPORT_MARGIN * 2;
    const shownHeight = Math.min(rect.height, maxHeight);
    let top = submenuParentRect.top;
    if (top + shownHeight > window.innerHeight - VIEWPORT_MARGIN) {
      top = Math.max(VIEWPORT_MARGIN, window.innerHeight - VIEWPORT_MARGIN - shownHeight);
    }
    setSubmenuPos({ top, left, maxHeight });
  }, [openSubmenuId, submenuParentRect]);

  // Focus the active row (submenu takes precedence when open).
  useEffect(() => {
    if (!isOpen) return;
    if (openSubmenuId) {
      if (submenuActiveId) submenuItemRefs.current[submenuActiveId]?.focus();
    } else if (activeId) {
      itemRefs.current[activeId]?.focus();
    }
  }, [isOpen, openSubmenuId, activeId, submenuActiveId]);

  // Close on outside mousedown / external scroll / resize (shared with Select), so
  // the portaled position never goes stale. The menu + submenu are scroll-safe.
  useDismissableLayer({
    open: isOpen,
    onDismiss: closeMenu,
    contains: [triggerRef, menuRef, submenuRef],
    scrollSafe: [menuRef, submenuRef],
  });

  // Close if the trigger becomes disabled while the menu is open. The trigger's
  // own `disabled` guard can't cover an already-open portaled menu, and the
  // disable can fire programmatically (e.g. placement mode starting on paste /
  // merge / import-completion), which would otherwise defeat the guard.
  useEffect(() => {
    if (disabled && isOpen) closeMenu();
  }, [disabled, isOpen, closeMenu]);

  const onTriggerKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (disabled) return;
      if (e.key === 'Enter' || e.key === ' ') {
        // Toggle, mirroring the trigger's click handler - so a keyboard user who
        // tabs back to the trigger while the menu is open can close it too, not
        // only open it.
        e.preventDefault();
        if (isOpen) closeMenu(true);
        else openMenu();
      } else if (e.key === 'ArrowDown') {
        e.preventDefault();
        if (isOpen) setActiveId(focusableIds(items)[0] ?? null);
        else openMenu();
      } else if (e.key === 'ArrowUp') {
        e.preventDefault();
        if (isOpen) {
          setActiveId(focusableIds(items).slice(-1)[0] ?? null);
        } else {
          openMenu();
          setActiveId(focusableIds(items).slice(-1)[0] ?? null);
        }
      }
    },
    [disabled, isOpen, openMenu, closeMenu, items],
  );

  const onMenuKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      const ids = focusableIds(items);
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setActiveId((cur) => stepId(ids, cur, 1));
          break;
        case 'ArrowUp':
          e.preventDefault();
          setActiveId((cur) => stepId(ids, cur, -1));
          break;
        case 'Home':
          e.preventDefault();
          setActiveId(ids[0] ?? null);
          break;
        case 'End':
          e.preventDefault();
          setActiveId(ids[ids.length - 1] ?? null);
          break;
        case 'ArrowRight': {
          const item = items.find((i) => i.id === activeId);
          if (item?.submenu) {
            e.preventDefault();
            openSubmenu(item);
          }
          break;
        }
        case 'Enter':
        case ' ': {
          e.preventDefault();
          const item = items.find((i) => i.id === activeId);
          if (!item) break;
          if (item.submenu) openSubmenu(item);
          else activateLeaf(item);
          break;
        }
        case 'Escape':
          e.preventDefault();
          closeMenu(true);
          break;
        case 'Tab':
          e.preventDefault();
          closeMenu(true);
          break;
      }
    },
    [items, activeId, openSubmenu, activateLeaf, closeMenu],
  );

  const onSubmenuKeyDown = useCallback(
    (e: React.KeyboardEvent, submenu: AppMenuItem[]) => {
      const ids = focusableIds(submenu);
      switch (e.key) {
        case 'ArrowDown':
          e.preventDefault();
          setSubmenuActiveId((cur) => stepId(ids, cur, 1));
          break;
        case 'ArrowUp':
          e.preventDefault();
          setSubmenuActiveId((cur) => stepId(ids, cur, -1));
          break;
        case 'Home':
          e.preventDefault();
          setSubmenuActiveId(ids[0] ?? null);
          break;
        case 'End':
          e.preventDefault();
          setSubmenuActiveId(ids[ids.length - 1] ?? null);
          break;
        case 'ArrowLeft':
        case 'Escape':
          e.preventDefault();
          closeSubmenu();
          break;
        case 'Enter':
        case ' ': {
          e.preventDefault();
          const item = submenu.find((i) => i.id === submenuActiveId);
          if (item) activateLeaf(item);
          break;
        }
        case 'Tab':
          e.preventDefault();
          closeMenu(true);
          break;
      }
    },
    [submenuActiveId, closeSubmenu, activateLeaf, closeMenu],
  );

  const renderRow = (
    item: AppMenuItem,
    opts: { isSubmenu: boolean },
  ): ReactNode => {
    if (item.separator) {
      return <MenuSeparator key={item.id} role="separator" />;
    }
    const binding = bindingFor(item.action);
    const hasSubmenu = !!item.submenu;
    const refMap = opts.isSubmenu ? submenuItemRefs : itemRefs;
    const isActive = opts.isSubmenu ? submenuActiveId === item.id : activeId === item.id;
    const variantClass = `${item.disabled ? 'text-text-secondary cursor-not-allowed' : ''} ${
      item.dangerBg
        ? 'bg-red-900/60 hover:bg-red-900/80'
        : item.disabled
          ? ''
          : `hover:bg-accent-10 ${isActive ? 'bg-accent-10' : ''}`
    }`;

    return (
      <MenuRow
        key={item.id}
        ref={(el) => {
          refMap.current[item.id] = el;
        }}
        type="button"
        role="menuitem"
        tabIndex={isActive ? 0 : -1}
        data-testid={item.id}
        disabled={item.disabled}
        title={item.title}
        aria-haspopup={hasSubmenu ? 'menu' : undefined}
        aria-expanded={hasSubmenu ? openSubmenuId === item.id : undefined}
        aria-disabled={item.disabled || undefined}
        onClick={() => {
          if (item.disabled) return;
          if (hasSubmenu) {
            if (openSubmenuId === item.id) closeSubmenu();
            else openSubmenu(item);
          } else {
            activateLeaf(item);
          }
        }}
        onMouseEnter={() => {
          if (item.disabled) return;
          if (opts.isSubmenu) {
            setSubmenuActiveId(item.id);
            return;
          }
          setActiveId(item.id);
          // Only (re)open when a *different* submenu is the target; re-opening an
          // already-open submenu would reset submenuActiveId to its first row and
          // discard the user's keyboard-highlighted position.
          if (hasSubmenu) {
            if (openSubmenuId !== item.id) openSubmenu(item);
          } else if (openSubmenuId) closeSubmenu();
        }}
        label={item.label}
        icon={item.icon}
        showIcons={showIcons}
        iconStyle={item.iconColor ? { color: item.iconColor } : undefined}
        className={variantClass}
      >
        {hasSubmenu ? (
          <ChevronRightIcon size={14} className="text-text-secondary flex-shrink-0" />
        ) : item.checked !== undefined ? (
          <CheckIcon size={14} className={`ml-auto flex-shrink-0 ${item.checked ? 'text-accent' : 'text-transparent'}`} />
        ) : (
          binding && (
            <kbd className="ml-auto pl-6 text-xs text-text-secondary whitespace-nowrap">
              {formatShortcut(binding, mac)}
            </kbd>
          )
        )}
      </MenuRow>
    );
  };

  const menuStyle: CSSProperties = menuPos
    ? {
        position: 'fixed',
        top: menuPos.top,
        left: menuPos.left,
        zIndex: 9999,
        visibility: 'visible',
        maxHeight: menuPos.maxHeight,
        overflowY: 'auto',
      }
    : {
        // Hidden first-pass: NO maxHeight, so the layout effect measures the
        // menu's natural height before deciding side + cap.
        position: 'fixed',
        top: 0,
        left: triggerRect?.left ?? 0,
        zIndex: 9999,
        visibility: 'hidden',
        pointerEvents: 'none',
      };

  const activeSubmenu = items.find((i) => i.id === openSubmenuId)?.submenu;
  const submenuStyle: CSSProperties = submenuPos
    ? {
        position: 'fixed',
        top: submenuPos.top,
        left: submenuPos.left,
        zIndex: 9999,
        visibility: 'visible',
        maxHeight: submenuPos.maxHeight,
        overflowY: 'auto',
      }
    : {
        position: 'fixed',
        top: submenuParentRect?.top ?? 0,
        left: submenuParentRect?.right ?? 0,
        zIndex: 9999,
        visibility: 'hidden',
        pointerEvents: 'none',
      };

  return (
    <>
      <IconButton
        ref={triggerRef}
        icon={<MenuIcon className={triggerDanger ? 'text-red-500' : undefined} />}
        size="sm"
        // Override the fixed sm square (w-7 h-7) with a tighter one at the final
        // condensation stage; `!` wins over the size class regardless of CSS order.
        className={compact ? '!w-6 !h-6' : undefined}
        disabled={disabled}
        onClick={() => {
          if (disabled) return;
          isOpen ? closeMenu() : openMenu();
        }}
        onKeyDown={onTriggerKeyDown}
        aria-label={ariaLabel}
        aria-haspopup="menu"
        aria-expanded={isOpen}
        aria-controls={isOpen ? menuId : undefined}
        data-testid={triggerTestId}
        data-dropdown-open={isOpen || undefined}
      />

      {isOpen &&
        createPortal(
          <div
            ref={menuRef}
            id={menuId}
            role="menu"
            aria-label={ariaLabel}
            data-testid={menuTestId}
            className="bg-panel border border-border rounded-lg shadow-[0_16px_40px_rgba(0,0,0,0.6)] py-1 min-w-[220px]"
            style={menuStyle}
            onKeyDown={onMenuKeyDown}
          >
            {items.map((item) => renderRow(item, { isSubmenu: false }))}
          </div>,
          document.body,
        )}

      {isOpen &&
        openSubmenuId &&
        activeSubmenu &&
        createPortal(
          <div
            ref={submenuRef}
            id={submenuDomId}
            role="menu"
            aria-label={items.find((i) => i.id === openSubmenuId)?.label ?? 'Submenu'}
            data-testid={`${menuTestId}-submenu-${openSubmenuId}`}
            className="bg-panel border border-border rounded-lg shadow-[0_16px_40px_rgba(0,0,0,0.6)] py-1 min-w-[200px]"
            style={submenuStyle}
            onKeyDown={(e) => onSubmenuKeyDown(e, activeSubmenu)}
          >
            {activeSubmenu.map((item) => renderRow(item, { isSubmenu: true }))}
          </div>,
          document.body,
        )}
    </>
  );
}
