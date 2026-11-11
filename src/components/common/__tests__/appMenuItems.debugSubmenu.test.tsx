/**
 * Debug Logs lives in the "Debug" submenu of BOTH hamburgers (home screen and
 * in-project), so it sits in the same place whether or not a project is open.
 */
import { describe, it, expect, vi } from 'vitest';
import { renderHook } from '@testing-library/react';

vi.mock('../../../utils/ipc', () => ({
  invokeIpc: vi.fn(),
  getErrorMessage: (_e: unknown, f?: string) => f ?? 'error',
}));

import { serviceMenuItems } from '../appMenuItems';
import { useProjectMenuItems } from '../../SingleSystemView/useProjectMenuItems';
import type { AppMenuItem } from '../AppMenu';

const t = ((key: string) => key) as unknown as Parameters<typeof serviceMenuItems>[0];
const ids = (items: AppMenuItem[] | undefined) => (items ?? []).map((i) => i.id);

describe('Debug submenu', () => {
  it('home menu: Debug Logs is inside Debug, not a top-level row', () => {
    const onDebug = vi.fn();
    const items = serviceMenuItems(t, { onSettings: vi.fn(), onDebug });
    expect(ids(items)).toEqual(['settings-button', 'debug-menu']);
    const debug = items.find((i) => i.id === 'debug-menu');
    expect(ids(debug?.submenu)).toEqual(['global-debug-button']);
    (debug?.submenu?.[0]?.onClick as () => void)();
    expect(onDebug).toHaveBeenCalledOnce();
  });

  it('in-project menu: Debug Logs leads the Debug submenu, above the stats toggles', () => {
    const { result } = renderHook(() => useProjectMenuItems({ onSave: async () => ({ success: true }) }));
    expect(ids(result.current)).not.toContain('global-debug-button');
    expect(ids(result.current).filter((id) => id === 'debug-menu')).toHaveLength(1);
    const debug = result.current.find((i) => i.id === 'debug-menu');
    expect(ids(debug?.submenu)).toEqual([
      'global-debug-button',
      'debug-menu-sep',
      'display-toggle-showDevStats',
      'display-toggle-showMemoryStats',
      'display-toggle-showMapStats',
      'display-toggle-showZoomStats',
    ]);
  });
});
