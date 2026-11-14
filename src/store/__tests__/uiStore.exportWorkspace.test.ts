import { describe, it, expect, beforeEach, vi } from 'vitest';
import { useUIStore, type AppMode } from '../uiStore';
import { useExportStore } from '../exportStore';
import type { DisplaySettings } from '../../types/loading';

const invokeIpcMock = vi.hoisted(() => vi.fn());

vi.mock('../../utils/ipc', () => ({
  invokeIpc: invokeIpcMock,
}));

/**
 * The Export workspace is not in the mode switcher: the project menu and Ctrl+E
 * enter it (`enterExport`) and "Close export" leaves it (`closeExport`) for the
 * mode it was entered from.
 */
describe('Export workspace entry and exit', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue(undefined);
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useUIStore.setState({ exportReturnMode: null });
  });

  it.each<Exclude<AppMode, 'export'>>(['survey', 'data', 'plan'])(
    '%s -> export -> close returns to %s',
    (from) => {
      useUIStore.getState().setAppMode(from);
      expect(useUIStore.getState().appMode).toBe(from);

      useUIStore.getState().enterExport();
      expect(useUIStore.getState().appMode).toBe('export');
      expect(useUIStore.getState().exportReturnMode).toBe(from);

      useUIStore.getState().closeExport();
      expect(useUIStore.getState().appMode).toBe(from);
      expect(useUIStore.getState().exportReturnMode).toBeNull();
    },
  );

  it('closeExport refuses while an export is being written', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().enterExport();
    useExportStore.getState().setExporting(true);
    try {
      useUIStore.getState().closeExport();
      expect(useUIStore.getState().appMode).toBe('export');
      expect(useUIStore.getState().exportReturnMode).toBe('data');
    } finally {
      useExportStore.getState().setExporting(false);
    }
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('data');
  });

  it('closes to Survey when no mode is recorded', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().setAppMode('export');
    useUIStore.setState({ exportReturnMode: null });

    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().exportReturnMode).toBeNull();
  });

  it('enterExport in Export keeps the original return mode', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().enterExport();
    useUIStore.getState().enterExport();
    expect(useUIStore.getState().exportReturnMode).toBe('plan');
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('plan');
  });

  it('closeExport outside Export changes no mode', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('plan');
    expect(useUIStore.getState().exportReturnMode).toBeNull();
  });

  // The agent harness drives `setAppMode('export')` directly; Close export must
  // still go back to where it came from.
  it('a direct setAppMode("export") records the mode it left', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().setAppMode('export');
    expect(useUIStore.getState().exportReturnMode).toBe('data');
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('data');
  });

  it('a direct setAppMode to another mode drops the record', () => {
    useUIStore.getState().setAppMode('data');
    useUIStore.getState().enterExport();
    useUIStore.getState().setAppMode('plan');
    expect(useUIStore.getState().exportReturnMode).toBeNull();

    // Re-entering records Plan, not the stale Data.
    useUIStore.getState().enterExport();
    useUIStore.getState().closeExport();
    expect(useUIStore.getState().appMode).toBe('plan');
  });

  it('a project load clears the record', () => {
    useUIStore.getState().setAppMode('plan');
    useUIStore.getState().enterExport();
    useUIStore.getState().applyDisplaySettings({} as DisplaySettings);
    expect(useUIStore.getState().appMode).toBe('survey');
    expect(useUIStore.getState().exportReturnMode).toBeNull();
  });
});
