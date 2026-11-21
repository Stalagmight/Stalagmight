/**
 * DisabledReason - why a control a read-only state turned off is off, in the
 * app's tooltip on hover (demo findings N-12, V-3, V-5).
 *
 * One component for every such control: the app menu's and the context
 * menu's rows (`MenuRow`'s `reason`) and the toolbar's Import, Undo and Redo.
 * The control itself must be `aria-disabled`, not `disabled`: WebKitGTK sends
 * no pointer events to a `disabled` button (no hover, so no tooltip; no click,
 * so no notice) and draws no native `title` for one. The caller turns a click
 * on it into the blocked-edit notice, never the action.
 *
 * The bubble is the opaque panel surface (`bg-panel`, as the menus are), so
 * the rows it overlaps never show through its text, and it opens at the
 * row's SIDE by default, so it covers neither neighbour of a menu row (V-3:
 * it opened above and covered the two rows over the hovered one). A toolbar
 * button, whose neighbours sit left and right, takes `below`.
 */
import type { ReactNode } from 'react';
import { Tooltip, type TooltipPlacement } from './Tooltip';

/** How long the pointer rests on a disabled control before its reason shows:
 *  the pointer crosses several rows on its way down a menu, and a bubble that
 *  opened on each would flicker. */
export const DISABLED_REASON_DELAY_MS = 300;

interface DisabledReasonProps {
  /** Why the control is off. */
  reason: string;
  /** The `aria-disabled` control. */
  children: ReactNode;
  placement?: TooltipPlacement;
  /** Extra classes for the wrapper around the control. */
  className?: string;
  /** Test id of the bubble (`<control test id>-reason` by convention). */
  testId?: string | undefined;
}

export function DisabledReason({ reason, children, placement = 'side', className, testId }: DisabledReasonProps) {
  return (
    <Tooltip
      content={reason}
      openDelayMs={DISABLED_REASON_DELAY_MS}
      role="none"
      opaque
      placement={placement}
      {...(className !== undefined ? { className } : {})}
      {...(testId !== undefined ? { tooltipTestId: testId } : {})}
    >
      {children}
    </Tooltip>
  );
}
