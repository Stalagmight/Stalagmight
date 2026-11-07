/**
 * The status button's right-click menu. The point of the surface is that it is the
 * SAME rows as the hamburger's SpeleoDB submenu reached by a shorter gesture, so the
 * id parity below is the load-bearing assertion: a row added to `useSpeleoMenuItems`
 * appears here without anyone remembering to wire it, and one selector reaches either
 * surface.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('error'));

import { SpeleoStatusButton } from '../SpeleoStatusButton';
import { useProjectMenuItems } from '../useProjectMenuItems';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { stubGetErrorMessage, fallbackOr } from '../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../test/factories/speleo';

const LINK: SpeleoLinkState = makeSpeleoLink({ forceEditAllowed: true });

let openDialogMock: ReturnType<typeof vi.fn>;

/** The ids the HAMBURGER would render for the same link state. */
function hamburgerSubmenuIds(): string[] {
  const { result } = renderHook(() =>
    useProjectMenuItems({ onSave: async () => ({ success: true }) }),
  );
  const row = result.current.find((it) => it.id === 'speleodb-menu');
  return (row?.submenu ?? []).map((it) => it.id);
}

const openMenu = async () => {
  render(<SpeleoStatusButton />);
  fireEvent.contextMenu(screen.getByTestId('speleodb-status-button'));
  return screen.findByTestId('speleodb-status-menu');
};

beforeEach(() => {
  openDialogMock = vi.fn();
  useSpeleoStore.setState({
    link: LINK,
    remoteProjectId: 'p1',
    divergence: null,
    busy: false,
    session: { loggedIn: true, userEmail: 'me@x', endpoint: 'https://x/', keychainBacked: true, storage: 'keychain', keychain: 'available' },
  });
  useUIStore.setState({ appMode: 'survey', openDialog: openDialogMock });
});

describe('SpeleoStatusMenu', () => {
  it('opens on a right-click of the status button and is closed before it', async () => {
    render(<SpeleoStatusButton />);
    expect(screen.queryByTestId('speleodb-status-menu')).not.toBeInTheDocument();
    fireEvent.contextMenu(screen.getByTestId('speleodb-status-button'));
    expect(await screen.findByTestId('speleodb-status-menu')).toHaveAttribute('role', 'menu');
  });

  it('renders exactly the hamburger submenu ids, in order', async () => {
    const expected = hamburgerSubmenuIds();
    const menu = await openMenu();
    // Separators render as bare rules, so compare the actionable rows.
    const rendered = Array.from(menu.querySelectorAll('[role="menuitem"]')).map((el) =>
      el.getAttribute('data-testid'),
    );
    expect(rendered).toEqual(expected.filter((id) => !id.includes('-sep-')));
  });

  it('closes on Escape', async () => {
    const menu = await openMenu();
    fireEvent.keyDown(menu, { key: 'Escape' });
    await waitFor(() =>
      expect(screen.queryByTestId('speleodb-status-menu')).not.toBeInTheDocument(),
    );
  });

  it('closes on an outside mousedown', async () => {
    await openMenu();
    fireEvent.mouseDown(document.body);
    await waitFor(() =>
      expect(screen.queryByTestId('speleodb-status-menu')).not.toBeInTheDocument(),
    );
  });

  it('runs a row action and closes', async () => {
    await openMenu();
    await userEvent.click(screen.getByTestId('speleodb-menu-project-settings'));
    expect(openDialogMock).toHaveBeenCalledWith('speleodb-project');
    await waitFor(() =>
      expect(screen.queryByTestId('speleodb-status-menu')).not.toBeInTheDocument(),
    );
  });

  it('moves the highlight with the arrow keys and activates with Enter', async () => {
    const menu = await openMenu();
    // Read-only + opted-in: lock, push (disabled), refresh, edit-without-lock, then
    // the two destinations. ArrowDown skips the disabled push row.
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'ArrowDown' });
    fireEvent.keyDown(menu, { key: 'Enter' });
    expect(openDialogMock).toHaveBeenCalledWith('speleodb-force-edit');
  });
});
