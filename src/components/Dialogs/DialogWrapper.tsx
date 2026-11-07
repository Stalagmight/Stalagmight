import { useLayoutEffect, useEffect, useId, useRef, type ReactNode, useCallback, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { Button, IconButton } from '../common/Button';
import { CloseIcon } from '../common/icons';
import { pushDialog, popDialog } from '../../utils/dialogStack';
import { useFocusTrap } from '../../hooks/useFocusTrap';

/** Resolve portal target once at module level to avoid per-render getElementById calls. */
let portalTargetCache: HTMLElement | null = null;
let portalTargetWarned = false;

function getPortalTarget(): HTMLElement | null {
  // Re-resolve if cached element is detached (HMR/test cleanup can remove it)
  if (portalTargetCache && portalTargetCache.isConnected) return portalTargetCache;
  portalTargetCache = document.getElementById('dialog-root');
  if (!portalTargetCache && !portalTargetWarned && import.meta.env.DEV) {
    portalTargetWarned = true;
    console.warn(
      '[DialogWrapper] #dialog-root element not found in DOM. ' +
      'Ensure <div id="dialog-root"></div> exists in index.html after #root.',
    );
  }
  return portalTargetCache;
}

/**
 * Check if an element is visible (has layout) and not inside an
 * [aria-hidden="true"] ancestor within the given container.
 */
function isVisibleAndAccessible(el: HTMLElement, container: HTMLElement): boolean {
  // Check the element has layout (is rendered and visible)
  if (el.getClientRects().length === 0) return false;

  // Walk ancestors up to the container, checking for aria-hidden
  let current: Element | null = el.parentElement;
  while (current && current !== container) {
    if (current.getAttribute('aria-hidden') === 'true') return false;
    current = current.parentElement;
  }
  return true;
}

/**
 * Focus an element and, for text inputs/textareas, place the cursor
 * at the end of the value (no auto-select).
 */
function focusElement(el: HTMLElement): void {
  el.focus();
  if (
    (el instanceof HTMLInputElement && (el.type === 'text' || el.type === 'number' || el.type === 'search' || el.type === 'url' || el.type === 'email' || el.type === 'tel')) ||
    el instanceof HTMLTextAreaElement
  ) {
    const len = el.value.length;
    // setSelectionRange is not supported on number inputs in some browsers;
    // guard with try/catch to avoid throwing.
    try {
      el.setSelectionRange(len, len);
    } catch {
      // Ignore - some input types (e.g., number) don't support selection
    }
  }
}

type DialogSize = 'sm' | 'md' | 'lg' | 'xl' | '2xl' | '4xl' | '5xl' | 'fullscreen';

const sizeClasses: Record<DialogSize, string> = {
  sm: 'max-w-sm',
  md: 'max-w-md',
  lg: 'max-w-lg',
  xl: 'max-w-xl',
  '2xl': 'max-w-2xl',
  '4xl': 'max-w-4xl',
  '5xl': 'max-w-5xl',
  // Near-viewport "focus mode" footprint for surfaces that take over the screen
  // (e.g. the import wizard). Explicit width/height with a 1rem inset on each
  // side; the flex parent centers it. Exit stays via the close button / Escape.
  fullscreen: 'max-w-none w-[calc(100vw-2rem)] h-[calc(100vh-2rem)]',
};

/** Fixed dimensions for dialogs that need constant size regardless of content. */
export interface FixedSize {
  width: string;
  height: string;
}

interface DialogWrapperProps {
  title: string;
  onClose: () => void;
  children: ReactNode | ((props: { requestClose: () => void }) => ReactNode);
  isDirty?: boolean;
  size?: DialogSize;
  'data-testid'?: string;
  maxHeight?: string;
  /** Whether clicking the backdrop closes the dialog. Defaults to true. */
  closeOnBackdropClick?: boolean;
  /** Body overflow behavior. Defaults to 'auto'; the header always remains visible. */
  overflow?: 'auto' | 'hidden';
  /** Additional className for the body content wrapper. */
  bodyClassName?: string;
  /** Fixed dimensions that override size and maxHeight. Uses CSS min()/clamp() for responsive fallback. */
  fixedSize?: FixedSize;
  /** Additional keydown handler for dialog-level shortcuts. */
  onKeyDown?: (event: ReactKeyboardEvent<HTMLDivElement>) => void;
  /** Hide the dialog visually while keeping it mounted (preserves state). */
  hidden?: boolean;
}

export function DialogWrapper({
  title,
  onClose,
  children,
  isDirty = false,
  size = '2xl',
  'data-testid': testId,
  maxHeight = 'max-h-[90vh]',
  closeOnBackdropClick = true,
  overflow = 'auto',
  bodyClassName = 'p-4',
  fixedSize,
  onKeyDown,
  hidden = false,
}: DialogWrapperProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const dialogRef = useRef<HTMLDivElement>(null);
  const dialogContentRef = useRef<HTMLDivElement>(null);
  const alertDialogRef = useRef<HTMLDivElement>(null);
  const [showConfirm, setShowConfirm] = useState(false);

  // Focus trap for the main dialog container.
  // Disabled when the dirty-confirm alertdialog is open so that its own trap takes over.
  useFocusTrap(dialogRef, { enabled: !showConfirm });

  // Focus trap for the nested dirty-confirm alertdialog.
  // Only active when the confirm overlay is shown.
  useFocusTrap(alertDialogRef, { enabled: showConfirm });

  // Set inert on the outer dialog content (header + body) when dirty-confirm is shown.
  // This prevents Tab from reaching outer dialog elements while the alertdialog is open.
  // When dismissed, remove inert and return focus to the outer dialog container.
  useLayoutEffect(() => {
    const contentEl = dialogContentRef.current;
    if (!contentEl) return;

    if (showConfirm) {
      contentEl.setAttribute('inert', '');
    } else {
      contentEl.removeAttribute('inert');
    }
  }, [showConfirm]);

  // When dirty-confirm opens, focus the alertdialog container.
  // When it closes, return focus to the outer dialog container.
  useLayoutEffect(() => {
    if (showConfirm) {
      const alertEl = alertDialogRef.current;
      if (alertEl && !alertEl.contains(document.activeElement)) {
        alertEl.focus();
      }
    } else {
      // Return focus to the outer dialog when confirm is dismissed
      // (but not on initial mount when showConfirm starts as false)
      const dialogEl = dialogRef.current;
      if (dialogEl && !dialogEl.contains(document.activeElement)) {
        dialogEl.focus();
      }
    }
  }, [showConfirm]);

  const requestClose = useCallback(() => {
    if (isDirty) {
      setShowConfirm(true);
    } else {
      onClose();
    }
  }, [isDirty, onClose]);

  const handleConfirmDiscard = useCallback(() => {
    setShowConfirm(false);
    onClose();
  }, [onClose]);

  const handleCancelDiscard = useCallback(() => {
    setShowConfirm(false);
  }, []);

  // Store the element that had focus before the dialog opened so we can
  // return focus to it on unmount. Captured once at mount time.
  const triggerRef = useRef<Element | null>(document.activeElement);

  // Register with dialogStack for inert management and focus the container.
  // useLayoutEffect ensures inert + focus are applied before the browser paints,
  // preventing timing windows where key presses reach global handlers.
  useLayoutEffect(() => {
    const el = dialogRef.current;
    if (!el) return;
    pushDialog(el);
    // Focus the dialog container so keyboard events (Escape) are reliably
    // intercepted by onKeyDown rather than falling through to global handlers.
    // Only focus if current active element is outside the dialog to avoid
    // overriding autoFocus on inputs inside dialogs.
    if (!el.contains(document.activeElement)) {
      el.focus();
    }
    return () => {
      popDialog(el);
    };
  }, []);

  // Initial focus placement using heuristic priority order.
  // Runs after mount with requestAnimationFrame so the DOM is fully rendered.
  // Cleanup: cancels pending rAF on unmount (React Strict Mode safety).
  useEffect(() => {
    const el = dialogContentRef.current;
    if (!el) return;

    const rafId = requestAnimationFrame(() => {
      // If focus already landed on a specific element inside the dialog before
      // this frame (a fast click/tab, a child's own deliberate focus effect,
      // or a component test typing right after render), keep it - only the
      // container-level default focus set by the mount layout effect above
      // gets upgraded by the heuristic. Checked against the OUTER container
      // (dialogRef, the element that layout effect focuses) so the
      // dirty-confirm alertdialog - a sibling of the content panel - is
      // protected too; the identity exclusion is what still lets that default
      // container focus be upgraded. Without this guard the rAF yanked focus
      // mid-interaction: under jsdom's ~16ms rAF it fired in the middle of a
      // userEvent.type() and the keystrokes went to the stolen-focus target
      // (#289, the SectionAdjustDialog flake).
      const outer = dialogRef.current;
      const active = document.activeElement;
      if (outer && active instanceof HTMLElement && active !== outer && outer.contains(active)) {
        return;
      }

      // Priority 1: element with [data-autofocus]
      const autofocus = el.querySelector<HTMLElement>('[data-autofocus]');
      if (autofocus && isVisibleAndAccessible(autofocus, el)) {
        focusElement(autofocus);
        return;
      }

      // Priority 2: first visible, accessible input, textarea, or select
      const formCandidates = el.querySelectorAll<HTMLElement>(
        'input:not(:disabled):not([type="hidden"]):not([type="checkbox"]):not([type="radio"]), textarea:not(:disabled), select:not(:disabled)',
      );
      for (const candidate of formCandidates) {
        if (isVisibleAndAccessible(candidate, el)) {
          focusElement(candidate);
          return;
        }
      }

      // Priority 3: enabled element with [data-primary-action]
      const primaryCandidates = el.querySelectorAll<HTMLElement>('[data-primary-action]:not(:disabled)');
      for (const candidate of primaryCandidates) {
        if (isVisibleAndAccessible(candidate, el)) {
          focusElement(candidate);
          return;
        }
      }

      // Priority 4: first enabled button that is NOT the close X button
      const buttons = el.querySelectorAll<HTMLElement>('button:not(:disabled)');
      for (const btn of buttons) {
        if (!btn.hasAttribute('data-dialog-close') && isVisibleAndAccessible(btn, el)) {
          focusElement(btn);
          return;
        }
      }

      // Priority 5: fallback to the close X button
      const closeBtn = el.querySelector<HTMLElement>('button[data-dialog-close]');
      if (closeBtn) {
        focusElement(closeBtn);
      }
    });

    return () => {
      cancelAnimationFrame(rafId);
    };
  }, []);

  // Return focus to the trigger element on unmount.
  // popDialog() removes inert from app siblings before this runs,
  // so the trigger element can receive focus.
  useEffect(() => {
    const trigger = triggerRef.current;
    return () => {
      if (
        trigger &&
        trigger instanceof HTMLElement &&
        document.body.contains(trigger)
      ) {
        trigger.focus();
      }
    };
  }, []);

  const handleKeyDown = useCallback(
    (e: ReactKeyboardEvent<HTMLDivElement>) => {
      if ((e.key === 'Escape' || e.key === 'Esc') && !e.repeat && !e.defaultPrevented) {
        // Don't close dialog if a dropdown is open - let the dropdown close first
        const openDropdown = document.querySelector('[data-dropdown-open]');
        if (openDropdown) {
          return;
        }

        e.preventDefault();
        e.stopPropagation();
        if (showConfirm) {
          handleCancelDiscard();
        } else {
          requestClose();
        }
        return;
      }

      onKeyDown?.(e);
    },
    [requestClose, showConfirm, handleCancelDiscard, onKeyDown],
  );

  // Only close on backdrop click when the mousedown also started on the backdrop.
  // This prevents stale double-click events from a previous dialog from closing
  // a newly mounted dialog, and also prevents drag-to-backdrop from triggering close.
  const backdropMouseDownRef = useRef(false);

  const handleBackdropMouseDown = useCallback((e: ReactMouseEvent) => {
    if (e.target === e.currentTarget) {
      backdropMouseDownRef.current = true;
    }
  }, []);

  const handleBackdropClick = useCallback((e: ReactMouseEvent) => {
    if (closeOnBackdropClick && e.target === e.currentTarget && backdropMouseDownRef.current) {
      requestClose();
    }
    backdropMouseDownRef.current = false;
  }, [closeOnBackdropClick, requestClose]);

  const portalTarget = getPortalTarget();
  if (!portalTarget) {
    return null;
  }

  // fullscreen sets its own explicit height, so it skips the default max-h clamp.
  const containerSizing = fixedSize
    ? ''
    : size === 'fullscreen'
      ? sizeClasses[size]
      : `${sizeClasses[size]} ${maxHeight} w-full mx-4`;

  return createPortal(
    <div
      ref={dialogRef}
      className="fixed inset-0 z-50 flex items-center justify-center"
      style={hidden ? { display: 'none' } : undefined}
      tabIndex={-1}
      onKeyDown={handleKeyDown}
    >
      <div
        className="absolute inset-0 bg-overlay"
        onMouseDown={handleBackdropMouseDown}
        onClick={handleBackdropClick}
        data-testid={testId ? `${testId}-backdrop` : 'dialog-backdrop'}
      />
      <div
        ref={dialogContentRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid={testId}
        className={`relative flex flex-col overflow-hidden bg-panel border border-border rounded-lg shadow-xl ${containerSizing}`}
        style={fixedSize ? { width: fixedSize.width, height: fixedSize.height } : undefined}
      >
        <div className="flex shrink-0 items-center justify-between px-4 py-3 border-b border-border">
          <h2 id={titleId} className="text-lg font-medium">{title}</h2>
          <IconButton
            data-testid={testId ? `${testId}-close` : 'dialog-close'}
            // Marks the X for the initial-focus pass above: its aria-label is
            // translated, so it cannot be found by text.
            data-dialog-close=""
            type="button"
            onClick={requestClose}
            icon={<CloseIcon size={14} />}
            size="sm"
            variant="ghost"
            aria-label={t('dialogs.dialogWrapper.closeDialog')}
            className="text-text-secondary hover:text-text-primary"
          />
        </div>
        <div
          className={`min-h-0 flex-1 ${overflow === 'auto' ? 'overflow-y-auto' : 'overflow-hidden'} ${bodyClassName}`}
          data-testid={testId ? `${testId}-body` : undefined}
        >
          {typeof children === 'function' ? children({ requestClose }) : children}
        </div>
      </div>

      {/* Inline confirm overlay for dirty state */}
      {showConfirm && (
        <div
          ref={alertDialogRef}
          className="fixed inset-0 z-[60] flex items-center justify-center"
          tabIndex={-1}
        >
          <div
            className="absolute inset-0 bg-overlay"
            onClick={handleCancelDiscard}
            data-testid={testId ? `${testId}-discard-backdrop` : 'dialog-discard-backdrop'}
          />
          <div
            role="alertdialog"
            aria-modal="true"
            aria-labelledby="confirm-discard-title"
            className="relative bg-panel border border-border rounded-lg shadow-xl max-w-sm w-full mx-4"
          >
            <div className="flex items-center justify-between px-4 py-3 border-b border-border">
              <h2 id="confirm-discard-title" className="text-lg font-medium">{t('dialogs.dialogWrapper.discardTitle')}</h2>
            </div>
            <div className="p-4 space-y-4">
              <p>{t('dialogs.dialogWrapper.discardMessage')}</p>
              <p className="text-text-secondary text-sm">{t('dialogs.dialogWrapper.discardNote')}</p>
              <div className="flex justify-end gap-2 pt-4 border-t border-border">
                <Button type="button" onClick={handleCancelDiscard} data-testid="dirty-dialog-keep-editing">{t('common.actions.cancel')}</Button>
                <Button type="button" variant="danger" onClick={handleConfirmDiscard} data-testid="dirty-dialog-discard">
                  {t('dialogs.dialogWrapper.discard')}
                </Button>
              </div>
            </div>
          </div>
        </div>
      )}
    </div>,
    portalTarget,
  );
}
