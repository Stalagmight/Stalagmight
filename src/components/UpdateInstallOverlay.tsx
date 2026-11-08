import { useEffect, useId, useLayoutEffect, useRef, type KeyboardEvent as ReactKeyboardEvent } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import { useUpdateStore, selectUpdateInstalling } from '../store/updateStore';
import { pushDialog, popDialog } from '../utils/dialogStack';
import { useScreenOccupant } from '../hooks/useScreenOccupant';

/**
 * "Installing update..." - holds the app still while an update restart installs
 * (#579 x #576).
 *
 * The install runs off the main thread (so the restart exits like a quit: the
 * SpeleoDB lock release runs, autosave stays held), which leaves the window live
 * for the seconds it takes to write the new bundle. The user has already passed
 * the unsaved-changes prompt by then, so an edit made in those seconds would be
 * dropped by the restart without a word. While `updateStore.installing` is set,
 * this window:
 *
 * - is a screen occupant (`useScreenOccupant`), so both shortcut hooks stand down,
 *   Escape included, as they do for any prompt outside the `activeDialog` slot;
 * - sits on the dialog stack (`pushDialog`), so `#root` - the menus, toolbar, map
 *   and panels - is inert and painted under it;
 * - takes focus and swallows every key that reaches it, so nothing bubbles on to
 *   a window-level handler.
 *
 * It follows the loading window's and the integrity dialog's conventions (#576):
 * the focused element is the dialog itself (role, `aria-modal` and its labels sit
 * on it, so focus never rests on an element with no role or name), it portals
 * into `#dialog-root` or, failing that, `document.body` (never nothing while it
 * holds the screen as an occupant), and when it goes away it hands focus back to
 * the element that had it, if that is still there and focus went with the window.
 * It needs no Tab trap (`useFocusTrap`): it holds nothing focusable, and
 * `holdKey` already swallows Tab.
 *
 * The editing gate and the mutation queue read the same flag
 * (`selectUpdateInstalling`), so an edit that does not come through the keyboard
 * or a control is refused too. It has no buttons: an install cannot be cancelled.
 * It goes away when the install fails (the existing failure toast then says so)
 * and never on success, which ends the process.
 */
export function UpdateInstallOverlay() {
  const installing = useUpdateStore(selectUpdateInstalling);
  useScreenOccupant(installing);
  return installing ? <InstallingWindow /> : null;
}

/** Swallow every key: nothing may act while the install runs. */
function holdKey(event: ReactKeyboardEvent<HTMLDivElement>): void {
  event.preventDefault();
  event.stopPropagation();
}

function InstallingWindow() {
  const { t } = useTranslation();
  const version = useUpdateStore((s) => s.info?.version ?? null);
  const containerRef = useRef<HTMLDivElement>(null);
  // The element that had focus when the window opened, read at the first render
  // (as DialogWrapper and the loading window do), so a Strict Mode effect replay
  // cannot re-read it from inside the window.
  const triggerRef = useRef<Element | null>(document.activeElement);
  const titleId = useId();
  const bodyId = useId();

  // Layout effect, as `DialogWrapper` does it: inert and focus land before the
  // browser paints, so no key press slips through to the app in between.
  useLayoutEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    pushDialog(el);
    el.focus();
    return () => popDialog(el);
  }, []);

  // Give focus back when the window goes (a failed install), after popDialog has
  // lifted `inert` from `#root`: only when focus went with the window (it is on
  // the body now), as the loading window does.
  useEffect(() => {
    const trigger = triggerRef.current;
    return () => {
      const active = document.activeElement;
      if (active !== null && active !== document.body) return;
      if (trigger instanceof HTMLElement && trigger.isConnected) trigger.focus();
    };
  }, []);

  return createPortal(
    <div
      ref={containerRef}
      className="fixed inset-0 z-50 flex items-center justify-center"
      role="alertdialog"
      aria-modal="true"
      aria-busy="true"
      aria-labelledby={titleId}
      aria-describedby={bodyId}
      tabIndex={-1}
      onKeyDown={holdKey}
      data-testid="update-install-overlay"
    >
      <div className="absolute inset-0 bg-overlay" />
      <div
        className="relative w-80 max-w-[calc(100vw-2rem)] bg-panel border border-border rounded-lg shadow-xl p-6"
        data-testid="update-install-panel"
      >
        <h2 id={titleId} className="text-lg font-medium text-text-primary mb-2">
          {version ? t('updateInstall.titleVersion', { version }) : t('updateInstall.title')}
        </h2>
        <p id={bodyId} className="text-sm text-text-secondary mb-4">
          {t('updateInstall.body')}
        </p>
        <div className="h-1.5 bg-border rounded-full overflow-hidden">
          <div
            className="h-full bg-accent-70 animate-pulse-shimmer"
            role="progressbar"
            aria-label={t('updateInstall.progressAria')}
          />
        </div>
      </div>
    </div>,
    document.getElementById('dialog-root') ?? document.body,
  );
}
