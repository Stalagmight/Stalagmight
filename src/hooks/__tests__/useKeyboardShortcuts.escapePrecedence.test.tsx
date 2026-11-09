/**
 * Escape precedence for the armed gas-plan origin picker (#395).
 *
 * The origin picker's Escape branch runs before the open-dialog handling, so it
 * must not swallow Escape while a dialog is open - the dialog's own close wins,
 * and the picker stays armed underneath it.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { render, renderHook } from '@testing-library/react';
import { DialogWrapper } from '../../components/Dialogs/DialogWrapper';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useGasPlanStore } from '../../store/gasPlanStore';

// useKeyboardShortcuts pulls in telemetry/save helpers that ultimately call the
// Tauri IPC bridge; stub it so importing the real hook here stays offline.
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

function pressEscape(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }));
}

beforeEach(() => {
  useUIStore.setState({
    selection: null,
    activeDialog: null,
    dialogData: {},
    placementMode: null,
    dataTableFullScreen: false,
    activeTool: 'select',
    appMode: 'survey',
    geometryMode: 'measured',
  });
  useGasPlanStore.setState({ originPickArmed: false });
});

describe('useKeyboardShortcuts Escape precedence (#395)', () => {
  it('disarms the origin picker on Escape when no dialog is open', () => {
    useGasPlanStore.setState({ originPickArmed: true });
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useGasPlanStore.getState().originPickArmed).toBe(false);
  });

  it('lets an open dialog win Escape, leaving the origin picker armed', () => {
    useGasPlanStore.setState({ originPickArmed: true });
    useUIStore.setState({ activeDialog: 'settings' });
    renderHook(() => useKeyboardShortcuts());
    pressEscape();
    expect(useGasPlanStore.getState().originPickArmed).toBe(true);
    expect(useUIStore.getState().activeDialog).toBeNull();
  });
});


it('routes Escape from outside the dialog through its dismissal guard', () => {
  const guardedDismiss = vi.fn();
  useUIStore.setState({ activeDialog: 'speleodb-close' });
  renderHook(() => useKeyboardShortcuts());
  render(<DialogWrapper title="Synthetic busy operation" onClose={guardedDismiss}><p>Busy</p></DialogWrapper>);
  pressEscape();
  expect(guardedDismiss).toHaveBeenCalledOnce();
  expect(useUIStore.getState().activeDialog).toBe('speleodb-close');
});
