import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useUIStore } from '../uiStore';
import { invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');

/**
 * Entering Plan/Export force-opens the right Dock (detail panel). #241: exiting
 * back to Survey must restore whatever dock state the user had BEFORE entering -
 * mirroring the existing prePlanGeometryMode/prePlanDisplay snapshot/restore
 * pattern - rather than leaving a dock the user had closed stranded open.
 * Since #395 (staged entry) Plan no longer pins the bottom Dock at entry time:
 * SingleSystemView opens it when the origin is chosen (an inlined, unpersisted
 * setState, exactly like the entry-time force-open). The snapshot/restore must
 * cover that mid-Plan forced open the same way.
 */
describe('Plan/Export dock snapshot/restore (#241)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(undefined);
    useUIStore.setState({
      appMode: 'survey',
      geometryMode: 'measured',
      detailPanelOpen: false,
      detailPanelMode: 'pin',
      dataTableOpen: false,
      dataTableMode: 'pin',
    });
  });

  it('restores a closed detail panel after leaving Plan', () => {
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    useUIStore.getState().setAppMode('plan');
    // Plan force-opens it.
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
  });

  it('leaves the data table closed at Plan entry (staged entry, #395)', () => {
    expect(useUIStore.getState().dataTableOpen).toBe(false);
    useUIStore.getState().setAppMode('plan');
    // No origin chosen yet: the bottom Dock stays closed (and is hidden by
    // SingleSystemView) so the entry card is not paired with an empty drawer.
    expect(useUIStore.getState().dataTableOpen).toBe(false);
  });

  it('restores a closed data table after leaving Plan, including the origin-chosen forced open', () => {
    expect(useUIStore.getState().dataTableOpen).toBe(false);
    useUIStore.getState().setAppMode('plan');
    // SingleSystemView pins the Dock open when the origin is chosen - same
    // inlined unpersisted setState it performs in its originChosen effect.
    useUIStore.setState({ dataTableOpen: true, dataTableMode: 'pin' });
    expect(useUIStore.getState().dataTableOpen).toBe(true);
    useUIStore.getState().setAppMode('survey');
    // The pre-Plan snapshot (closed) wins over the mid-Plan forced open.
    expect(useUIStore.getState().dataTableOpen).toBe(false);
  });

  it('restores the prior detail panel mode (e.g. hover), not just openness', () => {
    useUIStore.setState({ detailPanelOpen: true, detailPanelMode: 'hover' });
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().detailPanelMode).toBe('hover');
  });

  it('restores docks after leaving Export too', () => {
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    useUIStore.getState().setAppMode('export');
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
  });

  it('leaves an already-open dock open and untouched after Plan', () => {
    useUIStore.setState({ dataTableOpen: true, dataTableMode: 'pin', detailPanelOpen: true, detailPanelMode: 'pin' });
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().dataTableOpen).toBe(true);
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
  });

  it('a project load is authoritative: drops the stale dock snapshot without restoring it', () => {
    // Project A: closes the docks, enters Plan (captures the closed snapshot). A DIRECT
    // switch to project B (applyDisplaySettings) must clear the stale snapshot so the
    // later exitGasPlan funnel can't restore A's closed docks over B's live state.
    useUIStore.getState().setAppMode('plan');
    useUIStore.setState({ detailPanelOpen: true, dataTableOpen: true });
    useUIStore.getState().applyDisplaySettings({ geometryMode: 'measured' } as never);
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    expect(useUIStore.getState().dataTableOpen).toBe(true);
    // The leftover funnel is now a no-op (already survey) - B's dock state stays live.
    useUIStore.getState().exitGasPlan();
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    expect(useUIStore.getState().dataTableOpen).toBe(true);
  });

  // #241 must-fix: the entry-time force-open must never persist. Otherwise a user
  // who had pinned the detail panel closed would find it reopened on next launch,
  // because Plan/Export force it open in memory but savePanelLayout would have
  // written that forced-open state to disk.
  it('never persists the entry-time forced-open panel, in pin mode', () => {
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    expect(invokeIpcMock).not.toHaveBeenCalledWith('update_panel_layout', expect.anything(), expect.anything());
  });

  // #241: pins the snapshot-after-exits ordering - entering Plan while section-focused
  // must restore the PRE-FOCUS dock state on the way back to Survey, not the
  // focus-forced (collapsed) one exitSectionFocus itself just restored.
  it('restores the pre-focus dock state, not the focus-forced state, when Plan is entered while focused', () => {
    useUIStore.setState({ detailPanelOpen: true, detailPanelMode: 'pin', dataTableOpen: false, dataTableMode: 'pin' });
    useUIStore.getState().enterSectionFocus(1, 300);
    // Focus mode force-collapses the detail panel.
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('survey');
    // Restored to the pre-focus (open) state, not the focus-forced (closed) one.
    expect(useUIStore.getState().detailPanelOpen).toBe(true);
    expect(useUIStore.getState().focusedSectionId).toBeNull();
  });

  // #241: pins the appMode==='survey' snapshot guard - a Plan -> Export switch must
  // not overwrite the real pre-Plan snapshot with Export's (already-forced) state.
  it('restores the original survey dock state across a survey -> plan -> export -> survey chain', () => {
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    expect(useUIStore.getState().dataTableOpen).toBe(false);
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('export');
    useUIStore.getState().setAppMode('survey');
    expect(useUIStore.getState().detailPanelOpen).toBe(false);
    expect(useUIStore.getState().dataTableOpen).toBe(false);
  });
});
