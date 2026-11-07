// @vitest-environment jsdom
/**
 * #576 D27-32(g): the two crash/log capture installers are idempotent.
 *
 * Each ran its whole setup on every call: `installGlobalErrorCapture` added two
 * more window listeners (so one crash was reported twice), and
 * `installConsoleCapture` wrapped the console again (so every log line was pushed
 * to the backend ring buffer twice). The entry module calls each once today; a
 * latch keeps a second call - a re-run entry, a test, a future caller - a no-op.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn((_cmd: string, _args?: unknown) => Promise.resolve()));
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

const originalConsole = { ...console };

beforeEach(() => {
  vi.resetModules();
  invokeMock.mockClear();
});

afterEach(() => {
  Object.assign(console, originalConsole);
  vi.restoreAllMocks();
});

describe('capture installers are latched (D27-32(g))', () => {
  it('a second installGlobalErrorCapture adds no second pair of window listeners', async () => {
    const add = vi.spyOn(window, 'addEventListener');
    const { installGlobalErrorCapture } = await import('../globalErrorCapture');
    installGlobalErrorCapture();
    installGlobalErrorCapture();
    const kinds = add.mock.calls.map(([type]) => type).filter((t) => t === 'error' || t === 'unhandledrejection');
    expect(kinds).toEqual(['error', 'unhandledrejection']);
  });

  it('a second installConsoleCapture does not wrap the console twice', async () => {
    console.info = vi.fn();
    const { installConsoleCapture } = await import('../consoleCapture');
    installConsoleCapture();
    installConsoleCapture();
    console.info('one line');
    const pushes = invokeMock.mock.calls.filter(([cmd]) => cmd === 'push_frontend_log');
    expect(pushes).toHaveLength(1);
  });
});
