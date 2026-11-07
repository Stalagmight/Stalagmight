/**
 * #576 D27-32(b): projectStore and speleoStore load in either order, and
 * projectStore's SpeleoDB calls still reach the SpeleoDB store.
 *
 * They used to import each other's values. Then the order the app happened to
 * import them in decided whether a top-level read of a cross-import saw the
 * value or a dead binding (one planted read crashed the "speleoStore first"
 * order). projectStore now reaches speleoStore through speleoStoreHandle, which
 * speleoStore registers into; `pnpm lint:import-cycles` keeps the loop out.
 *
 * Each test loads the modules into a fresh registry (vi.resetModules), so the
 * order is the one the test chooses, not whichever file imported them first.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { invokeIpcMock as invokeIpc, stubGetErrorMessage } from '../../test/factories/ipc';

// A factory, not a factory-less vi.mock: vitest keeps a factory's result across
// vi.resetModules(), so the stores each test re-imports call the invokeIpc scripted
// below. A factory-less mock would be re-evaluated and hand them a fresh vi.fn.
vi.mock('../../utils/ipc', () => import('../../utils/__mocks__/ipc'));
stubGetErrorMessage((e: unknown, fallback?: string) => (e instanceof Error ? e.message : (fallback ?? String(e))));

beforeEach(() => {
  vi.resetModules();
  invokeIpc.mockReset();
  invokeIpc.mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
});

describe('store load order (#576 D27-32(b))', () => {
  it('projectStore first, then speleoStore: both evaluate and the handle is registered', async () => {
    const { useProjectStore } = await import('../projectStore');
    const handle = await import('../speleoStoreHandle');
    expect(handle.speleoState()).toBeNull();
    const { useSpeleoStore } = await import('../speleoStore');
    expect(typeof useProjectStore.getState().closeProject).toBe('function');
    expect(handle.speleoState()).toBe(useSpeleoStore.getState());
  });

  it('speleoStore first, then projectStore: both evaluate and the handle is registered', async () => {
    const { useSpeleoStore } = await import('../speleoStore');
    const { useProjectStore } = await import('../projectStore');
    const handle = await import('../speleoStoreHandle');
    expect(typeof useProjectStore.getState().closeProject).toBe('function');
    expect(handle.speleoState()).toBe(useSpeleoStore.getState());
  });

  it("projectStore's SpeleoDB calls reach the live store (a setState-swapped action included)", async () => {
    const { useSpeleoStore } = await import('../speleoStore');
    const { useProjectStore } = await import('../projectStore');
    const clearLink = vi.fn();
    useSpeleoStore.setState({ clearLink });

    const result = await useProjectStore.getState().closeProject();

    expect(result.success).toBe(true);
    expect(clearLink).toHaveBeenCalledTimes(1);
  });

  it('without speleoStore loaded, projectStore still works and simply skips the SpeleoDB calls', async () => {
    const { useProjectStore } = await import('../projectStore');

    const result = await useProjectStore.getState().closeProject();

    expect(result.success).toBe(true);
    expect(useProjectStore.getState().project).toBeNull();
  });
});
