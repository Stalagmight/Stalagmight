/**
 * Regression test (#289): DialogWrapper's initial-focus heuristic runs in a
 * requestAnimationFrame after mount. It must NOT steal focus from a control
 * the user already focused inside the dialog - under jsdom's ~16ms rAF the
 * callback fired mid-userEvent.type(), blurred the input between the click
 * and the keystroke, and the typed text was lost (the flaky
 * SectionAdjustDialog "can clear a typed value" failure).
 *
 * The rAF is stubbed and fired by hand so both orderings are deterministic
 * (prove-the-guard-via-injection): with focus on an inner control the
 * heuristic must yield; with only the container-level default focus it must
 * still run. In jsdom every candidate fails the getClientRects() visibility
 * check, so a heuristic that runs always lands on Priority 5, the close X
 * button - which is exactly what the pre-fix focus steal looked like.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';

import '../../i18n';
import { DialogWrapper } from './DialogWrapper';

let rafCallbacks: FrameRequestCallback[];

beforeEach(() => {
  rafCallbacks = [];
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
    rafCallbacks.push(cb);
    return rafCallbacks.length;
  });
  vi.stubGlobal('cancelAnimationFrame', () => {});
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function renderDialog() {
  render(
    <DialogWrapper title="Focus test" onClose={() => {}}>
      <input data-testid="first-input" />
      <input data-testid="second-input" />
    </DialogWrapper>,
  );
}

describe('DialogWrapper initial-focus rAF (#289)', () => {
  it('keeps the header fixed and makes the body scroll within the viewport clamp', () => {
    render(
      <DialogWrapper title="Responsive dialog" onClose={() => {}} data-testid="responsive-dialog">
        <div>Long content</div>
      </DialogWrapper>,
    );

    const dialog = screen.getByTestId('responsive-dialog');
    const body = screen.getByTestId('responsive-dialog-body');
    expect(dialog).toHaveClass('flex', 'flex-col', 'overflow-hidden', 'max-h-[90vh]');
    expect(dialog.querySelector('h2')?.parentElement).toHaveClass('shrink-0');
    expect(body).toHaveClass('min-h-0', 'flex-1', 'overflow-y-auto');
  });

  it('does not steal focus from a control already focused inside the dialog', () => {
    renderDialog();
    const second = screen.getByTestId('second-input');
    second.focus();
    expect(second).toHaveFocus();

    rafCallbacks.forEach((cb) => cb(0)); // the mount rAF fires mid-interaction

    expect(second).toHaveFocus(); // pre-fix: yanked to the close X button
  });

  it('still places initial focus when only the container default is focused', () => {
    renderDialog();
    // The mount layout effect focused the outer dialog container (the parent
    // of the role="dialog" content panel); no user focus happened before the
    // first frame.
    const outer = screen.getByRole('dialog').parentElement as HTMLElement;
    expect(outer).toHaveFocus();

    rafCallbacks.forEach((cb) => cb(0));

    // The invariant: the heuristic ran and upgraded the container-level
    // default to some element inside the dialog. (Which element is an
    // environment detail - jsdom's empty getClientRects() fails every
    // visibility-checked candidate, so it happens to be the Priority 5 close
    // button here - so assert the movement, not the target.)
    expect(outer).not.toHaveFocus();
    expect(outer.contains(document.activeElement)).toBe(true);
  });

  it('does not steal focus from the dirty-confirm alertdialog (a content-panel sibling)', () => {
    render(
      <DialogWrapper title="Focus test" onClose={() => {}} isDirty>
        <input data-testid="first-input" />
      </DialogWrapper>,
    );
    // A close attempt on a dirty dialog opens the confirm overlay, whose
    // layout effect focuses the alertdialog wrapper - a SIBLING of the
    // content panel, not a descendant of it.
    fireEvent.click(screen.getByLabelText('Close dialog'));
    const alertWrapper = screen.getByRole('alertdialog').parentElement as HTMLElement;
    expect(alertWrapper).toHaveFocus();

    rafCallbacks.forEach((cb) => cb(0)); // the main dialog's mount rAF fires late

    // Guarding on the inner content panel alone would miss this: the
    // heuristic would run against the (inert) main content and pull focus
    // out of the confirm overlay.
    expect(alertWrapper).toHaveFocus();
  });

  // #576 D20: the X button's aria-label is translated now, so the Priority 4
  // pass ("first enabled button that is NOT the close X") cannot recognise it
  // by its English label. In another language it used to land on the X, the
  // first button in DOM order, instead of the dialog's own first button.
  it('skips the close X in any language (it is found by marker, not by label)', async () => {
    const { default: i18n } = await import('../../i18n');
    i18n.addResourceBundle('xx', 'translation', { dialogs: { dialogWrapper: { closeDialog: 'Fermer' } } }, true, true);
    // jsdom lays nothing out; give every element a box so Priority 4 can pick one.
    vi.spyOn(HTMLElement.prototype, 'getClientRects').mockReturnValue([{}] as unknown as DOMRectList);
    await i18n.changeLanguage('xx');
    try {
      render(
        <DialogWrapper title="Focus test" onClose={() => {}}>
          <button type="button" data-testid="body-button">Go</button>
        </DialogWrapper>,
      );
      expect(screen.getByLabelText('Fermer')).toBeInTheDocument();
      rafCallbacks.forEach((cb) => cb(0));
      expect(screen.getByTestId('body-button')).toHaveFocus();
    } finally {
      vi.restoreAllMocks();
      await i18n.changeLanguage('en');
    }
  });
});
