/**
 * [#579] The background update-check hook.
 *
 * `resetAppUpdateLaunchCheckForTests` existed with no consumer, which is the
 * tell that the once-per-process guard was never exercised - and it was wrong:
 * the flag was armed BEFORE the timer, so React StrictMode's double-invoke
 * (arm, cancel the timer, then see the flag and return) meant the launch check
 * never fired in development at all.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render } from '@testing-library/react';
import { StrictMode } from 'react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

const listenMock = vi.hoisted(() => vi.fn(async () => vi.fn()));
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }));

import { useAppUpdate, resetAppUpdateLaunchCheckForTests, runAppUpdateCheck } from '../useAppUpdate';
import { useProjectStore } from '../../store/projectStore';
import { useUpdateStore } from '../../store/updateStore';
import { defaultConfig } from '../../types/config';
import type { Config } from '../../types/config';

function Harness() {
  useAppUpdate();
  return null;
}

function config(overrides: Partial<Config['general']> = {}): Config {
  return {
    ...defaultConfig,
    general: { ...defaultConfig.general, ...overrides },
  };
}

/** How many times the check command was invoked. */
function checkCalls(): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === 'check_for_app_update').length;
}

beforeEach(() => {
  vi.useFakeTimers();
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ status: { kind: 'up_to_date' }, current_version: '0.5.0' });
  listenMock.mockClear();
  resetAppUpdateLaunchCheckForTests();
  useUpdateStore.getState().reset();
  useProjectStore.setState({ config: null as never });
});

afterEach(() => {
  vi.useRealTimers();
});

describe('the launch check', () => {
  it('fires once, ~10s after the config arrives', async () => {
    useProjectStore.setState({ config: config() });
    render(<Harness />);

    await vi.advanceTimersByTimeAsync(9_000);
    expect(checkCalls()).toBe(0);

    await vi.advanceTimersByTimeAsync(2_000);
    expect(checkCalls()).toBe(1);
  });

  /**
   * The StrictMode regression. Under the double-invoke the first pass armed the
   * module flag and its cleanup cancelled the timer; the second pass saw the
   * armed flag and returned early, so nothing ever ran. Arming inside the timer
   * callback is what makes both properties hold at once.
   */
  it('still fires under StrictMode double-invoke, and only once', async () => {
    useProjectStore.setState({ config: config() });
    render(
      <StrictMode>
        <Harness />
      </StrictMode>,
    );

    await vi.advanceTimersByTimeAsync(11_000);

    expect(checkCalls()).toBe(1);
  });

  it('does not fire when the setting is off', async () => {
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    render(<Harness />);

    await vi.advanceTimersByTimeAsync(11_000);

    expect(checkCalls()).toBe(0);
  });

  it('is once per PROCESS - a remount does not check again', async () => {
    useProjectStore.setState({ config: config() });
    const first = render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(checkCalls()).toBe(1);

    first.unmount();
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);

    expect(checkCalls()).toBe(1);
  });
});

describe('the track switch', () => {
  it('re-checks when the track changes, even with the setting off', async () => {
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(checkCalls()).toBe(0);

    useProjectStore.setState({
      config: config({ update_check_enabled: false, update_track: 'weekly' }),
    });
    await vi.advanceTimersByTimeAsync(0);

    expect(checkCalls()).toBe(1);
  });

  it('does not re-check when an unrelated setting changes', async () => {
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);

    useProjectStore.setState({
      config: config({ update_check_enabled: false, splash_screen: true }),
    });
    await vi.advanceTimersByTimeAsync(0);

    expect(checkCalls()).toBe(0);
  });
});

describe('the check result reaches the store', () => {
  it('an available update lands as the banner state', async () => {
    invokeIpcMock.mockResolvedValue({
      status: { kind: 'available', version: '1.2.3', notes_url: 'https://notes' },
      current_version: '0.5.0',
    });
    useProjectStore.setState({ config: config() });
    render(<Harness />);

    await vi.advanceTimersByTimeAsync(11_000);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual({ version: '1.2.3', notesUrl: 'https://notes' });
  });

  /**
   * [review] A check overtaken by a newer one must tell its CALLER so, not hand
   * back a result the store already refused. About renders what it is given, so
   * returning the stale answer put a line on screen contradicting the banner
   * the newer check produced.
   */
  it('reports a check the store no longer owns as stale', async () => {
    let release: ((v: unknown) => void) | undefined;
    invokeIpcMock.mockImplementationOnce(
      () =>
        new Promise((resolve) => {
          release = resolve;
        }),
    );

    const inFlight = runAppUpdateCheck();
    // A newer check takes ownership while the first is still waiting.
    useUpdateStore.getState().beginCheck();
    release?.({
      status: { kind: 'available', version: '1.2.3', notes_url: 'https://n' },
      current_version: '0.5.0',
    });

    await expect(inFlight).resolves.toEqual({ kind: 'stale' });
    // ...and it wrote nothing.
    expect(useUpdateStore.getState().info).toBeNull();
  });

  it('reports a stale FAILURE as stale rather than failed', async () => {
    let reject: ((e: unknown) => void) | undefined;
    invokeIpcMock.mockImplementationOnce(
      () =>
        new Promise((_resolve, rej) => {
          reject = rej;
        }),
    );

    const inFlight = runAppUpdateCheck();
    useUpdateStore.getState().beginCheck();
    reject?.(new Error('offline'));

    await expect(inFlight).resolves.toEqual({ kind: 'stale' });
  });

  /** A failed background check is log-only: never a toast, never a banner. */
  it('a failed check goes quietly to idle', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    useProjectStore.setState({ config: config() });
    render(<Harness />);

    await vi.advanceTimersByTimeAsync(11_000);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('idle');
    expect(s.info).toBeNull();
  });
});
