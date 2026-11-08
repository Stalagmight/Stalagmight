/**
 * [#579 x #576] The "Installing update..." window holds the app still while an
 * update restart installs: a screen occupant (both shortcut hooks stand down), on
 * the dialog stack (the app under it is inert), focused, swallowing every key -
 * and gone, with the app live again, when the install fails.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { act, render, screen } from '@testing-library/react';

import { UpdateInstallOverlay } from '../UpdateInstallOverlay';
import { useUpdateStore } from '../../store/updateStore';
import { useUIStore } from '../../store/uiStore';

const INFO = { version: '9.9.9', notesUrl: 'https://example.test/notes' };

function App() {
  return (
    <>
      <button type="button" data-testid="app-control">
        app
      </button>
      <UpdateInstallOverlay />
    </>
  );
}

let windowKeys: string[];
const onWindowKey = (e: KeyboardEvent) => windowKeys.push(e.key);

beforeEach(() => {
  useUpdateStore.getState().reset();
  const store = useUpdateStore.getState();
  store.setAvailable(store.beginCheck(), INFO);
  useUIStore.setState({ screenOccupants: 0 });
  windowKeys = [];
  window.addEventListener('keydown', onWindowKey);
});

afterEach(() => {
  window.removeEventListener('keydown', onWindowKey);
});

describe('UpdateInstallOverlay', () => {
  it('shows nothing and holds nothing while no install runs', () => {
    const { container } = render(<App />);
    expect(screen.queryByTestId('update-install-overlay')).toBeNull();
    expect(useUIStore.getState().screenOccupants).toBe(0);
    expect(container.hasAttribute('inert')).toBe(false);
  });

  it('holds the screen and the keyboard while the install runs, and lets go when it fails', () => {
    const { container } = render(<App />);
    screen.getByTestId('app-control').focus();

    act(() => useUpdateStore.getState().setInstalling(true));

    const overlay = screen.getByTestId('update-install-overlay');
    expect(screen.getByTestId('update-install-panel')).toHaveTextContent('9.9.9');
    // On the dialog stack: the app under it is inert, the window is outside it.
    expect(document.getElementById('dialog-root')?.contains(overlay)).toBe(true);
    expect(container.hasAttribute('inert')).toBe(true);
    // A screen occupant: the shortcut hooks return before acting on any key.
    expect(useUIStore.getState().screenOccupants).toBe(1);
    // Focus moved off the app, onto the window...
    expect(document.activeElement).toBe(overlay);
    // ...which swallows every key: none reaches a window-level shortcut handler.
    for (const key of ['Delete', 'z', 'Escape', 'Tab']) {
      const ev = new KeyboardEvent('keydown', { key, ctrlKey: key === 'z', bubbles: true, cancelable: true });
      overlay.dispatchEvent(ev);
      expect(ev.defaultPrevented).toBe(true);
    }
    expect(windowKeys).toEqual([]);

    act(() => useUpdateStore.getState().setInstalling(false));

    expect(screen.queryByTestId('update-install-overlay')).toBeNull();
    expect(container.hasAttribute('inert')).toBe(false);
    expect(useUIStore.getState().screenOccupants).toBe(0);
  });

  // [#576 merge] The loading window's and the integrity dialog's conventions: the
  // focused element is the dialog, named; focus goes back where it was; and the
  // window shows even without `#dialog-root`, never holding the screen unseen.
  it('is focused as a named alert dialog and gives focus back when the install fails', () => {
    render(<App />);
    const control = screen.getByTestId('app-control');
    control.focus();

    act(() => useUpdateStore.getState().setInstalling(true));

    const dialog = screen.getByRole('alertdialog', { name: /9\.9\.9/ });
    expect(dialog).toBe(screen.getByTestId('update-install-overlay'));
    expect(document.activeElement).toBe(dialog);
    expect(dialog.getAttribute('aria-modal')).toBe('true');
    expect(dialog.getAttribute('aria-busy')).toBe('true');

    act(() => useUpdateStore.getState().setInstalling(false));

    expect(document.activeElement).toBe(control);
  });

  it('shows over document.body when there is no #dialog-root', () => {
    const root = document.getElementById('dialog-root');
    root?.remove();
    try {
      render(<App />);
      act(() => useUpdateStore.getState().setInstalling(true));

      const overlay = screen.getByTestId('update-install-overlay');
      expect(overlay.parentElement).toBe(document.body);
      expect(document.activeElement).toBe(overlay);
      expect(useUIStore.getState().screenOccupants).toBe(1);

      act(() => useUpdateStore.getState().setInstalling(false));
      expect(screen.queryByTestId('update-install-overlay')).toBeNull();
    } finally {
      if (root) document.body.appendChild(root);
    }
  });
});
