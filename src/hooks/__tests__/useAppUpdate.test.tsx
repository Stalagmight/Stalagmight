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

import {
  useAppUpdate,
  resetAppUpdateLaunchCheckForTests,
  runAppUpdateCheck,
  downloadAppUpdate,
  PERIODIC_CHECK_INTERVAL_MS,
} from '../useAppUpdate';
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

/** How many times the download command was invoked. */
function downloadCalls(): number {
  return invokeIpcMock.mock.calls.filter((c) => c[0] === 'download_app_update').length;
}

const AVAILABLE = {
  status: { kind: 'available', version: '1.2.3', notes_url: 'https://notes' },
  current_version: '0.5.0',
  installs_on_quit: true,
};

/** Answer each command by name; `download` decides how the download goes. */
function answer(check: unknown, download: () => Promise<unknown> = async () => null): void {
  invokeIpcMock.mockImplementation((cmd: string) =>
    cmd === 'download_app_update' ? download() : Promise.resolve(check),
  );
}

const HOUR_MS = 60 * 60 * 1000;

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
  it('an available update lands in the store and downloads in the background', async () => {
    answer(AVAILABLE);
    useProjectStore.setState({ config: config() });
    render(<Harness />);

    await vi.advanceTimersByTimeAsync(11_000);

    const s = useUpdateStore.getState();
    expect(downloadCalls()).toBe(1);
    expect(s.status).toBe('ready');
    expect(s.info).toEqual({ version: '1.2.3', notesUrl: 'https://notes', installsOnQuit: true });
  });

  it('carries the platform\'s install-on-quit answer through to the banner info', async () => {
    useProjectStore.setState({ config: config() });
    answer({ ...AVAILABLE, installs_on_quit: false });

    await runAppUpdateCheck();

    expect(useUpdateStore.getState().info?.installsOnQuit).toBe(false);
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

/**
 * [#631] Background download: a check that finds an update fetches it without
 * being asked, and stays quiet about it - the banner stays hidden until it is
 * ready, and a failure only drops back to offering Download.
 */
describe('the background download', () => {
  it('is marked as background while it runs, so the banner stays hidden', async () => {
    useProjectStore.setState({ config: config() });
    let finish: ((v: unknown) => void) | undefined;
    answer(AVAILABLE, () => new Promise((resolve) => (finish = resolve)));

    await runAppUpdateCheck();

    const s = useUpdateStore.getState();
    expect(s.status).toBe('downloading');
    expect(s.backgroundDownload).toBe(true);

    finish?.(null);
    await vi.advanceTimersByTimeAsync(0);
    expect(useUpdateStore.getState().status).toBe('ready');
  });

  it('a failure falls back to offering Download', async () => {
    useProjectStore.setState({ config: config() });
    answer(AVAILABLE, () => Promise.reject(new Error('offline')));

    await runAppUpdateCheck();
    await vi.advanceTimersByTimeAsync(0);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.backgroundDownload).toBe(false);
  });

  /**
   * A re-check during a download makes the backend refuse the first one's
   * bytes. That refusal must not knock back the second download of the SAME
   * version, which a version comparison alone cannot tell apart.
   */
  it('a superseded download\'s failure does not touch the one that replaced it', async () => {
    const pending: Array<{ resolve: (v: unknown) => void; reject: (e: unknown) => void }> = [];
    answer(AVAILABLE, () => new Promise((resolve, reject) => pending.push({ resolve, reject })));

    const first = downloadAppUpdate('1.2.3', true);
    const second = downloadAppUpdate('1.2.3', true);
    pending[0]?.reject(new Error('A newer update check replaced this one'));

    await expect(first).resolves.toBe('stale');
    expect(useUpdateStore.getState().status).toBe('downloading');

    // Seed the offer the second download completes against.
    useUpdateStore.setState({ info: { version: '1.2.3', notesUrl: 'https://n', installsOnQuit: true } });
    pending[1]?.resolve(null);
    await expect(second).resolves.toBe('ready');
    expect(useUpdateStore.getState().status).toBe('ready');
  });
});

/** [#631] The periodic check while the app stays open. */
describe('the periodic check', () => {
  it('re-checks once the interval has passed, and not before', async () => {
    useProjectStore.setState({ config: config() });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(checkCalls()).toBe(1);

    await vi.advanceTimersByTimeAsync(PERIODIC_CHECK_INTERVAL_MS - HOUR_MS);
    expect(checkCalls()).toBe(1);

    await vi.advanceTimersByTimeAsync(HOUR_MS + 10 * 60 * 1000);
    expect(checkCalls()).toBe(2);
  });

  /**
   * Timers do not run while a laptop sleeps, but the wall clock does. The
   * check must key on elapsed wall-clock time, so the first tick after waking
   * catches up instead of waiting out a whole fresh interval.
   */
  it('catches up within one tick after the clock jumps (sleep and wake)', async () => {
    useProjectStore.setState({ config: config() });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(checkCalls()).toBe(1);

    vi.setSystemTime(Date.now() + PERIODIC_CHECK_INTERVAL_MS + HOUR_MS);
    await vi.advanceTimersByTimeAsync(10 * 60 * 1000);

    expect(checkCalls()).toBe(2);
  });

  /**
   * Turned off AFTER the launch check ran, so the only thing standing between
   * the poll and a second check is the setting itself.
   */
  it('stops when the setting is turned off', async () => {
    useProjectStore.setState({ config: config() });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(checkCalls()).toBe(1);

    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    await vi.advanceTimersByTimeAsync(2 * PERIODIC_CHECK_INTERVAL_MS);

    expect(checkCalls()).toBe(1);
  });

  /**
   * The backend drops a finished download on every successful check, so a
   * poll over a ready update would throw it away only to fetch it again.
   */
  it('leaves a ready download alone', async () => {
    answer(AVAILABLE);
    useProjectStore.setState({ config: config() });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(useUpdateStore.getState().status).toBe('ready');

    await vi.advanceTimersByTimeAsync(2 * PERIODIC_CHECK_INTERVAL_MS);

    expect(checkCalls()).toBe(1);
    expect(downloadCalls()).toBe(1);
  });

  it('retries a failed background download on the next check', async () => {
    let fail = true;
    answer(AVAILABLE, () => (fail ? Promise.reject(new Error('offline')) : Promise.resolve(null)));
    useProjectStore.setState({ config: config() });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);
    expect(useUpdateStore.getState().status).toBe('available');

    fail = false;
    await vi.advanceTimersByTimeAsync(PERIODIC_CHECK_INTERVAL_MS + 10 * 60 * 1000);

    expect(downloadCalls()).toBe(2);
    expect(useUpdateStore.getState().status).toBe('ready');
  });
});

/**
 * [#631 review] The setting is the user's opt-out, and About is a question,
 * not a request to install. Neither may turn into a silent download.
 */
describe('who may download without asking', () => {
  it('a track switch with automatic updates off checks but does not download', async () => {
    answer(AVAILABLE);
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    render(<Harness />);
    await vi.advanceTimersByTimeAsync(11_000);

    useProjectStore.setState({
      config: config({ update_check_enabled: false, update_track: 'weekly' }),
    });
    await vi.advanceTimersByTimeAsync(0);

    expect(checkCalls()).toBe(1);
    expect(downloadCalls()).toBe(0);
    expect(useUpdateStore.getState().status).toBe('available');
  });

  it('About with automatic updates off leaves the download to the user', async () => {
    answer(AVAILABLE);
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });

    await runAppUpdateCheck({ userInitiated: true });

    expect(downloadCalls()).toBe(0);
  });

  it('About with automatic updates on downloads in view, not in the background', async () => {
    let finish: ((v: unknown) => void) | undefined;
    answer(AVAILABLE, () => new Promise((resolve) => (finish = resolve)));
    useProjectStore.setState({ config: config() });

    await runAppUpdateCheck({ userInitiated: true });

    expect(useUpdateStore.getState().status).toBe('downloading');
    expect(useUpdateStore.getState().backgroundDownload).toBe(false);
    finish?.(null);
  });

  it('About un-hides a banner dismissed earlier, since it points the user at it', async () => {
    answer(AVAILABLE, () => Promise.reject(new Error('offline')));
    useProjectStore.setState({ config: config({ update_check_enabled: false }) });
    useUpdateStore.getState().dismiss('1.2.3');

    await runAppUpdateCheck({ userInitiated: true });

    expect(useUpdateStore.getState().dismissedVersion).toBeNull();
  });

  /**
   * The backend drops the parked bytes on every successful check, so a
   * re-check from About would throw a finished download away. About answers
   * from what is in hand instead, and shows it.
   */
  it('About over a ready download answers without re-checking, and reveals it', async () => {
    answer(AVAILABLE);
    useProjectStore.setState({ config: config() });
    await runAppUpdateCheck();
    await vi.advanceTimersByTimeAsync(0);
    expect(useUpdateStore.getState().status).toBe('ready');
    useUpdateStore.getState().dismiss('1.2.3');

    const outcome = await runAppUpdateCheck({ userInitiated: true });

    expect(outcome).toEqual({ kind: 'available', currentVersion: '0.5.0', version: '1.2.3' });
    expect(checkCalls()).toBe(1);
    expect(downloadCalls()).toBe(1);
    expect(useUpdateStore.getState().status).toBe('ready');
    expect(useUpdateStore.getState().dismissedVersion).toBeNull();
  });

  /**
   * A check that finds nothing while a background download is in flight makes
   * the backend refuse that download's bytes; its failure must land nowhere.
   */
  it('a download failing after an up-to-date check leaves the store idle', async () => {
    let fail: ((e: unknown) => void) | undefined;
    answer(AVAILABLE, () => new Promise((_resolve, reject) => (fail = reject)));
    useProjectStore.setState({ config: config() });
    await runAppUpdateCheck();
    expect(useUpdateStore.getState().status).toBe('downloading');

    answer({ status: { kind: 'up_to_date' }, current_version: '0.5.0', installs_on_quit: true });
    await runAppUpdateCheck();
    fail?.(new Error('A newer update check replaced this one'));
    await vi.advanceTimersByTimeAsync(0);

    expect(useUpdateStore.getState().status).toBe('idle');
  });
});
