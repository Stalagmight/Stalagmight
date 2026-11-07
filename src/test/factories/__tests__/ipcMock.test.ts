/**
 * Pins the shared IPC fake (#576 E15): a factory-less `vi.mock` of `utils/ipc`
 * loads `src/utils/__mocks__/ipc.ts`, the code under test and the test see the
 * same `vi.fn`, and `getErrorMessage` is the real reader until a file swaps it.
 * The swaps themselves are file-wide, so they are pinned in ipcMockStubs.test.ts.
 */
import { describe, expect, it, vi } from 'vitest';
import { getErrorMessage, invokeIpc } from '../../../utils/ipc';
import { invokeIpcMock } from '../ipc';

vi.mock('../../../utils/ipc');

describe('the shared utils/ipc mock', () => {
  it('hands the code under test the same vi.fn the test scripts', async () => {
    expect(vi.isMockFunction(invokeIpc)).toBe(true);
    expect(invokeIpc).toBe(invokeIpcMock);
    invokeIpcMock.mockResolvedValueOnce(['a']);
    await expect(invokeIpc('list_recent_projects')).resolves.toEqual(['a']);
    expect(invokeIpcMock).toHaveBeenCalledWith('list_recent_projects');
  });

  it('answers undefined for a call nobody scripted', () => {
    invokeIpcMock.mockReset();
    expect(invokeIpcMock('get_project_state_binary')).toBeUndefined();
  });

  it('reads error messages with the real getErrorMessage by default', () => {
    expect(getErrorMessage(new Error('disk full'), 'fallback')).toBe('disk full');
    expect(getErrorMessage({ code: 'Internal', message: 'boom' })).toBe('boom');
    expect(getErrorMessage(42, 'fallback')).toBe('fallback');
  });
});
