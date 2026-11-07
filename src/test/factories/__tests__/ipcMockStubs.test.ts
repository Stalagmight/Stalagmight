/**
 * The file-wide switches of the shared IPC fake (#576 E15): a swapped
 * `getErrorMessage` survives vi.resetAllMocks()/restoreAllMocks() (it is not a
 * vi.fn, like the inline stubs it replaced), and `emptyBinaryRefresh` answers an
 * unscripted `get_project_state_binary` with empty bytes while leaving the call on
 * the record.
 */
import { describe, expect, it, vi } from 'vitest';
import { getErrorMessage, invokeIpc } from '../../../utils/ipc';
import { emptyBinaryRefresh, fallbackOr, invokeIpcMock, messageOr, stubGetErrorMessage } from '../ipc';

vi.mock('../../../utils/ipc');
stubGetErrorMessage(fallbackOr('the server said no'));
emptyBinaryRefresh();

describe('file-wide switches of the shared utils/ipc mock', () => {
  it('reads error messages with the stub the file installed', () => {
    expect(getErrorMessage(new Error('disk full'), 'Could not save')).toBe('Could not save');
    expect(getErrorMessage(new Error('disk full'))).toBe('the server said no');
  });

  it('keeps the stub through vi.resetAllMocks and vi.restoreAllMocks', () => {
    vi.resetAllMocks();
    vi.restoreAllMocks();
    expect(getErrorMessage('x')).toBe('the server said no');
  });

  it('presets read like the inline stubs they replace', () => {
    expect(messageOr('error')(new Error('boom'), 'f')).toBe('boom');
    expect(messageOr('error')('plain', 'f')).toBe('f');
    expect(messageOr('error')(null)).toBe('error');
    expect(fallbackOr()(new Error('boom'))).toBeUndefined();
  });

  it('answers an unscripted binary refresh with empty bytes, and still records it', async () => {
    invokeIpcMock.mockReset();
    const answer = await invokeIpc('get_project_state_binary');
    expect(answer).toBeInstanceOf(ArrayBuffer);
    expect((answer as ArrayBuffer).byteLength).toBe(0);
    expect(invokeIpcMock).toHaveBeenCalledWith('get_project_state_binary');
  });

  it('passes scripted bytes, and every other command, through untouched', async () => {
    const bytes = new Uint8Array([1, 2, 3]);
    invokeIpcMock.mockResolvedValueOnce(bytes);
    await expect(invokeIpc('get_project_state_binary')).resolves.toBe(bytes);
    invokeIpcMock.mockReturnValueOnce('sync');
    expect(invokeIpcMock('list_recent_projects')).toBe('sync');
  });
});
