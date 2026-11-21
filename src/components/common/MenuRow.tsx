/**
 * MenuRow / MenuSeparator - the row body shared by AppMenu and ContextMenu.
 *
 * Both menus render visually-identical rows: the same button base classes
 * ('w-full px-3 py-2 text-left text-sm flex items-center gap-2'), the same
 * icon gating (an 18px wrapper span rendered only at syntactic level >=
 * 'icons'), and the same 'border-t border-border my-1' separator. This module
 * owns exactly those identical parts; each menu keeps its own concerns
 * (portal/positioning/keyboard nav in AppMenu; overlay/positioning in
 * ContextMenu) by passing variant classes, trailing content, and any extra
 * button attributes in.
 */
import { forwardRef, useId, type ReactNode, type ButtonHTMLAttributes, type CSSProperties } from 'react';
import { DisabledReason, DISABLED_REASON_DELAY_MS } from './DisabledReason';

/** The button base classes shared by every menu row. */
export const MENU_ROW_BASE_CLASS =
  'w-full px-3 py-2 text-left text-sm flex items-center gap-2';

/** A row's hint line: ONE line of `text-xs`, whatever the text, truncated
 *  (callers pass the full text as `title`). Fixed so a row whose hint arrives
 *  later keeps its height; two lines left a blank one under every short hint. */
export const MENU_ROW_HINT_CLASS = 'text-xs leading-4 h-4 truncate';

/** A wrapping hint, for a row whose hint is known when it first renders (a
 *  flyout's rows): as tall as the text, up to three lines. */
export const MENU_ROW_HINT_WRAP_CLASS = 'text-xs leading-4 line-clamp-3 whitespace-normal';

/** Pixel heights the menus position themselves with: a plain row, and the
 *  extra a hint adds (`h-4`). */
export const MENU_ROW_HEIGHT_PX = 36;
export const MENU_ROW_HINT_HEIGHT_PX = 16;

/** How long the pointer rests on a disabled row before its reason shows
 *  ({@link DISABLED_REASON_DELAY_MS}). */
export const MENU_ROW_REASON_DELAY_MS = DISABLED_REASON_DELAY_MS;

interface MenuRowProps extends ButtonHTMLAttributes<HTMLButtonElement> {
  'data-testid'?: string;
  /** Row label (rendered in the flex-1 truncating text span). */
  label: string;
  /** Leading icon, rendered only when `showIcons` is true. */
  icon?: ReactNode;
  /** Whether to render the leading icon wrapper (syntactic level >= 'icons'). */
  showIcons: boolean;
  /** Inline style for the icon wrapper (e.g. a fixed icon tint). */
  iconStyle?: CSSProperties | undefined;
  /** Variant / state classes appended after the shared base classes. */
  className?: string;
  /** Trailing content rendered after the label (chevron, check, shortcut…). */
  children?: ReactNode;
  /**
   * A second, smaller muted line under the label (why a row is disabled, or
   * what it will do). Fixed at one line, so a row keeps its height while the
   * text changes; longer text is truncated, so callers also pass it as `title`.
   */
  hint?: string | undefined;
  /** Let the hint wrap to its own height (up to three lines) instead of the
   *  fixed single line: only for rows that never re-render with a new hint. */
  hintWraps?: boolean;
  /**
   * Why a read-only state turned this `disabled` row off (demo finding N-12),
   * shown in the app's tooltip on hover (`DisabledReason`: opaque, at the
   * row's side, V-3). Such a row is `aria-disabled`, not `disabled`: a
   * `disabled` button gets no click (so the menu could not say why on one),
   * and WebKitGTK drew no native `title` tooltip for one. The menu must
   * therefore turn a click on it into the notice, never the action. Ignored
   * on an enabled row.
   */
  reason?: string | undefined;
}

/**
 * A menu row button: shared base classes + icon-gated 18px wrapper + truncating
 * label + an optional trailing slot. All other button attributes (ref, role,
 * onClick, disabled, data-testid, aria-*, onMouseEnter, …) pass straight through.
 */
export const MenuRow = forwardRef<HTMLButtonElement, MenuRowProps>(function MenuRow(
  { label, icon, showIcons, iconStyle, className, children, hint, hintWraps, reason, 'data-testid': testId, ...buttonProps },
  ref,
) {
  const hintId = useId();
  // A disabled row fades its icon and label (as a disabled `Button` does,
  // `opacity-50`), so it reads as off at a glance: the muted text colour alone
  // was hard to tell from an enabled row (demo finding I-10). The hint, which
  // says why, keeps its full muted colour.
  const faded = buttonProps.disabled ? ' opacity-50' : '';
  // With a hint, the accessible name stays the label and the hint describes it.
  const hintProps =
    hint === undefined ? {} : { 'aria-label': buttonProps['aria-label'] ?? label, 'aria-describedby': hintId };
  const explained = buttonProps.disabled === true && reason !== undefined;
  const button = (
    <button
      {...buttonProps}
      {...hintProps}
      ref={ref}
      disabled={explained ? undefined : buttonProps.disabled}
      aria-disabled={explained ? true : buttonProps['aria-disabled']}
      data-testid={testId}
      className={`${MENU_ROW_BASE_CLASS}${className ? ` ${className}` : ''}`}
    >
      {showIcons && (
        <span
          className={`w-[18px] h-[18px] flex-shrink-0 inline-flex items-center justify-center${faded}`}
          style={iconStyle}
          aria-hidden="true"
        >
          {icon}
        </span>
      )}
      {hint === undefined ? (
        <span className={`flex-1 truncate${faded}`}>{label}</span>
      ) : (
        <span className="flex-1 min-w-0 flex flex-col">
          <span className={`truncate${faded}`}>{label}</span>
          <span
            id={hintId}
            className={`${hintWraps ? MENU_ROW_HINT_WRAP_CLASS : MENU_ROW_HINT_CLASS} text-text-secondary`}
            data-testid={testId ? `${testId}-hint` : undefined}
          >
            {hint}
          </span>
        </span>
      )}
      {children}
    </button>
  );
  if (!explained) return button;
  return (
    <DisabledReason reason={reason} className="!flex w-full" testId={testId ? `${testId}-reason` : undefined}>
      {button}
    </DisabledReason>
  );
});

/** The horizontal rule shared by both menus' separators. */
export function MenuSeparator(props: { role?: string }) {
  return <div role={props.role} className="border-t border-border my-1" />;
}
