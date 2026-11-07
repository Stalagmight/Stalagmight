// @vitest-environment jsdom
/**
 * [#576 RV4-R-S6] A SpeleoDB Done / Push that fails after its pre-push save still
 * shows what that save could not keep: the backend sends the save's outcome as the
 * `speleodb-pre-push-save` event, and this hook shows its warnings like any save's.
 */
import { renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';

type Handler = (event: { payload: { warnings: unknown[] } }) => void;
const listeners = vi.hoisted(() => new Map<string, Handler>());
const unlisten = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipcEvents', () => ({
  IPC_EVENTS: { SPELEODB_PRE_PUSH_SAVE: 'speleodb-pre-push-save' },
  listenIpc: vi.fn(async (name: string, handler: Handler) => {
    listeners.set(name, handler);
    return unlisten;
  }),
}));
const surfaceSaveWarnings = vi.hoisted(() => vi.fn());
vi.mock('../../utils/saveFeedback', () => ({ surfaceSaveWarnings }));

import { useSpeleoPrePushSave } from '../useSpeleoPrePushSave';

const LOST = { code: 'DISPLAY_SETTINGS_NOT_SAVED', message: 'lost', severity: 'warning' };

beforeEach(() => {
  listeners.clear();
  unlisten.mockReset();
  surfaceSaveWarnings.mockReset();
});

describe('useSpeleoPrePushSave (#576 RV4-R-S6)', () => {
  it('shows the warnings of a pre-push save whose Done / Push then failed', async () => {
    const { unmount } = renderHook(() => useSpeleoPrePushSave());
    await vi.waitFor(() => expect(listeners.has('speleodb-pre-push-save')).toBe(true));

    listeners.get('speleodb-pre-push-save')?.({ payload: { warnings: [LOST] } });

    expect(surfaceSaveWarnings).toHaveBeenCalledWith([LOST]);
    unmount();
    expect(unlisten).toHaveBeenCalled();
  });
});
