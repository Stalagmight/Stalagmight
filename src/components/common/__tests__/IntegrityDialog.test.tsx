/**
 * [NF3] The "Project integrity" dialog is on the dialog stack like any other dialog.
 *
 * It used to render inline inside `#root`, outside the stack: a dialog (or the
 * loading window) stacked over it made `#root` inert and the integrity dialog
 * with it, while it still painted on top; Tab walked out of it; focus never moved
 * into it. Now it is portaled into `#dialog-root` and pushed on the stack: the app
 * behind it is inert, focus moves to its first control, Tab wraps inside it, a
 * dialog opened over it takes the top and hands it back, and focus goes back to
 * the element that had it when it closes. Real i18n, `.tsx` so vitest runs it
 * under jsdom.
 */
import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import '../../../i18n';
import { IntegrityDialog } from '../IntegrityDialog';
import type { EditBlockedReason } from '../../../hooks/useEditingBlocked';
import { isTopmost, popDialog, pushDialog } from '../../../utils/dialogStack';

const ISSUES = ['Station merge #0 station_a references missing section 42'];

function Scene({
  open,
  withButton = true,
  repairBlocked = null,
}: {
  open: boolean;
  withButton?: boolean;
  repairBlocked?: EditBlockedReason | null;
}) {
  return (
    <>
      {withButton && <button type="button" data-testid="behind">Review</button>}
      {open && (
        <IntegrityDialog issues={ISSUES} repairBlocked={repairBlocked} repairing={false} onClose={vi.fn()} onRepair={vi.fn()} />
      )}
    </>
  );
}

/** Focus the button behind it, then open the dialog (as the banner's Review does). */
function openFromBehind(repairBlocked: EditBlockedReason | null = null) {
  const view = render(<Scene open={false} repairBlocked={repairBlocked} />);
  screen.getByTestId('behind').focus();
  view.rerender(<Scene open repairBlocked={repairBlocked} />);
  return view;
}

describe('IntegrityDialog on the dialog stack (NF3)', () => {
  it('portals into #dialog-root, makes what is behind it inert, and moves focus to its first control', () => {
    openFromBehind();
    const dialog = screen.getByTestId('integrity-dialog');
    expect(document.getElementById('dialog-root')).toContainElement(dialog);
    expect(isTopmost(dialog)).toBe(true);
    expect(screen.getByTestId('behind').closest('[inert]')).not.toBeNull();
    expect(screen.getByTestId('integrity-dialog-cancel')).toHaveFocus();
  });

  it('keeps Tab and Shift+Tab inside it', async () => {
    const user = userEvent.setup();
    openFromBehind();
    const cancel = screen.getByTestId('integrity-dialog-cancel');
    const repair = screen.getByTestId('integrity-dialog-repair');
    repair.focus();
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab({ shift: true });
    expect(repair).toHaveFocus();
  });

  it('with Repair unavailable, Tab and Shift+Tab stay on Cancel', async () => {
    const user = userEvent.setup();
    openFromBehind('workspace');
    const cancel = screen.getByTestId('integrity-dialog-cancel');
    expect(screen.getByTestId('integrity-dialog-repair')).toBeDisabled();
    cancel.focus();
    await user.tab();
    expect(cancel).toHaveFocus();
    await user.tab({ shift: true });
    expect(cancel).toHaveFocus();
  });

  it('a dialog opened over it takes the top and paints above it, and hands the top back when it closes', () => {
    openFromBehind();
    const dialog = screen.getByTestId('integrity-dialog');
    const above = document.createElement('div');
    document.getElementById('dialog-root')!.appendChild(above);
    try {
      pushDialog(above);
      expect(isTopmost(above)).toBe(true);
      expect(dialog).toHaveAttribute('inert');
      expect(dialog.style.zIndex).not.toBe('');
      expect(Number(above.style.zIndex)).toBeGreaterThan(Number(dialog.style.zIndex));

      popDialog(above);
      expect(isTopmost(dialog)).toBe(true);
      expect(dialog).not.toHaveAttribute('inert');
      // The integrity dialog is still up, so the app behind it stays inert.
      expect(screen.getByTestId('behind').closest('[inert]')).not.toBeNull();
    } finally {
      popDialog(above);
      above.remove();
    }
  });

  it('gives focus back to the element that had it when it closes', () => {
    const view = openFromBehind();
    const behind = screen.getByTestId('behind');
    expect(behind).not.toHaveFocus();
    view.rerender(<Scene open={false} />);
    expect(screen.queryByTestId('integrity-dialog')).toBeNull();
    expect(behind.closest('[inert]')).toBeNull();
    expect(behind).toHaveFocus();
  });

  // Guard (passes before and after): the element that had focus may be gone (a
  // repair that clears every issue removes the banner's Review with the dialog).
  it('does not throw when the element that had focus is gone by the time it closes', () => {
    const view = openFromBehind();
    view.rerender(<Scene open withButton={false} />);
    expect(screen.queryByTestId('behind')).toBeNull();
    expect(() => view.rerender(<Scene open={false} withButton={false} />)).not.toThrow();
    expect(screen.queryByTestId('integrity-dialog')).toBeNull();
  });
});
