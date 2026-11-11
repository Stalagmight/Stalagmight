import { beforeEach, expect, it, vi } from 'vitest';
import { runConfirmedProjectSwitch, runProjectSwitch, runTreeReplacement } from '../projectTransition';

const { probe, warning, error, clear } = vi.hoisted(() => ({
  probe: vi.fn(), warning: vi.fn(), error: vi.fn(), clear: vi.fn(),
}));
vi.mock('../ipc', () => ({ invokeIpc: probe, getErrorMessage: String }));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: warning }));
vi.mock('../../store/uiStore', () => ({ useUIStore: { getState: () => ({ openDialog: error }) } }));
vi.mock('../telemetry', () => ({ clearProjectActionSource: clear }));

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const restricted = { project: { permission: 'READ_ONLY', name: 'Synthetic cave', lock_holder: null }, endpoint: 'https://example.com', verification: 'verified' };
beforeEach(() => { vi.clearAllMocks(); probe.mockResolvedValue(null); warning.mockResolvedValue(true); });

it.each(['success', 'rejection', 'cancel'] as const)('supersedes a slow probe: %s', async (mode) => {
  const a = deferred<unknown>();
  probe.mockImplementation((_command: unknown, args: { path: string }) => args.path === 'A' ? a.promise : null);
  const workA = vi.fn(async () => ({ success: true }));
  const workB = vi.fn(async () => ({ success: true }));
  const first = runConfirmedProjectSwitch('A', () => null, false, workA);
  await vi.waitFor(() => expect(probe).toHaveBeenCalled());
  expect(await runConfirmedProjectSwitch('B', () => null, false, workB)).toEqual({ success: true });
  if (mode === 'rejection') a.reject(new Error('stale'));
  else a.resolve(mode === 'cancel' ? restricted : null);
  expect(await first).toEqual({ success: false, superseded: true });
  expect(workA).not.toHaveBeenCalled();
  expect(workB).toHaveBeenCalledOnce();
  expect(warning).not.toHaveBeenCalled();
  expect(error).not.toHaveBeenCalled();
  expect(clear).not.toHaveBeenCalled();
});

it('opens a restricted project without a pre-open access confirmation', async () => {
  probe.mockResolvedValue(restricted);
  const work = vi.fn(async () => ({ success: true }));
  expect(await runConfirmedProjectSwitch('A', () => null, false, work)).toEqual({ success: true });
  expect(work).toHaveBeenCalledOnce();
  expect(warning).not.toHaveBeenCalled();
});

it.each([true, false])('checks queued pre-confirmed/already-open work (%s)', async (confirmed) => {
  const gate = deferred<void>();
  const entered = deferred<void>();
  const replacement = runTreeReplacement(async () => { entered.resolve(); await gate.promise; });
  await entered.promise;
  const work = vi.fn(async () => ({ success: true }));
  const a = runConfirmedProjectSwitch('A', () => confirmed ? null : 'A', confirmed, work);
  const b = runProjectSwitch(async () => undefined);
  gate.resolve();
  await replacement;
  expect(await a).toEqual({ success: false, superseded: true });
  await b;
  expect(work).not.toHaveBeenCalled();
  expect(probe).not.toHaveBeenCalled();
});
