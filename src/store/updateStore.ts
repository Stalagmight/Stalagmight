import { create } from 'zustand';
import type { AppUpdateProgress } from '../types/ipc/AppUpdateProgress';

/**
 * In-app updater state (#579).
 *
 * `idle` covers both "never checked" and "checked, nothing new" - the banner
 * (and every other consumer) only ever needs to know there is nothing to show,
 * not why. A background check failure (docs/notifications.md: never a
 * toast/banner for a failed background check) also lands on `idle` via
 * {@link UpdateStore.setCheckFailed}; `error` is reserved for a download that
 * failed with no prior `info` to fall back to (should not happen in practice,
 * since a download is only ever started after `setAvailable`).
 */
export type UpdateStatus = 'idle' | 'checking' | 'available' | 'downloading' | 'ready' | 'error';

export interface UpdateInfo {
  version: string;
  notesUrl: string;
}

interface UpdateStore {
  status: UpdateStatus;
  info: UpdateInfo | null;
  progress: AppUpdateProgress | null;
  /**
   * The version the user dismissed the banner for, this session only - NOT
   * persisted, so a dismissed update is offered again on the next launch.
   */
  dismissedVersion: string | null;

  /**
   * Which check the store is currently willing to hear from.
   *
   * Three independent callers start checks - the launch hook, the track switch,
   * and About's button - and they can be in flight at once (switch track, then
   * press Check). Replies arrive in whatever order the network decides, so
   * without this the SLOWER reply wins and the banner ends up describing the
   * track the user just left. Every reply carries the sequence it was issued
   * under and is dropped unless it is still the latest.
   */
  checkSeq: number;
  /**
   * The version the in-flight download belongs to, so a progress or completion
   * event from a download that has since been superseded cannot land on a
   * different version's banner. The wedge this prevents is `ready` with `info`
   * from another check - a Restart button that installs something else, or
   * nothing.
   */
  downloadVersion: string | null;

  /** Start a check; the returned sequence must be passed back with the reply. */
  beginCheck: () => number;
  /** No update on offer - back to idle, clearing anything from a previous check. */
  setUpToDate: (seq: number) => void;
  /** A background check failed - idle, silently (no toast/banner). */
  setCheckFailed: (seq: number) => void;
  setAvailable: (seq: number, info: UpdateInfo) => void;
  startDownloading: (version: string) => void;
  setProgress: (progress: AppUpdateProgress) => void;
  setReady: (version: string) => void;
  /** A download failed - the banner re-offers the download it already knows about. */
  setDownloadFailed: () => void;
  dismiss: (version: string) => void;
  /** Test-only reset. */
  reset: () => void;
}

const INITIAL_STATE: Pick<
  UpdateStore,
  'status' | 'info' | 'progress' | 'dismissedVersion' | 'checkSeq' | 'downloadVersion'
> = {
  status: 'idle',
  info: null,
  progress: null,
  dismissedVersion: null,
  checkSeq: 0,
  downloadVersion: null,
};

export const useUpdateStore = create<UpdateStore>((set, get) => ({
  ...INITIAL_STATE,

  beginCheck: () => {
    const { checkSeq, status } = get();
    const seq = checkSeq + 1;
    // `checking` replaces the status ONLY when there is nothing on screen to
    // lose. A re-check while an update is already offered (or downloaded, or
    // downloading) must not blank the banner on the way in: the reply will
    // overwrite the status anyway if it lands, and if it never lands - the
    // usual case, a background poll with no network - the user would have
    // watched an actionable banner, and possibly a finished download, vanish
    // for no reason they could see. It also means `checking` is a reliable
    // signal that nothing was actionable, which is what `setCheckFailed` reads.
    const actionable = status !== 'idle' && status !== 'error';
    set(actionable ? { checkSeq: seq } : { status: 'checking', checkSeq: seq });
    return seq;
  },
  setUpToDate: (seq) => {
    if (get().checkSeq !== seq) return;
    set({ status: 'idle', info: null, progress: null, downloadVersion: null });
  },
  setCheckFailed: (seq) => {
    if (get().checkSeq !== seq) return;
    // A check that never reached the server knows nothing about the update we
    // already found: the backend still holds the parked `Update` and, if it got
    // that far, the verified bytes - it does not bump its generation on a failed
    // check either. Dropping to idle here hid an actionable banner - including a
    // finished "Restart to update" - on the strength of one network blip, with
    // no way to get it back short of another successful check. `checking` is
    // only ever the status when there was nothing actionable (see `beginCheck`),
    // so this is exactly the "nothing to lose" case.
    if (get().status !== 'checking') return;
    set({ status: 'idle', progress: null });
  },
  setAvailable: (seq, info) => {
    if (get().checkSeq !== seq) return;
    set({ status: 'available', info, progress: null, downloadVersion: null });
  },
  startDownloading: (version) => set({ status: 'downloading', downloadVersion: version }),
  setProgress: (progress) => {
    // Only while a download is actually running: a tick that arrives after the
    // download ended (or was superseded by a check) would otherwise paint a
    // progress bar over an unrelated state.
    if (get().status !== 'downloading') return;
    set({ progress });
  },
  setReady: (version) => {
    const { status, downloadVersion, info } = get();
    if (status !== 'downloading' || downloadVersion !== version || info?.version !== version) {
      return;
    }
    set({ status: 'ready' });
  },
  setDownloadFailed: () =>
    set((state) => ({
      status: state.info ? 'available' : 'error',
      progress: null,
      downloadVersion: null,
    })),
  dismiss: (version) => set({ dismissedVersion: version }),
  reset: () => set(INITIAL_STATE),
}));
