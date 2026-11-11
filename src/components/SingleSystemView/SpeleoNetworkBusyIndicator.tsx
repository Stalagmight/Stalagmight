import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { selectSpeleoNetworkBusy, useSpeleoNetworkStore } from '../../store/speleoNetworkStore';

/**
 * A small spinner on the SpeleoDB tab while any SpeleoDB network call is in
 * flight (see `speleoNetworkStore`).
 *
 * Wrap the tab's icon with it: the spinner then overlays the icon's TOP-LEFT
 * corner on a panel-coloured disc, the mirror of the read-only lock badge that
 * sits on the bottom-right corner, so it never covers that badge. Without an
 * icon to wrap, it is a zero-size anchor and the spinner sits just after it.
 * Either way the spinner is absolutely positioned and adds no width: the tab
 * keeps its size and its label never moves.
 *
 * The live region is mounted whether or not anything is in flight, so screen
 * readers announce the label when the spinner appears.
 */
export function SpeleoNetworkBusyIndicator({ children }: { children?: ReactNode }) {
  const { t } = useTranslation();
  const busy = useSpeleoNetworkStore(selectSpeleoNetworkBusy);
  const overIcon = children != null;
  const label = t('speleodb.workspaceTab.busy');
  return (
    <span
      className={`relative ${overIcon ? 'inline-flex' : 'inline-block h-0 w-0 align-middle'}`}
      data-testid="speleodb-network-busy-slot"
    >
      {children}
      <span role="status" aria-live="polite" className="sr-only" data-testid="speleodb-network-busy-status">
        {busy ? label : ''}
      </span>
      {busy && (
        <span
          title={label}
          aria-hidden="true"
          className={`pointer-events-none absolute flex items-center justify-center text-accent ${
            overIcon
              ? '-left-1 -top-1 rounded-full bg-panel p-[1px]'
              : 'left-1.5 top-1/2 -translate-y-1/2'
          }`}
          data-testid="speleodb-network-busy"
          data-placement={overIcon ? 'icon' : 'anchor'}
        >
          {/* Filled, not stroked: a three-quarter ring drawn as one closed shape. */}
          <svg viewBox="0 0 16 16" className="h-2.5 w-2.5 animate-spin" aria-hidden="true" focusable="false">
            <path fill="currentColor" d="M8 0a8 8 0 1 0 8 8h-2.5A5.5 5.5 0 1 1 8 2.5V0z" />
          </svg>
        </span>
      )}
    </span>
  );
}
