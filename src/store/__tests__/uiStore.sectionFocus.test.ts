/**
 * Unit tests for section-focus mode in uiStore (#50), including the reconcile
 * with #70's closure-adjusted geometry view.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCoreResolving({}));

import { useUIStore } from '../uiStore';

const FOCUS_HEIGHT = 300;

describe('uiStore section focus (#50)', () => {
  beforeEach(() => {
    // Reset the slice these tests touch to a known pre-focus layout.
    useUIStore.setState({
      focusedSectionId: null,
      sectionsPanelOpen: true,
      sectionsPanelMode: 'pin',
      detailPanelOpen: true,
      detailPanelMode: 'pin',
      dataTableOpen: false,
      dataTableMode: 'pin',
      dataTableHeight: 200,
      geometryMode: 'measured',
      selection: null,
    });
  });

  it('enterSectionFocus collapses side panels and opens the data table', () => {
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    const s = useUIStore.getState();
    expect(s.focusedSectionId).toBe(7);
    expect(s.sectionsPanelOpen).toBe(false);
    expect(s.detailPanelOpen).toBe(false);
    expect(s.dataTableOpen).toBe(true);
    // table was closed -> seeds the focus height
    expect(s.dataTableHeight).toBe(FOCUS_HEIGHT);
  });

  it('entering focus leaves the read-only closure-adjusted view for measured', () => {
    useUIStore.setState({ geometryMode: 'closureAdjusted' });
    useUIStore.getState().enterSectionFocus(3, FOCUS_HEIGHT);
    expect(useUIStore.getState().geometryMode).toBe('measured');
  });

  it('entering focus when already measured leaves geometryMode unchanged', () => {
    useUIStore.getState().enterSectionFocus(3, FOCUS_HEIGHT);
    expect(useUIStore.getState().geometryMode).toBe('measured');
  });

  it('exitSectionFocus restores the pre-focus panel layout', () => {
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    useUIStore.getState().exitSectionFocus();
    const s = useUIStore.getState();
    expect(s.focusedSectionId).toBeNull();
    expect(s.sectionsPanelOpen).toBe(true);
    expect(s.detailPanelOpen).toBe(true);
    expect(s.dataTableOpen).toBe(false);
    expect(s.dataTableHeight).toBe(200);
  });

  it('setFocusedSection follows the selection while focused', () => {
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    useUIStore.getState().setFocusedSection(9);
    expect(useUIStore.getState().focusedSectionId).toBe(9);
    // panels stay collapsed (it does not re-enter / re-snapshot)
    expect(useUIStore.getState().sectionsPanelOpen).toBe(false);
  });

  it('setFocusedSection is a no-op when not focused', () => {
    useUIStore.getState().setFocusedSection(9);
    expect(useUIStore.getState().focusedSectionId).toBeNull();
  });

  it('re-focusing another section keeps the original pre-focus snapshot', () => {
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // simulate a second enter (e.g., via another path) - must not overwrite snapshot
    useUIStore.getState().enterSectionFocus(8, 999);
    useUIStore.getState().exitSectionFocus();
    const s = useUIStore.getState();
    expect(s.sectionsPanelOpen).toBe(true);
    expect(s.dataTableOpen).toBe(false);
    expect(s.dataTableHeight).toBe(200);
  });

  it('focus pins the data table open and restores prior panel modes on exit', () => {
    // Pre-focus: data table in hover mode (closed at rest).
    useUIStore.setState({ dataTableMode: 'hover', sectionsPanelMode: 'hover' });
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // Focus forces the data table pinned-open; side modes are untouched on enter.
    expect(useUIStore.getState().dataTableMode).toBe('pin');
    expect(useUIStore.getState().dataTableOpen).toBe(true);
    useUIStore.getState().exitSectionFocus();
    const s = useUIStore.getState();
    expect(s.dataTableMode).toBe('hover');
    expect(s.sectionsPanelMode).toBe('hover');
    expect(s.dataTableOpen).toBe(false);
  });

  it('a manual panel toggle drops focus mode', () => {
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    useUIStore.getState().toggleSectionsPanel();
    expect(useUIStore.getState().focusedSectionId).toBeNull();
  });

  it('toggling one panel out of focus restores the OTHER panels pre-focus layout', () => {
    // Distinct pre-focus layout: detail open+hover, sections open+pin, data closed.
    useUIStore.setState({
      sectionsPanelOpen: true,
      sectionsPanelMode: 'pin',
      detailPanelOpen: true,
      detailPanelMode: 'hover',
      dataTableOpen: false,
      dataTableMode: 'pin',
    });
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // Focus forces every panel collapsed/pinned (table open+pin); the real layout
    // is snapshotted. Toggling the data table exits focus and persists the layout.
    useUIStore.getState().toggleDataTable();
    const s = useUIStore.getState();
    expect(s.focusedSectionId).toBeNull();
    // Regression guard (#50): the untouched panels must reflect the user's
    // pre-focus values, NOT the focus-forced collapse/pin that savePanelLayout
    // would otherwise persist as the user's layout.
    expect(s.detailPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('hover');
    expect(s.sectionsPanelOpen).toBe(true);
    expect(s.sectionsPanelMode).toBe('pin');
    // The toggled panel reflects the toggle (table was force-open → closes, pinned).
    expect(s.dataTableOpen).toBe(false);
    expect(s.dataTableMode).toBe('pin');
  });

  it('setPanelOpen out of focus restores the OTHER panels and clears focus', () => {
    // Distinct pre-focus layout: sections open+pin, detail open+hover, data closed.
    useUIStore.setState({
      sectionsPanelOpen: true,
      sectionsPanelMode: 'pin',
      detailPanelOpen: true,
      detailPanelMode: 'hover',
      dataTableOpen: false,
      dataTableMode: 'pin',
    });
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // Open the sections panel via setPanelOpen - this exits focus and persists.
    useUIStore.getState().setPanelOpen('sections', true);
    const s = useUIStore.getState();
    // (b) focus cleared
    expect(s.focusedSectionId).toBeNull();
    // The targeted panel reflects the call.
    expect(s.sectionsPanelOpen).toBe(true);
    // (a) the OTHER panels are restored to their pre-focus snapshot values, NOT
    // the focus-forced collapse/pin.
    expect(s.detailPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('hover');
    expect(s.dataTableOpen).toBe(false);
    expect(s.dataTableMode).toBe('pin');
  });

  it('setPanelMode out of focus restores the OTHER panels and clears focus', () => {
    // Distinct pre-focus layout: sections open+hover, detail open+pin, data open+pin.
    useUIStore.setState({
      sectionsPanelOpen: true,
      sectionsPanelMode: 'hover',
      detailPanelOpen: true,
      detailPanelMode: 'pin',
      dataTableOpen: true,
      dataTableMode: 'pin',
    });
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // Pin the detail panel via setPanelMode - this exits focus and persists.
    useUIStore.getState().setPanelMode('detail', 'pin');
    const s = useUIStore.getState();
    // (b) focus cleared
    expect(s.focusedSectionId).toBeNull();
    // The targeted panel reflects the call (pin → open).
    expect(s.detailPanelMode).toBe('pin');
    expect(s.detailPanelOpen).toBe(true);
    // (a) the OTHER panels are restored to their pre-focus snapshot values.
    expect(s.sectionsPanelOpen).toBe(true);
    expect(s.sectionsPanelMode).toBe('hover');
    expect(s.dataTableOpen).toBe(true);
    expect(s.dataTableMode).toBe('pin');
  });

  it('toggleDataTable out of focus restores the OTHER panels and clears focus', () => {
    // Distinct pre-focus layout: sections closed+hover, detail open+pin, data open+hover.
    useUIStore.setState({
      sectionsPanelOpen: false,
      sectionsPanelMode: 'hover',
      detailPanelOpen: true,
      detailPanelMode: 'pin',
      dataTableOpen: true,
      dataTableMode: 'hover',
    });
    useUIStore.getState().enterSectionFocus(7, FOCUS_HEIGHT);
    // Toggle the data table - this exits focus and persists.
    useUIStore.getState().toggleDataTable();
    const s = useUIStore.getState();
    // (b) focus cleared
    expect(s.focusedSectionId).toBeNull();
    // (a) the OTHER panels are restored to their pre-focus snapshot values, NOT
    // the focus-forced collapse/pin.
    expect(s.sectionsPanelOpen).toBe(false);
    expect(s.sectionsPanelMode).toBe('hover');
    expect(s.detailPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('pin');
  });

  it('toggling a panel when NOT focused only changes that panel', () => {
    // The snapshot spread must be a no-op when there is no focus to exit.
    useUIStore.setState({
      sectionsPanelOpen: true,
      sectionsPanelMode: 'pin',
      detailPanelOpen: true,
      detailPanelMode: 'hover',
      dataTableOpen: false,
      dataTableMode: 'pin',
    });
    useUIStore.getState().toggleSectionsPanel();
    const s = useUIStore.getState();
    expect(s.sectionsPanelOpen).toBe(false);
    expect(s.sectionsPanelMode).toBe('pin');
    // Other panels untouched.
    expect(s.detailPanelOpen).toBe(true);
    expect(s.detailPanelMode).toBe('hover');
    expect(s.dataTableOpen).toBe(false);
  });
});
