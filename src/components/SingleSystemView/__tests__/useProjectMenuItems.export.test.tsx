/**
 * The hamburger's Export row is the Export workspace's menu entry: outside
 * Export it enters the workspace, inside it reads "Close export" and returns to
 * the mode Export was entered from. The modal Export dialog is gone, so the row
 * must never open a dialog.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { act, renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn().mockResolvedValue(undefined),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { useProjectMenuItems } from '../useProjectMenuItems';
import { useUIStore, type AppMode } from '../../../store/uiStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import type { AppMenuItem } from '../../common/AppMenu';

function exportRow(): AppMenuItem {
  const { result } = renderHook(() => useProjectMenuItems({ onSave: async () => ({ success: true }) }));
  const row = result.current.find((it) => it.id === 'toolbar-export-button');
  if (!row) throw new Error('no Export row');
  return row;
}

beforeEach(() => {
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured', activeDialog: null });
  useUIStore.setState({ exportReturnMode: null });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', session: null });
});

describe('useProjectMenuItems Export row', () => {
  it('reads "Export" and keeps the Ctrl+E hint outside Export', () => {
    const row = exportRow();
    expect(row.label).toBe('Export');
    expect(row.action).toBe('export');
  });

  it.each<Exclude<AppMode, 'export'>>(['survey', 'data', 'plan'])(
    'enters the Export workspace from %s, then closes back to it',
    (from) => {
      useUIStore.getState().setAppMode(from);
      const enterRow = exportRow();
      act(() => enterRow.onClick?.());
      expect(useUIStore.getState().appMode).toBe('export');
      expect(useUIStore.getState().activeDialog).toBeNull();

      const closeRow = exportRow();
      expect(closeRow.label).toBe('Close export');
      expect(closeRow.action).toBe('export');
      act(() => closeRow.onClick?.());
      expect(useUIStore.getState().appMode).toBe(from);
      expect(useUIStore.getState().activeDialog).toBeNull();
    },
  );
});
