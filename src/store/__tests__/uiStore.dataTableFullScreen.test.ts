/**
 * Unit tests for the data-table fullscreen mode in uiStore. The mode is
 * session-only (never persisted) and owns the raw-table selection used by
 * the fullscreen raw sqlite viewer.
 */
import { describe, it, expect, beforeEach } from 'vitest';

import { useUIStore } from '../uiStore';

describe('uiStore data-table fullscreen', () => {
  beforeEach(() => {
    useUIStore.setState({
      view: 'single',
      dataTableFullScreen: false,
      rawTableSelection: null,
      dataTableOpen: false,
      dataTableMode: 'pin',
      focusedSectionId: null,
    });
  });

  it('enterDataTableFullScreen forces the data table open', () => {
    useUIStore.getState().enterDataTableFullScreen();
    const s = useUIStore.getState();
    expect(s.dataTableFullScreen).toBe(true);
    expect(s.dataTableOpen).toBe(true);
  });

  it('entering with the table already open keeps it open', () => {
    useUIStore.setState({ dataTableOpen: true });
    useUIStore.getState().enterDataTableFullScreen();
    expect(useUIStore.getState().dataTableOpen).toBe(true);
  });

  it('exitDataTableFullScreen restores the prior (closed) state and clears the raw selection', () => {
    // Entered from closed (beforeEach default): exit restores it to closed
    // rather than stranding the table open.
    useUIStore.getState().enterDataTableFullScreen();
    useUIStore.getState().setRawTableSelection('shots');
    useUIStore.getState().exitDataTableFullScreen();
    const s = useUIStore.getState();
    expect(s.dataTableFullScreen).toBe(false);
    expect(s.rawTableSelection).toBeNull();
    expect(s.dataTableOpen).toBe(false);
  });

  it('exitDataTableFullScreen restores a hover-mode table instead of leaving it pinned open', () => {
    useUIStore.setState({ dataTableOpen: false, dataTableMode: 'hover' });
    useUIStore.getState().enterDataTableFullScreen();
    // Fullscreen forces the table open + pinned while active.
    expect(useUIStore.getState().dataTableOpen).toBe(true);
    expect(useUIStore.getState().dataTableMode).toBe('pin');
    useUIStore.getState().exitDataTableFullScreen();
    const s = useUIStore.getState();
    expect(s.dataTableOpen).toBe(false);
    expect(s.dataTableMode).toBe('hover');
  });

  it('exitDataTableFullScreen keeps a table that was pinned-open before', () => {
    useUIStore.setState({ dataTableOpen: true, dataTableMode: 'pin' });
    useUIStore.getState().enterDataTableFullScreen();
    useUIStore.getState().exitDataTableFullScreen();
    const s = useUIStore.getState();
    expect(s.dataTableOpen).toBe(true);
    expect(s.dataTableMode).toBe('pin');
  });

  it('exitDataTableFullScreen is a no-op when not fullscreen', () => {
    useUIStore.setState({ rawTableSelection: 'shots' });
    useUIStore.getState().exitDataTableFullScreen();
    // Not in the mode → nothing is touched (no surprise raw-selection reset).
    expect(useUIStore.getState().rawTableSelection).toBe('shots');
  });

  it('setRawTableSelection stores and clears the table name', () => {
    useUIStore.getState().setRawTableSelection('sections');
    expect(useUIStore.getState().rawTableSelection).toBe('sections');
    useUIStore.getState().setRawTableSelection(null);
    expect(useUIStore.getState().rawTableSelection).toBeNull();
  });

  it('setView exits fullscreen mode (cannot strand it across views)', () => {
    useUIStore.getState().enterDataTableFullScreen();
    useUIStore.getState().setRawTableSelection('shots');
    useUIStore.getState().setView('global');
    const s = useUIStore.getState();
    expect(s.dataTableFullScreen).toBe(false);
    expect(s.rawTableSelection).toBeNull();
  });

  // Regression (#3): fullscreen entered WHILE section-focused must snapshot the
  // user's real pre-focus table state, not the focus-forced open+pin - otherwise a
  // hover/closed table is stranded pinned-open once both focus and fullscreen exit.
  describe('fullscreen entered while section-focused', () => {
    beforeEach(() => {
      // A full pre-focus layout so enterSectionFocus has something to snapshot.
      useUIStore.setState({
        focusedSectionId: null,
        sectionsPanelOpen: true,
        sectionsPanelMode: 'pin',
        detailPanelOpen: true,
        detailPanelMode: 'pin',
        dataTableOpen: false,
        dataTableMode: 'hover',
        dataTableHeight: 200,
        dataTableFullScreen: false,
        geometryMode: 'measured',
      });
    });

    it('exit-focus then exit-fullscreen restores the real hover/closed table (not pinned-open)', () => {
      useUIStore.getState().enterSectionFocus(7, 300);
      expect(useUIStore.getState().dataTableMode).toBe('pin'); // focus forces pin
      useUIStore.getState().enterDataTableFullScreen();
      useUIStore.getState().exitSectionFocus();
      useUIStore.getState().exitDataTableFullScreen();
      const s = useUIStore.getState();
      expect(s.dataTableOpen).toBe(false);
      expect(s.dataTableMode).toBe('hover');
    });

    it('exiting fullscreen while still focused keeps the table open+pinned (focus needs it)', () => {
      useUIStore.getState().enterSectionFocus(7, 300);
      useUIStore.getState().enterDataTableFullScreen();
      useUIStore.getState().exitDataTableFullScreen();
      const mid = useUIStore.getState();
      expect(mid.dataTableFullScreen).toBe(false);
      expect(mid.dataTableOpen).toBe(true);
      expect(mid.dataTableMode).toBe('pin');
      // Then leaving focus restores the user's real hover/closed preference.
      useUIStore.getState().exitSectionFocus();
      const s = useUIStore.getState();
      expect(s.dataTableOpen).toBe(false);
      expect(s.dataTableMode).toBe('hover');
    });
  });
});
