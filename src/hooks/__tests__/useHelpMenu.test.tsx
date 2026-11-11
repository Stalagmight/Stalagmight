/**
 * #665: the macOS Help menu's "Keyboard Shortcuts" entry emits an event that
 * this hook turns into the shortcuts dialog unless something already owns the
 * screen.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => new Map<string, () => void>());
const unlisten = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (name: string, cb: () => void) => {
    listeners.set(name, cb);
    return unlisten;
  }),
}));

import { useHelpMenu, SHOW_SHORTCUTS_EVENT } from '../useHelpMenu';
import { useUIStore, type PlacementModeState } from '../../store/uiStore';
import { readFileSync } from 'fs';
import { join } from 'path';

function Harness() {
  useHelpMenu();
  return null;
}

async function mountAndFire() {
  const view = render(<Harness />);
  await vi.waitFor(() => expect(listeners.has(SHOW_SHORTCUTS_EVENT)).toBe(true));
  listeners.get(SHOW_SHORTCUTS_EVENT)?.();
  return view;
}

describe('useHelpMenu', () => {
  beforeEach(() => {
    listeners.clear();
    unlisten.mockClear();
    useUIStore.setState({
      activeDialog: null,
      placementMode: null,
      dataTableFullScreen: false,
      screenOccupants: 0,
    });
  });

  it('opens the keyboard shortcuts dialog when the menu event arrives', async () => {
    await mountAndFire();
    expect(useUIStore.getState().activeDialog).toBe('keyboard-shortcuts');
  });

  it('does not replace a dialog that is already open', async () => {
    useUIStore.setState({ activeDialog: 'settings' });
    await mountAndFire();
    expect(useUIStore.getState().activeDialog).toBe('settings');
  });

  it('does nothing during a full-screen data table, like the ? key', async () => {
    useUIStore.setState({ dataTableFullScreen: true });
    await mountAndFire();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('does nothing during station placement', async () => {
    useUIStore.setState({ placementMode: {} as PlacementModeState });
    await mountAndFire();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('does nothing while an overlay is waiting for an answer', async () => {
    useUIStore.setState({ screenOccupants: 1 });
    await mountAndFire();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('listens for the event name the Rust menu sends', () => {
    const rust = readFileSync(join(__dirname, '../../../src-tauri/src/app_menu.rs'), 'utf-8');
    expect(rust).toContain(`const SHOW_SHORTCUTS_EVENT: &str = "${SHOW_SHORTCUTS_EVENT}";`);
  });

  it('stops listening on unmount', async () => {
    const view = await mountAndFire();
    view.unmount();
    await vi.waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });
});
