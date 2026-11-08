import { describe, it, expect, beforeEach } from 'vitest';
import { useUpdateStore } from '../updateStore';

const INFO = { version: '1.2.3', notesUrl: 'https://x' };

beforeEach(() => {
  useUpdateStore.getState().reset();
});

/** Run one check to completion, returning the sequence it used. */
function check(): number {
  return useUpdateStore.getState().beginCheck();
}

/** Put the store in the state a completed check + started download leaves. */
function seedAvailable(info = INFO): void {
  useUpdateStore.getState().setAvailable(check(), info);
}

describe('updateStore transitions', () => {
  it('starts idle with no info', () => {
    const s = useUpdateStore.getState();
    expect(s.status).toBe('idle');
    expect(s.info).toBeNull();
    expect(s.dismissedVersion).toBeNull();
  });

  it('beginCheck -> checking', () => {
    check();
    expect(useUpdateStore.getState().status).toBe('checking');
  });

  it('setUpToDate clears info and progress', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setProgress({ downloaded: 10, total: 100 });
    useUpdateStore.getState().setUpToDate(check());
    const s = useUpdateStore.getState();
    expect(s.status).toBe('idle');
    expect(s.info).toBeNull();
    expect(s.progress).toBeNull();
  });

  // A failed check from an IDLE store is the plain case; the cases where
  // something actionable is already on screen are pinned in the
  // "a failed re-check" block below (they used to go idle, which was the bug).
  it('setCheckFailed goes idle when nothing was on offer', () => {
    useUpdateStore.getState().setCheckFailed(check());
    const s = useUpdateStore.getState();
    expect(s.status).toBe('idle');
    expect(s.info).toBeNull();
  });

  it('setAvailable -> available with info', () => {
    seedAvailable();
    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual(INFO);
  });

  it('startDownloading -> downloading, setProgress updates progress', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    expect(useUpdateStore.getState().status).toBe('downloading');
    useUpdateStore.getState().setProgress({ downloaded: 50, total: 100 });
    expect(useUpdateStore.getState().progress).toEqual({ downloaded: 50, total: 100 });
  });

  it('setReady -> ready', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setReady(INFO.version);
    expect(useUpdateStore.getState().status).toBe('ready');
  });

  it('setDownloadFailed with existing info falls back to available', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setDownloadFailed();
    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.progress).toBeNull();
    expect(s.info).toEqual(INFO);
  });

  it('setDownloadFailed with no info falls back to error', () => {
    useUpdateStore.getState().setDownloadFailed();
    expect(useUpdateStore.getState().status).toBe('error');
  });

  it('dismiss records the dismissed version, session only', () => {
    useUpdateStore.getState().dismiss('1.2.3');
    expect(useUpdateStore.getState().dismissedVersion).toBe('1.2.3');
  });
});

/**
 * [#579 review] Three callers can have a check in flight at once (launch hook,
 * track switch, About button) and the replies race. Without the sequence the
 * SLOWER reply wins, so the banner describes the track the user just left.
 */
describe('a check reply that has been overtaken is dropped', () => {
  it('a stale setAvailable cannot overwrite a newer check', () => {
    const stale = check();
    const fresh = check();

    useUpdateStore.getState().setAvailable(fresh, { version: '2.0.0', notesUrl: 'https://new' });
    useUpdateStore.getState().setAvailable(stale, { version: '1.0.0', notesUrl: 'https://old' });

    expect(useUpdateStore.getState().info).toEqual({ version: '2.0.0', notesUrl: 'https://new' });
  });

  it('a stale setUpToDate cannot erase a newer available', () => {
    const stale = check();
    const fresh = check();

    useUpdateStore.getState().setAvailable(fresh, INFO);
    useUpdateStore.getState().setUpToDate(stale);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual(INFO);
  });

  /**
   * [review] A failed RE-check dropped an actionable banner to idle even though
   * the backend's parked Update - and any completed download - was untouched: a
   * check that never reached the server does not bump the generation either. A
   * finished "Restart to update" vanished on one network blip, unrecoverable
   * short of another successful check.
   */
  it('a failed re-check leaves an available banner alone', () => {
    seedAvailable();
    useUpdateStore.getState().setCheckFailed(check());

    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual(INFO);
  });

  it('a failed re-check leaves a FINISHED download alone', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setReady(INFO.version);

    useUpdateStore.getState().setCheckFailed(check());

    expect(useUpdateStore.getState().status).toBe('ready');
  });

  it('a failed check with nothing to lose still goes idle', () => {
    useUpdateStore.getState().setCheckFailed(check());
    expect(useUpdateStore.getState().status).toBe('idle');
  });

  /** ...and the "checking" state itself must not blank the banner on the way in. */
  it('beginCheck does not blank an actionable banner', () => {
    seedAvailable();
    check();
    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual(INFO);
  });

  it('a stale setCheckFailed cannot blank a newer answer', () => {
    const stale = check();
    const fresh = check();

    useUpdateStore.getState().setAvailable(fresh, INFO);
    useUpdateStore.getState().setCheckFailed(stale);

    expect(useUpdateStore.getState().status).toBe('available');
  });

  it('the latest reply still lands', () => {
    check();
    const fresh = check();
    useUpdateStore.getState().setAvailable(fresh, INFO);
    expect(useUpdateStore.getState().status).toBe('available');
  });
});

/**
 * [#579 review] The wedge this prevents: `ready` carrying `info` from a
 * different check, i.e. a Restart button that installs a version nobody was
 * offered - or, with `info` nulled by an intervening up-to-date reply, one that
 * can only fail.
 */
describe('download events are pinned to the version they started for', () => {
  it('setReady is ignored when a later check replaced the offer', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);

    // A check lands mid-download and finds a different version.
    useUpdateStore.getState().setAvailable(check(), { version: '9.9.9', notesUrl: 'https://n' });
    useUpdateStore.getState().setReady(INFO.version);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('available');
    expect(s.info).toEqual({ version: '9.9.9', notesUrl: 'https://n' });
  });

  it('setReady is ignored when the offer was withdrawn entirely', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setUpToDate(check());

    useUpdateStore.getState().setReady(INFO.version);

    const s = useUpdateStore.getState();
    expect(s.status).toBe('idle');
    // The wedge: ready with no info at all.
    expect(s.info).toBeNull();
  });

  it('setProgress is ignored once the download is no longer running', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setReady(INFO.version);

    useUpdateStore.getState().setProgress({ downloaded: 1, total: 2 });

    expect(useUpdateStore.getState().progress).toBeNull();
    expect(useUpdateStore.getState().status).toBe('ready');
  });

  it('a progress tick from before any download never lands', () => {
    seedAvailable();
    useUpdateStore.getState().setProgress({ downloaded: 1, total: 2 });
    expect(useUpdateStore.getState().progress).toBeNull();
  });

  it('the matching version still readies', () => {
    seedAvailable();
    useUpdateStore.getState().startDownloading(INFO.version);
    useUpdateStore.getState().setProgress({ downloaded: 1, total: 2 });
    useUpdateStore.getState().setReady(INFO.version);
    expect(useUpdateStore.getState().status).toBe('ready');
  });
});
