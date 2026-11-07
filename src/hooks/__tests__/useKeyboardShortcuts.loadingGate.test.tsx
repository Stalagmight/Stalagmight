/**
 * [#576 W54-NF2] The display-toggle shortcuts during a project load.
 *
 * A load started from an open project (Ctrl+O or drag-drop, then Discard in the
 * unsaved-changes prompt) runs with the project view still mounted: the loading
 * modal lives in the project list, so no overlay, dialog or screen occupant is
 * up, and this window-level handler took Shift+G and friends. The toggle changed
 * the outgoing project's display mid-switch. While a load is in progress they
 * now do nothing; once it ended (a failed one leaves its error in the loading
 * state and the old project open) they work again.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { renderHook } from '@testing-library/react';
import { useKeyboardShortcuts } from '../useKeyboardShortcuts';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { initialLoadingState } from '../../store/projectLoadingState';

function pressShiftG(): void {
  window.dispatchEvent(new KeyboardEvent('keydown', { key: 'G', shiftKey: true, bubbles: true }));
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
    showGrid: false,
  });
  useProjectStore.setState({ loading: { ...initialLoadingState } });
});

describe('useKeyboardShortcuts display toggles during a load (#576 W54-NF2)', () => {
  it('toggles the grid with no load in progress', () => {
    renderHook(() => useKeyboardShortcuts());
    pressShiftG();
    expect(useUIStore.getState().showGrid).toBe(true);
  });

  it('does nothing while a load is in progress', () => {
    useProjectStore.setState({
      loading: { ...initialLoadingState, isLoading: true, clientOpId: 7, projectName: 'CaveB', overallProgress: 40 },
    });
    renderHook(() => useKeyboardShortcuts());
    pressShiftG();
    expect(useUIStore.getState().showGrid).toBe(false);
  });

  it('toggles again once the load failed, with the old project still open', () => {
    useProjectStore.setState({
      loading: {
        ...initialLoadingState,
        isLoading: true,
        clientOpId: 7,
        projectName: 'CaveB',
        overallProgress: 40,
        error: { message: 'could not open' },
      },
    });
    renderHook(() => useKeyboardShortcuts());
    pressShiftG();
    expect(useUIStore.getState().showGrid).toBe(true);
  });
});
