/**
 * #429 hygiene - `ALREADY_OPEN` is not a load failure.
 *
 * The backend rejects `load_project_full_binary` with `ALREADY_OPEN` when the
 * requested path is the project it already has open, and every caller treats that
 * as control flow (`loadProject` short-circuits on it and switches the view). It
 * was nevertheless logged as `console.error`, so an ordinary reopen filled the
 * debug log with an error the user was then asked to report. Only the LEVEL
 * changes: the rejection still propagates unchanged, and every other failure
 * stays an error.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const invoke = vi.fn();
vi.mock('@tauri-apps/api/core', async () =>
  (await import('../../test/factories/tauri')).tauriCore((...args: unknown[]) => invoke(...args)),
);

const { loadProjectFullBinary } = await import('../loadProjectBinary');

describe('loadProjectFullBinary IPC rejection logging', () => {
  let errorSpy: ReturnType<typeof vi.spyOn>;
  let infoSpy: ReturnType<typeof vi.spyOn>;

  beforeEach(() => {
    invoke.mockReset();
    errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {});
    infoSpy = vi.spyOn(console, 'info').mockImplementation(() => {});
  });

  afterEach(() => {
    errorSpy.mockRestore();
    infoSpy.mockRestore();
  });

  it('logs the expected ALREADY_OPEN short-circuit below error level, and rethrows it', async () => {
    const rejection = { code: 'ALREADY_OPEN', message: 'Project already open' };
    invoke.mockRejectedValue(rejection);

    await expect(loadProjectFullBinary('/caves/sump.stalx')).rejects.toBe(rejection);

    expect(errorSpy).not.toHaveBeenCalled();
    expect(infoSpy).toHaveBeenCalledTimes(1);
  });

  it('still logs a real IPC failure as an error, and rethrows it', async () => {
    const rejection = { code: 'IO_ERROR', message: 'disk went away' };
    invoke.mockRejectedValue(rejection);

    await expect(loadProjectFullBinary('/caves/sump.stalx')).rejects.toBe(rejection);

    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(infoSpy).not.toHaveBeenCalled();
  });

  it('treats a bare Error as a real failure - the downgrade is code-scoped only', async () => {
    const rejection = new Error('Project already open');
    invoke.mockRejectedValue(rejection);

    await expect(loadProjectFullBinary('/caves/sump.stalx')).rejects.toBe(rejection);

    expect(errorSpy).toHaveBeenCalledTimes(1);
    expect(infoSpy).not.toHaveBeenCalled();
  });

  it.each([null, undefined, 'ALREADY_OPEN', { code: 'already_open' }])(
    'treats %o as a real failure',
    async (rejection) => {
      invoke.mockRejectedValue(rejection);

      await expect(loadProjectFullBinary('/caves/sump.stalx')).rejects.toBe(rejection);

      expect(errorSpy).toHaveBeenCalledTimes(1);
      expect(infoSpy).not.toHaveBeenCalled();
    }
  );
});
