import { useEffect, useRef } from 'react';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { invokeIpc } from '../utils/ipc';
import { debugLog } from '../utils/debugLog';
import { useProjectStore } from '../store/projectStore';
import { useUpdateStore } from '../store/updateStore';
import type { AppUpdateCheck } from '../types/ipc/AppUpdateCheck';
import type { AppUpdateProgress } from '../types/ipc/AppUpdateProgress';
import type { UpdateTrack } from '../types/ipc/UpdateTrack';

/** How long after config loads before the automatic launch check fires (#579). */
const LAUNCH_CHECK_DELAY_MS = 10_000;

/** How often the app re-checks while it stays open (#631). */
export const PERIODIC_CHECK_INTERVAL_MS = 4 * 60 * 60 * 1000;

/**
 * How often the periodic check looks at the clock. Comparing wall-clock time on
 * a short tick, rather than arming one 4-hour timer, is what catches a laptop
 * waking from sleep: timers do not advance while it sleeps, but `Date.now()`
 * does, so the first tick after waking sees the interval has passed.
 */
const PERIODIC_TICK_MS = 5 * 60 * 1000;

/** When the most recent check (of any kind) started; drives the periodic one. */
let lastCheckStartedAt = 0;

/** The running app's version, as the most recent successful check reported it. */
let lastCurrentVersion: string | null = null;

/**
 * Guards the automatic launch check to once per process - a module flag
 * (not a ref) because it must survive `useAppUpdate` being mounted more than
 * once conceptually across the app's lifetime (it is mounted once in App.tsx,
 * but a module flag is the one guarantee that holds regardless of how React
 * chooses to remount the tree).
 */
let launchCheckArmed = false;

/** Test-only reset so a leaked arm can't bleed across cases. */
export function resetAppUpdateLaunchCheckForTests(): void {
  launchCheckArmed = false;
  lastCheckStartedAt = 0;
  lastCurrentVersion = null;
}

/**
 * What one check found, for a caller that has something to say about it.
 *
 * The store is driven either way - this is the ADDITIONAL, caller-specific
 * answer, which is why it carries `currentVersion`: About says "you are on X,
 * Y is available", and the banner cannot, because it does not run the check.
 */
export type AppUpdateCheckOutcome =
  | { kind: 'up_to_date'; currentVersion: string }
  | { kind: 'available'; currentVersion: string; version: string }
  /**
   * A newer check took ownership of the store while this one was in flight, so
   * this reply was never applied. Distinct from `failed`: nothing went wrong,
   * this answer is simply not the current one - and a caller that RENDERS the
   * answer (About) must not show it, or its text would contradict the banner
   * the newer check produced.
   */
  | { kind: 'stale' }
  | { kind: 'failed' };

/**
 * Download and verify an offered update, driving the store through it.
 *
 * `background` marks a download no one asked for (#631): every check that finds
 * an update starts one, and the banner stays hidden until it is ready. A
 * failure is quiet either way - the banner drops back to offering Download, and
 * the next check retries - so the caller decides whether to say anything.
 *
 * `stale` means a later download took over while this one ran, so its answer
 * was not applied and is nothing to report.
 */
export async function downloadAppUpdate(
  version: string,
  background: boolean,
): Promise<'ready' | 'failed' | 'stale'> {
  const seq = useUpdateStore.getState().startDownloading(version, background);
  // A later download owns the store now; this one's answer is not the current one.
  const superseded = () => useUpdateStore.getState().downloadSeq !== seq;
  try {
    await invokeIpc('download_app_update', undefined, { purpose: 'download app update' });
    if (superseded()) return 'stale';
    useUpdateStore.getState().setReady(version);
    return 'ready';
  } catch (e) {
    debugLog('[useAppUpdate] download failed:', e);
    if (superseded()) return 'stale';
    useUpdateStore.getState().setDownloadFailed();
    return 'failed';
  }
}

/**
 * Run one update check and put the answer in the store.
 *
 * The SINGLE path for every caller - the launch hook, the periodic check, a
 * track switch, and About's manual button - because the alternative (About
 * keeping its own local result) makes the two disagree in both directions:
 * About promising a banner the store never learned to show, and the backend's
 * check nulling the parked download bytes underneath a banner still offering
 * "Restart to update".
 *
 * A found update is downloaded straight away only when automatic updates are
 * on (#631) - the setting is the user's opt-out, so a track switch or a manual
 * check with it off leaves the Download button to them. `userInitiated` (About)
 * shows that download's progress instead of hiding it, and never re-checks
 * over an update already downloading or downloaded: the backend drops the
 * parked bytes on every successful check, so pressing "Check" would throw away
 * a finished download only to fetch the same one again.
 *
 * A failed check is never surfaced as a toast or banner (docs/notifications.md)
 * - it goes quietly to idle. Callers who ASKED for the check say so themselves.
 */
export async function runAppUpdateCheck(
  opts: { userInitiated?: boolean } = {},
): Promise<AppUpdateCheckOutcome> {
  const userInitiated = opts.userInitiated === true;
  const store = useUpdateStore.getState();
  if (
    userInitiated &&
    store.info &&
    lastCurrentVersion !== null &&
    (store.status === 'downloading' || store.status === 'ready')
  ) {
    store.reveal();
    return { kind: 'available', currentVersion: lastCurrentVersion, version: store.info.version };
  }
  // Every reply is matched against this, so a check overtaken by a later one
  // (a track switch, a second press of the About button) cannot land.
  const seq = store.beginCheck();
  lastCheckStartedAt = Date.now();
  try {
    const result = await invokeIpc<AppUpdateCheck>('check_for_app_update', undefined, {
      purpose: 'check for app update',
    });
    // The setters drop a stale write on their own; this is the CALLER's half of
    // the same fact. Returning the answer anyway let About render a result the
    // store had already rejected, so its line and the banner disagreed.
    if (useUpdateStore.getState().checkSeq !== seq) return { kind: 'stale' };
    lastCurrentVersion = result.current_version;
    if (result.status.kind === 'available') {
      const { version, notes_url: notesUrl } = result.status;
      useUpdateStore
        .getState()
        .setAvailable(seq, { version, notesUrl, installsOnQuit: result.installs_on_quit });
      // About says "see the update banner"; a dismissal from earlier this
      // session must not make that a lie.
      if (userInitiated) useUpdateStore.getState().reveal();
      // [#631] Fetch it straight away, so the banner's first appearance is
      // already "ready" and an ordinary quit can install it.
      if (useProjectStore.getState().config?.general.update_check_enabled) {
        void downloadAppUpdate(version, !userInitiated);
      }
      return { kind: 'available', currentVersion: result.current_version, version };
    }
    useUpdateStore.getState().setUpToDate(seq);
    return { kind: 'up_to_date', currentVersion: result.current_version };
  } catch (e) {
    debugLog('[useAppUpdate] check failed:', e);
    if (useUpdateStore.getState().checkSeq !== seq) return { kind: 'stale' };
    useUpdateStore.getState().setCheckFailed(seq);
    return { kind: 'failed' };
  }
}

/**
 * Drives the in-app updater's background checks (#579): mount once at app
 * root (see App.tsx). `check_for_app_update` deliberately does not gate on
 * `update_check_enabled` itself - this hook is the gate for the automatic
 * launch check; the About dialog's manual "Check for updates" button calls
 * {@link runAppUpdateCheck} directly and is unaffected by either the setting or
 * this hook.
 */
export function useAppUpdate(): void {
  const config = useProjectStore((s) => s.config);
  const prevTrackRef = useRef<UpdateTrack | undefined>(undefined);

  // (a) The automatic launch check: once per process, ~10s after config is
  // available, gated on update_check_enabled.
  useEffect(() => {
    if (!config || launchCheckArmed) return;
    if (!config.general.update_check_enabled) return;
    const timer = window.setTimeout(() => {
      // Armed HERE, not before the timer. React StrictMode double-invokes
      // effects in development: the first pass armed the flag and the cleanup
      // cancelled its timer, so the second pass saw the flag and returned - and
      // the launch check never ran in dev at all. Arming when the timer
      // actually fires keeps the once-per-process guarantee (nothing else can
      // reach this line first) while surviving a cancelled pass.
      if (launchCheckArmed) return;
      launchCheckArmed = true;
      void runAppUpdateCheck();
    }, LAUNCH_CHECK_DELAY_MS);
    return () => window.clearTimeout(timer);
  }, [config]);

  // (b) The periodic check (#631): every PERIODIC_CHECK_INTERVAL_MS while the
  // app stays open, gated on the same setting as the launch check. It waits for
  // the launch check to own the first one, and it never runs over a download
  // that is in flight or finished: the backend drops the downloaded bytes on
  // every successful check, so a poll would throw away a ready update only to
  // fetch the same one again. A newer release published meanwhile is picked up
  // on the next launch, after this one has installed on quit.
  useEffect(() => {
    if (!config?.general.update_check_enabled) return;
    const timer = window.setInterval(() => {
      if (!launchCheckArmed) return;
      if (Date.now() - lastCheckStartedAt < PERIODIC_CHECK_INTERVAL_MS) return;
      const { status } = useUpdateStore.getState();
      if (status === 'checking' || status === 'downloading' || status === 'ready') return;
      void runAppUpdateCheck();
    }, PERIODIC_TICK_MS);
    return () => window.clearInterval(timer);
  }, [config?.general.update_check_enabled]);

  // (c) Download progress, streamed while `download_app_update` is in flight.
  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    (async () => {
      try {
        const fn = await listen<AppUpdateProgress>('app-update:download-progress', (event) => {
          useUpdateStore.getState().setProgress(event.payload);
        });
        if (disposed) fn();
        else unlisten = fn;
      } catch (e) {
        console.warn('[useAppUpdate] download-progress listener not available:', e);
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  // (d) A track switch (stable <-> weekly) re-checks against the new track,
  // which also clears a stale available-banner when the new track has
  // nothing on offer (runAppUpdateCheck -> setUpToDate resets info/progress).
  // Not gated on update_check_enabled: the user just made an explicit choice
  // in Settings, the same as pressing "Check for updates" in About.
  useEffect(() => {
    const track = config?.general.update_track;
    if (track === undefined) return;
    if (prevTrackRef.current === undefined) {
      prevTrackRef.current = track;
      return;
    }
    if (track !== prevTrackRef.current) {
      prevTrackRef.current = track;
      void runAppUpdateCheck();
    }
  }, [config?.general.update_track]);
}
