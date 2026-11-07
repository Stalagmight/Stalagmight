import { useEffect, useLayoutEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { useTranslation } from 'react-i18next';
import type { EditBlockedReason } from '../../hooks/useEditingBlocked';
import { getFocusableElements, useFocusTrap } from '../../hooks/useFocusTrap';
import { popDialog, pushDialog } from '../../utils/dialogStack';

export interface IntegrityDialogProps {
  /** The integrity check's findings, one line each. */
  issues: string[];
  /** Why Repair (a survey mutation) is unavailable right now, if it is (#576 D39(a)). */
  repairBlocked: EditBlockedReason | null;
  /** A repair is running: both buttons are disabled and the backdrop does not close. */
  repairing: boolean;
  onClose: () => void;
  onRepair: () => void;
}

/**
 * The "Project integrity" details and repair dialog, opened from the integrity
 * banner's Review and by a save that the integrity issues blocked. App owns its
 * open state and the repair.
 *
 * [NF3] It used to be a `fixed inset-0 z-[1000]` div rendered inline inside
 * `#root`, outside the dialog stack. Any stacked layer (a dialog, the loading
 * window) makes `#root` inert, so it went inert with it while still painting on
 * top of that layer; and Tab was not trapped in it. It is now an ordinary
 * dialog, the way `DialogWrapper` does it: portaled into `#dialog-root` and
 * pushed on top of the dialog stack (so the app is inert, a dialog opened later
 * goes above it, and the loading window goes beneath it), `useFocusTrap` wraps
 * Tab inside it, focus moves to its first control when it opens, and goes back
 * to the element that had it when it closes - when that element is still in the
 * document (a repair that clears every issue removes the banner's Review with it).
 */
export function IntegrityDialog({ issues, repairBlocked, repairing, onClose, onRepair }: IntegrityDialogProps) {
  const { t } = useTranslation();
  const dialogRef = useRef<HTMLDivElement>(null);
  // The element that had focus when the dialog opened. Read at the first render,
  // as DialogWrapper does, so a Strict Mode effect replay cannot re-read it from
  // inside the dialog.
  const triggerRef = useRef<Element | null>(document.activeElement);

  useFocusTrap(dialogRef);

  // Register with the dialog stack and move focus in before the browser paints,
  // so no key press reaches the app behind it in between.
  useLayoutEffect(() => {
    const el = dialogRef.current;
    if (!el) return;
    pushDialog(el);
    if (!el.contains(document.activeElement)) {
      getFocusableElements(el)[0]?.focus();
    }
    return () => {
      popDialog(el);
    };
  }, []);

  // Give focus back on close. popDialog has lifted `inert` from `#root` by then.
  useEffect(() => {
    const trigger = triggerRef.current;
    return () => {
      if (trigger instanceof HTMLElement && trigger.isConnected) trigger.focus();
    };
  }, []);

  return createPortal(
    <div
      ref={dialogRef}
      className="fixed inset-0 z-50 bg-overlay flex items-center justify-center p-4"
      role="dialog"
      aria-modal="true"
      aria-labelledby="integrity-dialog-title"
      data-testid="integrity-dialog"
      onClick={() => !repairing && onClose()}
    >
      <div
        className="bg-panel border border-border rounded-lg shadow-xl w-full max-w-xl max-h-[80vh] flex flex-col"
        data-testid="integrity-dialog-panel"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="px-4 py-3 border-b border-border">
          <h2 id="integrity-dialog-title" className="font-medium text-text-primary">
            Project integrity ({issues.length}{' '}
            {issues.length === 1 ? 'issue' : 'issues'})
          </h2>
        </div>
        <div className="px-4 py-3 overflow-auto">
          <p className="text-sm text-text-secondary mb-3">
            The following problems were found in this project. Repairing drops invalid
            references and realigns drifted station merges; valid survey data is kept. Save the
            project afterwards to make the fix permanent.
          </p>
          <ul
            className="list-disc list-inside space-y-1 text-sm text-text-primary"
            data-testid="integrity-dialog-list"
          >
            {issues.map((issue, i) => (
              <li key={i} className="break-words">{issue}</li>
            ))}
          </ul>
        </div>
        <div className="px-4 py-3 border-t border-border flex justify-end gap-2">
          {repairBlocked !== null && (
            <span
              id="integrity-repair-blocked"
              className="mr-auto self-center text-xs text-text-secondary"
              data-testid="integrity-dialog-repair-blocked"
            >
              {t(`integrityRepair.blocked.${repairBlocked}`)}
            </span>
          )}
          <button
            onClick={() => onClose()}
            disabled={repairing}
            className="px-3 py-1.5 rounded border border-border text-sm hover:bg-background disabled:opacity-50"
            data-testid="integrity-dialog-cancel"
          >
            Cancel
          </button>
          <button
            onClick={onRepair}
            disabled={repairing || repairBlocked !== null}
            title={repairBlocked !== null ? t(`integrityRepair.blocked.${repairBlocked}`) : undefined}
            aria-describedby={repairBlocked !== null ? 'integrity-repair-blocked' : undefined}
            className="px-3 py-1.5 rounded bg-accent text-white text-sm disabled:opacity-50"
            data-testid="integrity-dialog-repair"
          >
            {repairing ? 'Repairing…' : 'Repair'}
          </button>
        </div>
      </div>
    </div>,
    document.getElementById('dialog-root') ?? document.body,
  );
}
