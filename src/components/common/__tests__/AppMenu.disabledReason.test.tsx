/**
 * Demo finding N-12: a menu row a read-only state turned off said why only in
 * a native `title`, which WebKitGTK never draws, and a click on it did
 * nothing. A row with a `disabledReason` now shows the reason in the app's
 * Tooltip on hover, and a click shows it as the blocked-edit notice, once,
 * without running the row's action. A row disabled without a reason keeps
 * the plain `disabled` behaviour.
 *
 * Injection: dropping `reason` from AppMenu's MenuRow (the row renders
 * `disabled`, no tooltip) or the `disabledReason` branch of its onClick fails
 * the first case.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import { AppMenu, type AppMenuItem } from '../AppMenu';
import { useUIStore } from '../../../store/uiStore';

const save = vi.fn();
const open = vi.fn();
const debugRow = vi.fn();

const ITEMS: AppMenuItem[] = [
  { id: 'row-save', label: 'Save', onClick: save, disabled: true, disabledReason: 'Synthetic: open read-only.' },
  { id: 'row-plain-off', label: 'Plain', onClick: debugRow, disabled: true },
  { id: 'row-open', label: 'Open', onClick: open },
];

function openMenu(): void {
  render(<AppMenu items={ITEMS} />);
  fireEvent.click(screen.getByTestId('app-menu-trigger'));
}

beforeEach(() => {
  vi.clearAllMocks();
  useUIStore.getState().clearToast();
});

describe('AppMenu rows disabled with a reason', () => {
  it('say why on hover, and a click shows the notice once instead of the action', async () => {
    openMenu();
    const row = screen.getByTestId('row-save');
    expect(row.getAttribute('aria-disabled')).toBe('true');
    expect(row.hasAttribute('disabled')).toBe(false);
    expect(row.getAttribute('title')).toBeNull();

    fireEvent.mouseEnter(row);
    expect((await screen.findByTestId('row-save-reason')).textContent).toBe('Synthetic: open read-only.');

    fireEvent.click(row);
    fireEvent.click(row);
    expect(save).not.toHaveBeenCalled();
    const notices = useUIStore.getState().toasts.filter((toast) => toast.key === 'edit-disabled');
    expect(notices).toHaveLength(1);
    expect(notices[0]?.message).toBe('Synthetic: open read-only.');
    expect(notices[0]?.kind).toBe('warning');
  });

  it('leave a row disabled without a reason as it was', () => {
    openMenu();
    const row = screen.getByTestId('row-plain-off');
    expect(row.hasAttribute('disabled')).toBe(true);
    fireEvent.click(row);
    expect(debugRow).not.toHaveBeenCalled();
    expect(useUIStore.getState().toasts).toHaveLength(0);
    expect(screen.queryByTestId('row-plain-off-reason')).toBeNull();
  });

  it('leave an enabled row with no tooltip, and its click runs the action', () => {
    openMenu();
    const row = screen.getByTestId('row-open');
    fireEvent.mouseEnter(row);
    expect(screen.queryByRole('tooltip')).toBeNull();
    fireEvent.click(row);
    expect(open).toHaveBeenCalledOnce();
  });
});
