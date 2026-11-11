import { useId, useLayoutEffect, useRef } from 'react';
import { createPortal } from 'react-dom';
import { popDialog, pushDialog } from '../../utils/dialogStack';

/** A centered, non-dismissable progress surface above the active form and app. */
export function SpeleoProgressOverlay({ title, description, testId }: {
  title: string; description: string; testId: string;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const titleId = useId();
  useLayoutEffect(() => {
    const container = ref.current;
    if (!container) return;
    const previous = document.activeElement;
    pushDialog(container);
    container.focus();
    return () => {
      popDialog(container);
      if (previous instanceof HTMLElement && previous.isConnected) previous.focus();
    };
  }, []);
  return createPortal(
    <div ref={ref} role="dialog" aria-modal="true" aria-labelledby={titleId} aria-busy="true"
      tabIndex={-1} data-testid={testId}
      onKeyDown={(event) => { event.preventDefault(); event.stopPropagation(); }}
      className="fixed inset-0 flex items-center justify-center bg-overlay p-6 backdrop-blur-sm focus:outline-none">
      <div className="flex w-full max-w-sm flex-col items-center rounded-xl border border-border bg-panel px-7 py-9 text-center shadow-2xl" role="status" aria-live="polite">
        <svg aria-hidden="true" className="mb-6 h-12 w-12 animate-spin text-accent" viewBox="0 0 24 24" fill="none">
          <circle className="opacity-20" cx="12" cy="12" r="10" stroke="currentColor" strokeWidth="2.5" />
          <path d="M12 2a10 10 0 0 1 10 10" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" />
        </svg>
        <h2 id={titleId} className="text-lg font-semibold text-text-primary">{title}</h2>
        <p className="mt-2 text-sm leading-relaxed text-text-secondary">{description}</p>
      </div>
    </div>, document.getElementById('dialog-root') ?? document.body,
  );
}
