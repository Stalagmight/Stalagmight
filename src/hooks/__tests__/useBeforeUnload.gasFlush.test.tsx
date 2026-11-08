/**
 * [#576 W86] A gas-planning value committed just before an update restart is written
 * before the install starts, not under it and not lost.
 *
 * Plan-mode gas fields persist through one debounced writer (`gasConfigDisk.ts`,
 * 450 ms). A value typed and then Restart pressed inside that window was still
 * waiting when the install started, and its `update_gas_config` fired from the timer
 * mid-install - or never, once the restart ended the process. The install now
 * flushes the writer first and waits for it, bounded so a write that never answers
 * cannot hold the install window up for ever.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/react';

const listeners = vi.hoisted(() => [] as Array<() => Promise<void> | void>);
vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async (_name: string, cb: () => Promise<void>) => {
    listeners.push(cb);
    return vi.fn();
  }),
}));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ destroy: vi.fn(async () => {}), close: vi.fn(async () => {}) }),
}));
vi.mock('@tauri-apps/plugin-dialog', () => ({ confirm: vi.fn(async () => true) }));
vi.mock('../../utils/ipc');

import { useBeforeUnload } from '../useBeforeUnload';
import {
  guardQuit,
  requestUpdateRestart,
  resetQuitConfirmationForTests,
  resetUpdateRestartForTests,
} from '../../utils/quit';
import { flushGasConfigWrite, queueGasConfigWrite } from '../../utils/gasConfigDisk';
import { useAutosaveHoldStore } from '../../utils/autosaveHold';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { useUpdateStore } from '../../store/updateStore';
import { defaultConfig, type Config } from '../../types/config';
import { deferred, type Deferred } from '../../test/factories/async';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

emptyBinaryRefresh();

const INSTALL = 'install_app_update_and_restart';
const GAS = 'update_gas_config';

function App() {
  useBeforeUnload(false, 'unused', undefined, guardQuit);
  return null;
}

function closeRequested(): Promise<void> {
  return Promise.all(listeners.map((cb) => cb())).then(() => undefined);
}

/** The install and gas-config commands, in the order they reached the backend. */
function sentInOrder(): string[] {
  return invokeIpcMock.mock.calls.map((c) => c[0] as string).filter((cmd) => cmd === INSTALL || cmd === GAS);
}

let install: Deferred<unknown>;
let gasWrite: Deferred<unknown> | null;

beforeEach(async () => {
  vi.useFakeTimers();
  listeners.length = 0;
  invokeIpcMock.mockReset();
  install = deferred<unknown>();
  gasWrite = null;
  invokeIpcMock.mockImplementation((cmd: string) => {
    // A real install takes seconds to write the new bundle, and then ends the process.
    if (cmd === INSTALL) return install.promise;
    if (cmd === GAS && gasWrite) return gasWrite.promise;
    return Promise.resolve(null);
  });
  resetQuitConfirmationForTests();
  resetUpdateRestartForTests();
  useAutosaveHoldStore.setState({ holds: 0 });
  useUpdateStore.getState().reset();
  useUIStore.setState({ activeDialog: null, dialogData: {}, toasts: [] });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  // A clean project: the restart has nothing to ask about.
  useProjectStore.setState({
    project: { name: 'CaveA' } as never,
    isDirty: false,
    config: structuredClone(defaultConfig) as Config,
  });
});

afterEach(async () => {
  install.resolve(null);
  gasWrite?.resolve(null);
  await vi.advanceTimersByTimeAsync(10_000);
  await flushGasConfigWrite();
  vi.useRealTimers();
});

/** Restart pressed: the arm, the close request, and as far as the install's IPC. */
async function restart(): Promise<void> {
  render(<App />);
  await vi.advanceTimersByTimeAsync(0);
  await requestUpdateRestart();
  void closeRequested();
  await vi.advanceTimersByTimeAsync(0);
}

describe('a pending gas-config write when an update restart installs', () => {
  it('is written before the install starts, with the value the user set', async () => {
    queueGasConfigWrite({ swim_speed_m_per_min: 17 });
    await restart();
    // Well past the writer's debounce, with the install still running.
    await vi.advanceTimersByTimeAsync(2_000);

    expect(sentInOrder()).toEqual([GAS, INSTALL]);
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === GAS);
    expect((call?.[1] as { gas: { swim_speed_m_per_min: number } }).gas.swim_speed_m_per_min).toBe(17);
    expect(useUpdateStore.getState().installing).toBe(true);
  });

  it('the install waits for the write to land, not just to be sent', async () => {
    gasWrite = deferred<unknown>();
    queueGasConfigWrite({ swim_speed_m_per_min: 17 });
    await restart();
    expect(sentInOrder()).toEqual([GAS]);

    gasWrite.resolve(null);
    await vi.advanceTimersByTimeAsync(0);
    expect(sentInOrder()).toEqual([GAS, INSTALL]);
  });

  it('a write that never answers does not hold the install up for ever', async () => {
    gasWrite = deferred<unknown>();
    queueGasConfigWrite({ swim_speed_m_per_min: 17 });
    await restart();
    expect(sentInOrder()).toEqual([GAS]);

    await vi.advanceTimersByTimeAsync(5_000);
    expect(sentInOrder()).toEqual([GAS, INSTALL]);
  });

  it('control: with nothing pending, the install starts at once and writes no gas config', async () => {
    await restart();
    await vi.advanceTimersByTimeAsync(2_000);

    expect(sentInOrder()).toEqual([INSTALL]);
  });
});
