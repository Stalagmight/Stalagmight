import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useUIStore } from '../uiStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

/**
 * `openWarningsTab` is the SpeleoDB upload dialog's "Resolve now": it shows the
 * whole Warnings list rather than starting to resolve one issue, and it has to
 * land where the user can see it whatever the workspace was doing - the same
 * reveal `triggerResolveWarning` performs, plus leaving Plan/Export, whose own
 * panels replace the Details panel.
 */
describe('openWarningsTab', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(undefined);
    useUIStore.setState({
      appMode: 'survey',
      geometryMode: 'measured',
      detailPanelTab: 'details',
      detailPanelOpen: false,
      detailPanelMode: 'pin',
      detailRevealNonce: 0,
      requestResolveWarning: null,
      focusedSectionId: null,
      dataTableFullScreen: false,
    });
  });

  it('opens a closed pinned panel on the Warnings tab and saves that layout', () => {
    useUIStore.getState().openWarningsTab();
    const s = useUIStore.getState();
    expect(s.detailPanelTab).toBe('warnings');
    expect(s.detailPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('pin');
    // The list, not one issue's resolve view.
    expect(s.requestResolveWarning).toBeNull();
    expect(invokeIpcMock).toHaveBeenCalled();
  });

  it('floats a hover-mode panel open without pinning it', () => {
    useUIStore.setState({ detailPanelMode: 'hover' });
    useUIStore.getState().openWarningsTab();
    const s = useUIStore.getState();
    expect(s.detailPanelTab).toBe('warnings');
    expect(s.detailRevealNonce).toBe(1);
    expect(s.detailPanelMode).toBe('hover');
    expect(s.detailPanelOpen).toBe(false);
    expect(invokeIpcMock).not.toHaveBeenCalled();
  });

  it.each(['plan', 'export'] as const)('leaves %s for Survey, where the Details panel lives', (mode) => {
    useUIStore.getState().setAppMode(mode);
    useUIStore.getState().openWarningsTab();
    const s = useUIStore.getState();
    expect(s.appMode).toBe('survey');
    expect(s.detailPanelTab).toBe('warnings');
    // Leaving Plan/Export restores the pre-entry docks (closed here); the reveal
    // runs after that restore, so the panel is open at the end.
    expect(s.detailPanelOpen).toBe(true);
  });

  it('leaves Data for Survey: Data renders no Details panel', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().openWarningsTab();
    const s = useUIStore.getState();
    expect(s.appMode).toBe('survey');
    expect(s.detailPanelTab).toBe('warnings');
    // Data leaves the dock flags alone (no snapshot), so the reveal opens it.
    expect(s.detailPanelOpen).toBe(true);
  });

  it('triggerResolveWarning from Data switches to Survey before revealing', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().triggerResolveWarning(3, 7);
    const s = useUIStore.getState();
    expect(s.appMode).toBe('survey');
    expect(s.requestResolveWarning).toEqual({ sectionId: 3, shotId: 7 });
    expect(s.detailPanelTab).toBe('warnings');
    expect(s.detailPanelOpen).toBe(true);
  });

  it('leaves data-table fullscreen, which covers the panel', () => {
    useUIStore.getState().enterDataTableFullScreen();
    useUIStore.getState().openWarningsTab();
    expect(useUIStore.getState().dataTableFullScreen).toBe(false);
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
  });

  it('triggerResolveWarning still asks for one issue through the same reveal', () => {
    useUIStore.getState().triggerResolveWarning(3, 7);
    const s = useUIStore.getState();
    expect(s.requestResolveWarning).toEqual({ sectionId: 3, shotId: 7 });
    expect(s.detailPanelTab).toBe('warnings');
    expect(s.detailPanelOpen).toBe(true);
  });
});
