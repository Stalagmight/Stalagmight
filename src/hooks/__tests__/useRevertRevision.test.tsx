/**
 * The revert composition (lock -> restore -> push & release). It has no backend verb
 * of its own, so the invariants that matter are all about WHAT IT LEAVES BEHIND when
 * a middle step fails: never take a lock we already know will be refused, hand a lock
 * back only while the copy is clean, and never auto-release over un-pushed work.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { renderHook, act } from '@testing-library/react';

vi.mock('../../utils/ipc');
stubGetErrorMessage(messageOr('error'));

import { useRevertRevision, type RevertOutcome, type RevertRevisionArgs } from '../useRevertRevision';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import { invokeIpcMock, stubGetErrorMessage, messageOr } from '../../test/factories/ipc';
import { makeSpeleoLink } from '../../test/factories/speleo';

/** [#576 NF-W8FE-2] Done and Push answer the wire envelope, never a bare outcome. */
function report<R>(result: R): { result: R; save_warnings: [] } {
  return { result, save_warnings: [] };
}

const LINK: SpeleoLinkState = makeSpeleoLink();

const ARGS: RevertRevisionArgs = {
  projectId: 'p1',
  projectName: 'Cave A',
  hexsha: 'abcdef0123',
  shortSha: 'abcdef01',
  message: 'undo the bad survey',
};

const RESTORED = { local_path: '/cache/p1.stalx', concurrent_session: false };

const UNPUSHED_REFUSAL = {
  message: 'local edits never pushed',
  code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
  unpushed_local: {
    project_id: 'p1',
    local_path: '/cache/p1.stalx',
    last_edit_at: '2026-07-26T09:00:00Z',
  },
};

/** Route each command to a canned outcome; anything unlisted resolves to null. */
function ipc(routes: Record<string, () => unknown>): void {
  invokeIpcMock.mockImplementation(async (command: string) => routes[command]?.() ?? null);
}
const ok = (value: unknown) => () => value;
const rejects = (error: unknown) => () => {
  throw error;
};

/** The speleodb_* commands actually issued, in order. */
const issued = () =>
  invokeIpcMock.mock.calls.map((c) => c[0] as string).filter((c) => c.startsWith('speleodb_'));

const argsOf = (command: string) =>
  invokeIpcMock.mock.calls.find((c) => c[0] === command)?.[1] as Record<string, unknown> | undefined;

const run = async (over: Partial<RevertRevisionArgs> = {}): Promise<RevertOutcome> => {
  const { result } = renderHook(() => useRevertRevision());
  let outcome: RevertOutcome | undefined;
  await act(async () => {
    outcome = await result.current({ ...ARGS, ...over });
  });
  return outcome as RevertOutcome;
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({
    link: { ...LINK },
    remoteProjectId: 'p1',
    divergence: null,
    banner: null,
    busy: false,
    refreshLinkStatus: vi.fn(async () => {}),
  });
  useProjectStore.setState({
    refreshProjectState: vi.fn(async () => {}),
    checkProjectIntegrity: vi.fn(async () => []),
    saveProject: vi.fn(async () => ({ success: true }) as never),
    isDirty: false,
  });
  useUIStore.setState({ showToast: vi.fn(), setView: vi.fn(), clearSelection: vi.fn() });
});

describe('useRevertRevision - the happy path', () => {
  it('locks, restores, then pushes & releases - in that order, with those arguments', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual([
      // [#258] The lock-free probe leads: the "which file?" question is settled
      // before anything is acquired.
      'speleodb_dump_candidates',
      'speleodb_start_editing',
      'speleodb_restore_revision',
      // [#576 NF-W2F-1] the restored tree re-reads the open file's flags
      'speleodb_open_file_flags',
      'speleodb_done',
    ]);
    expect(argsOf('speleodb_restore_revision')).toEqual({
      projectId: 'p1',
      projectName: 'Cave A',
      hexsha: 'abcdef0123',
      restore: true,
      // The first attempt never waives the un-pushed-edits guard.
      discardLocal: false,
      // [#258] A first attempt sends no chosen entry, which is what makes the
      // backend classify the dump normally rather than honour a stale pick.
      entryPath: null,
      // [#303] A revert is not an Unwind: it rides the old tree FORWARD, so the
      // downloaded copy's undo history is dropped like any other adopt.
      keepUndoHistory: false,
      // ...and with no history kept there is no un-pushed-work waiver to acknowledge.
      // Pinned false deliberately: this flag is the ONE thing that lets the backend
      // discard work the user never pushed, and no path but the Unwind confirmation
      // may ever carry it.
      discardUnpushed: false,
      // A revert walks no chain, so there is no direction to select a guard with.
      chainForward: false,
    });
    expect(argsOf('speleodb_done')).toEqual({
      message: 'undo the bad survey',
      acceptOlderVersion: false,
    });
  });

  it('reports each step as it starts', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });
    const steps: string[] = [];

    await run({ onStep: (s) => steps.push(s) });

    expect(steps).toEqual(['locking', 'restoring', 'pushing']);
  });
});

describe('useRevertRevision - the resume is identity-checked too (#258)', () => {
  it('does not publish a replacement project when the resume finds the context stale', async () => {
    // `resumeAtPush` used to return BEFORE the identity check, two hunks under a
    // comment claiming that check ran "at ENTRY". `publish` calls `done(message)` -
    // argument-free, acting on whatever is open, with a real commit message - so it
    // PUSHES. A retry after a project switch therefore uploaded the replacement and
    // released its lock.
    useSpeleoStore.setState({ remoteProjectId: 'p2' });
    ipc({ speleodb_done: ok(report({ kind: 'released' })) });

    expect(await run({ resumeAtPush: true })).toEqual({ kind: 'project_changed' });
    expect(issued()).toEqual([]);
  });

  it('still resumes normally when the project is the one it was asked about', async () => {
    // The other half: the pre-flights must STAY behind the resume, because the restore
    // is what set the bit the un-pushed guard tests. Only the identity moved ahead.
    useSpeleoStore.setState({
      remoteProjectId: 'p1',
      link: { ...LINK, hasUnpushedEdits: true },
    });
    useProjectStore.setState({ isDirty: true });
    ipc({ speleodb_done: ok(report({ kind: 'released' })) });

    expect(await run({ resumeAtPush: true })).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual(['speleodb_done']);
  });
});

describe('useRevertRevision - the swapped tree owns no stale entity refs (#258)', () => {
  it('resets every per-project tool ref, not just the selection', async () => {
    // The swap replaces the working tree without going through `loadProject`, which is
    // the only thing that resets these - so `clearSelection` was the whole reset and
    // merge/comment/measure refs survived. #258 is what makes that dangerous: two
    // `.stalx` under one project are independent databases with independently assigned
    // section ids, so a picker answer can swap in a DIFFERENT survey whose ids collide,
    // and a half-armed merge would resolve its endpoint against the wrong station.
    useUIStore.setState({
      mergeStationA: { section_id: 1, station_index: 0 },
      mergeStationB: null,
      commentTarget: { sectionId: 1, shotId: 7 } as never,
    });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    expect(await run()).toEqual({ kind: 'reverted' });

    const ui = useUIStore.getState();
    expect(ui.mergeStationA).toBeNull();
    expect(ui.commentTarget).toBeNull();
  });
});

describe('useRevertRevision - refusing before it takes anything', () => {
  it('needs a discard for an un-pushed copy WITHOUT acquiring the lock', async () => {
    // The whole point of the pre-flight: the restore would be refused anyway, so
    // acquiring first would block another user for the length of a call we already
    // know fails.
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
    ipc({});

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'needs_discard', lockHeld: false, stateUnknown: false });
    expect(issued()).toEqual([]);
  });

  it('refuses over edits that are only in memory, WITHOUT acquiring the lock', async () => {
    // The restore closes and reopens the project at the checked-out tree, so an
    // unsaved edit is gone - and `useRestoreRevision` clears `isDirty` behind it, so
    // nothing is left that says it existed. The persisted SpeleoDB bit usually covers
    // this, but `mark_remote_dirty` logs and swallows a failure to write it, and once
    // the lock is lost the link can read clean with `isDirty` the only evidence left.
    useProjectStore.setState({ isDirty: true });
    ipc({});

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'unsaved_local' });
    expect(issued()).toEqual([]);
  });

  it('checks disk-dirty BEFORE the un-pushed bit', async () => {
    // A project dirty in both senses must not be sent to the armed discard first: that
    // waiver saves nothing, so confirming it would destroy the in-memory work too,
    // under copy ("changes that are not on SpeleoDB") that never mentions it. Saving
    // first folds those edits into the un-pushed set that copy then describes.
    useProjectStore.setState({ isDirty: true });
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
    ipc({});

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'unsaved_local' });
  });

  it('is NOT waived by the armed discard press', async () => {
    // `discardLocal` answers the un-pushed question only. Nothing in that confirmation
    // asked about work that has never reached the disk.
    useProjectStore.setState({ isDirty: true });
    ipc({});

    const outcome = await run({ discardLocal: true });

    expect(outcome).toEqual({ kind: 'unsaved_local' });
    expect(issued()).toEqual([]);
  });

  it('reports the holder when someone else has the lock', async () => {
    ipc({ speleodb_start_editing: ok({ kind: 'locked_by', email: 'other@x' }) });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'locked_by', email: 'other@x' });
    expect(issued()).toEqual(['speleodb_dump_candidates', 'speleodb_start_editing']);
  });

  it('reports a divergence and unwinds nothing (the backend released its own lock)', async () => {
    ipc({ speleodb_start_editing: ok({ kind: 'divergence_needed' }) });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'diverged' });
    expect(issued()).toEqual(['speleodb_dump_candidates', 'speleodb_start_editing']);
  });

  it('reports a plain lock failure when nothing else explains it', async () => {
    ipc({ speleodb_start_editing: rejects({ message: 'offline' }) });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'lock_failed' });
  });
});

describe('useRevertRevision - unwinding a failed restore', () => {
  it('keeps the lock and uploads nothing when the restore is refused for local edits', async () => {
    // The refusal IS the backend saying its dirty bit is set, so releasing here
    // would PUBLISH the un-pushed work: `speleodb_done` pushes whenever the backend
    // link is dirty, and `message: null` only picks the commit message - it does not
    // suppress the upload. The old behaviour read the FRONTEND mirror, which the
    // pre-flight has already forced to false on this route, and so released - the
    // user's survey work went up as a revision with no message and no confirmation,
    // and only then were they offered the armed "this replaces your un-pushed
    // changes" button for work that was already published.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects(UNPUSHED_REFUSAL),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'needs_discard', lockHeld: true, stateUnknown: false });
    expect(issued()).not.toContain('speleodb_done');
  });

  it('carries a copy that could not be read through, and still keeps the lock (#576 W55)', async () => {
    // [NF-W53-1] Same refusal, same unwind - the copy could not be read, so it cannot be
    // shown clean and the lock stays - but the outcome says so, so the dialog can word
    // it as unreadable rather than as un-pushed changes.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects({
        ...UNPUSHED_REFUSAL,
        unpushed_local: { ...UNPUSHED_REFUSAL.unpushed_local, last_edit_at: null, state_unknown: true },
      }),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'needs_discard', lockHeld: true, stateUnknown: true });
    expect(issued()).not.toContain('speleodb_done');
  });

  it('releases and reports the failure for an ordinary restore error', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects({ message: 'offline' }),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toMatchObject({ kind: 'restore_failed', lockHeld: false });
    expect(issued()).toContain('speleodb_done');
  });

  it('reports NO lock held when the release says the edit gate had already dropped', async () => {
    // `lockHeld` answers "am I now holding a lock I did not have?", and `not_editing`
    // is the backend saying the gate - and with it the lock - was already gone (a 401
    // auto-drop is the usual way in). Folding every non-`released` answer into
    // `lockHeld: true` made the dialog say "You still hold the write lock" about a
    // lock that demonstrably was not held.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects({ message: 'offline' }),
      speleodb_done: ok(report({ kind: 'not_editing' })),
    });

    const outcome = await run();

    expect(outcome).toMatchObject({ kind: 'restore_failed', lockHeld: false });
  });

  it('stays on the pessimistic answer when the release itself throws', async () => {
    // A thrown release leaves the lock genuinely unknown. Being sent to check a lock
    // we no longer hold costs a trip; being told nothing about one we DO hold strands
    // a server mutex, so "unknown" reports as held.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects({ message: 'offline' }),
      speleodb_done: rejects({ message: 'offline' }),
    });

    const outcome = await run();

    expect(outcome).toMatchObject({ kind: 'restore_failed', lockHeld: true });
  });

  it('KEEPS the lock when the copy is dirty - releasing would strand un-pushed work', async () => {
    // The armed-discard retry waives the pre-flight, so the flow reaches the backend
    // with un-pushed edits still present and fails there for its own reason. Releasing
    // now would leave that work on this computer with nothing protecting it, and a
    // done(message) would publish edits nobody asked to publish.
    useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects({ message: 'offline' }),
    });

    const outcome = await run({ discardLocal: true });

    expect(outcome).toMatchObject({ kind: 'restore_failed', lockHeld: true });
    expect(issued()).not.toContain('speleodb_done');
  });
});

describe('useRevertRevision - a restore that landed but a push that did not', () => {
  it('reports push_failed and does NOT try to release', async () => {
    // Auto-releasing here would leave the reverted tree as un-pushed edits with no
    // lock over them - the exact state the #260 banner's Retry exists to resolve.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: rejects({ message: 'offline' }),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'push_failed' });
    expect(issued().filter((c) => c === 'speleodb_done')).toHaveLength(1);
  });

  it('separates a dropped session from a failed push, because the advice differs', async () => {
    // `not_editing` means the edit gate went away mid-flight (a 401 auto-drop is the
    // usual way in): nothing uploaded, nothing released, and NO lock held. It reaches
    // the same dead end as push_failed, but that outcome's copy tells the user they
    // are holding the write lock and can Retry from the banner - which here would
    // send them at a button that cannot work.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'not_editing' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'not_editing' });
    // Still no release attempt: there is nothing to release.
    expect(issued().filter((c) => c === 'speleodb_done')).toHaveLength(1);
  });

  it('reports push_unconfirmed rather than claiming the lock came back', async () => {
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'push_unconfirmed' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'push_unconfirmed' });
  });
});

/**
 * `resumeAtPush` exists for the state a failed (or unconfirmed) upload leaves behind:
 * the revision IS in the working copy and the lock IS still held, so the upload is the
 * only step outstanding. Re-running the composite there re-downloads and re-swaps a
 * tree that is already correct, and - on the attempt whose upload really landed -
 * routes the retry through the acquire's reconcile, which has no authorship recovery
 * and so calls our own commit a divergence and releases the lock to prompt about it.
 */
describe('useRevertRevision - resuming at the push', () => {
  beforeEach(() => {
    // The state the resume is for, as the restore leaves it: lock held, dirty bit set.
    useSpeleoStore.setState({ link: { ...LINK, editState: 'editing', hasUnpushedEdits: true } });
  });

  it('issues ONLY the push - no second acquire, no second restore', async () => {
    // Every verb is routed, so anything the resume issues shows up in `issued()`.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run({ resumeAtPush: true });

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual(['speleodb_done']);
    expect(issued()).not.toContain('speleodb_start_editing');
    expect(issued()).not.toContain('speleodb_restore_revision');
    // And with the SAME message: a resume publishes the revert the user confirmed.
    expect(argsOf('speleodb_done')).toEqual({
      message: 'undo the bad survey',
      acceptOlderVersion: false,
    });
  });

  it('runs past the unsaved-edits guard too, however the dirty flag reads', async () => {
    // The resume must not be refusable on either pre-flight. The restore leaves the
    // working copy saved, so this should not arise - but a stale flag stranding the
    // user mid-revert, holding a lock with a retry they cannot press, is the one
    // failure this control exists to prevent.
    useProjectStore.setState({ isDirty: true });
    ipc({ speleodb_done: ok(report({ kind: 'released' })) });

    const outcome = await run({ resumeAtPush: true });

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual(['speleodb_done']);
  });

  it('runs past the pre-flight guard the restore itself tripped', async () => {
    // `hasUnpushedEdits` is set by the restore, so the ordinary route would refuse
    // this retry outright - offering to "replace my changes" over the user's own
    // revert. The resume must not consult that guard at all.
    ipc({ speleodb_done: ok(report({ kind: 'released' })) });

    const outcome = await run({ resumeAtPush: true });

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual(['speleodb_done']);
  });

  it('reports the pushing step and no other', async () => {
    ipc({ speleodb_done: ok(report({ kind: 'released' })) });
    const steps: string[] = [];

    await run({ resumeAtPush: true, onStep: (s) => steps.push(s) });

    expect(steps).toEqual(['pushing']);
  });

  it('maps a still-failing resume through the same tail as the first attempt', async () => {
    // One copy of the done-result -> outcome mapping: a resume that fails again has
    // to describe itself exactly as the first failure did, or the dialog would offer
    // a different way out of the same state.
    ipc({ speleodb_done: rejects({ message: 'still offline' }) });

    const outcome = await run({ resumeAtPush: true });

    expect(outcome).toEqual({ kind: 'push_failed' });
    expect(issued()).toEqual(['speleodb_done']);
  });

  it('maps an unconfirmed resume through that tail too', async () => {
    ipc({ speleodb_done: ok(report({ kind: 'push_unconfirmed' })) });

    const outcome = await run({ resumeAtPush: true });

    expect(outcome).toEqual({ kind: 'push_unconfirmed' });
    expect(issued()).toEqual(['speleodb_done']);
  });
});

describe('useRevertRevision - an ambiguous dump is a question, not a failure (#258)', () => {
  const AMBIGUOUS = {
    message: 'more than one Stalagmight file',
    code: 'SPELEODB_AMBIGUOUS',
    dump_candidates: [
      { entry_path: 'misc/a.stalx', size_bytes: 10, modified: null },
      { entry_path: 'misc/b.stalx', size_bytes: 20, modified: null },
    ],
  };

  it('asks BEFORE taking the lock when the probe finds a choice', async () => {
    // The point of the probe: no lock is taken at all, so there is nothing to hand
    // back. Contrast the case below, which is the fallback for when the probe could
    // not know - the composite gets as far as the restore and unwinds.
    ipc({
      speleodb_dump_candidates: ok(AMBIGUOUS.dump_candidates),
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'awaiting_dump_choice', candidates: AMBIGUOUS.dump_candidates });
    expect(issued()).toEqual(['speleodb_dump_candidates']);
  });

  it('abandons the composite when the open project changes under the probe', async () => {
    // The probe is the only step here that can involve a dump download, so it is the
    // longest, and it is the last point at which the project is still named: after it
    // `startEditing()` takes no argument and locks whatever is open NOW, while the
    // restore still targets the captured id. A swap in between would take a write
    // lock on one project and check a revision out of another - and the candidates
    // would raise a picker describing a server tree nobody is looking at.
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_dump_candidates') {
        // The user closes this dialog and opens something else while the probe is
        // still in flight.
        useSpeleoStore.setState({ remoteProjectId: 'p2' });
        return AMBIGUOUS.dump_candidates;
      }
      if (cmd === 'speleodb_start_editing') return { kind: 'editing' };
      if (cmd === 'speleodb_restore_revision') return RESTORED;
      if (cmd === 'speleodb_done') return report({ kind: 'released' });
      return null;
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'project_changed' });
    // Nothing acquired, nothing written - so there is nothing to unwind either.
    expect(issued()).toEqual(['speleodb_dump_candidates']);
  });

  it('abandons a picker retry whose project changed while the dialog was open', async () => {
    // The retry re-enters the composite from the top with `entryPath` set, which
    // SKIPS the probe branch and every check inside it - so the guard that only lived
    // there covered the shorter wait and missed the longer one. The picker is an
    // interaction of unbounded length; the probe is a download.
    useSpeleoStore.setState({ remoteProjectId: 'p2' });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run({ entryPath: 'misc/b.stalx' });

    expect(outcome).toEqual({ kind: 'project_changed' });
    expect(issued()).toEqual([]);
  });

  it('refuses to restore over edits that arrived while the probe was out', async () => {
    // The unsaved-work guard runs at entry, before a download that can take as long as
    // it likes. An edit landing during it was still in memory when the restore closed
    // and reopened the project at the checked-out tree - and `useRestoreRevision`
    // force-clears `isDirty` behind it, so nothing would have said it ever existed.
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_dump_candidates') {
        useProjectStore.setState({ isDirty: true });
        return null;
      }
      if (cmd === 'speleodb_start_editing') return { kind: 'editing' };
      if (cmd === 'speleodb_restore_revision') return RESTORED;
      if (cmd === 'speleodb_done') return report({ kind: 'released' });
      return null;
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'unsaved_local' });
    // Nothing acquired: the re-check sits in front of the lock, not behind it.
    expect(issued()).toEqual(['speleodb_dump_candidates']);
  });

  it('hands back the lock it just took when the project changed during the acquire', async () => {
    // The acquire is a round trip of its own, and the restore on the other side of it
    // replaces the whole working tree. Walking away without releasing would strand a
    // server mutex for the rest of the holder's session, so the mismatch unwinds like
    // every other failure arm - and `done(null)` is correct here by construction,
    // because the restore that sets the dirty bit has not run.
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_dump_candidates') return null;
      if (cmd === 'speleodb_start_editing') {
        useSpeleoStore.setState({ remoteProjectId: 'p2' });
        return { kind: 'editing' };
      }
      if (cmd === 'speleodb_restore_revision') return RESTORED;
      if (cmd === 'speleodb_done') return report({ kind: 'released' });
      if (cmd === 'speleodb_release_lock_acquired_here') return null;
      return null;
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'project_changed' });
    // Acquired, unwound, and never restored.
    expect(issued()).toEqual([
      'speleodb_dump_candidates',
      'speleodb_start_editing',
      'speleodb_release_lock_acquired_here',
    ]);
    // Through the PROVENANCE-SCOPED verb, and through neither of the two things that
    // look like they would do. `done` is argument-free and acts on whatever is OPEN -
    // which this branch has just proved is a different project - and `message: null`
    // picks the commit message rather than suppressing the upload, so unwinding through
    // it would push the replacement whenever it was dirty and editable.
    // `speleodb_release_lock(projectId)` targets an ARBITRARY project by design, so it
    // would surrender a lock this account inherited from a sibling session, and would
    // name an id that is not necessarily the one `startEditing` locked.
    expect(issued()).not.toContain('speleodb_done');
    expect(issued()).not.toContain('speleodb_release_lock');
    // No argument: only the backend knows what it took.
    expect(argsOf('speleodb_release_lock_acquired_here')).toBeUndefined();
  });

  it('leaves an INHERITED same-account lock alone when the project changes', async () => {
    // The destructive half. An acquire that met a lock this account already held
    // inherited it from a sibling session guarding its own un-pushed work, and
    // `speleodb_start_editing` is explicit that no unwind may hand that back. The
    // frontend cannot tell the two apart - `startEditing` reduces every success to
    // `true` - so the decision has to sit in the backend, and this test's job is to
    // prove the frontend does not make it. It asks unconditionally; the no-op is the
    // backend's answer, which its own unit tests cover.
    let released = 0;
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_dump_candidates') return null;
      if (cmd === 'speleodb_start_editing') {
        useSpeleoStore.setState({ remoteProjectId: 'p2' });
        return { kind: 'editing' };
      }
      if (cmd === 'speleodb_release_lock_acquired_here') {
        released += 1;
        return null;
      }
      return null;
    });

    expect(await run()).toEqual({ kind: 'project_changed' });
    // Exactly one ask, carrying no id for the backend to misapply.
    expect(released).toBe(1);
    expect(issued()).not.toContain('speleodb_release_lock');
    expect(issued()).not.toContain('speleodb_done');
  });

  it('still reports the project change when the release of that lock fails', async () => {
    // The outcome is already decided by then. Reporting a release failure instead
    // would name the smaller problem, and the stranded lock stays visible in the
    // held-lock resume prompt either way.
    invokeIpcMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'speleodb_dump_candidates') return null;
      if (cmd === 'speleodb_start_editing') {
        useSpeleoStore.setState({ remoteProjectId: 'p2' });
        return { kind: 'editing' };
      }
      if (cmd === 'speleodb_release_lock_acquired_here')
        throw { message: 'offline', code: 'SPELEODB_RETRY' };
      return null;
    });

    expect(await run()).toEqual({ kind: 'project_changed' });
  });

  it('does not probe on the retry - the answer is already in hand', async () => {
    ipc({
      speleodb_dump_candidates: ok(AMBIGUOUS.dump_candidates),
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run({ entryPath: 'misc/b.stalx' });

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).not.toContain('speleodb_dump_candidates');
  });

  it('a probe that fails is advisory only and never blocks the revert', async () => {
    // It reports nothing of its own: the flow proceeds and the real call stays the
    // one authoritative place a problem is surfaced.
    ipc({
      speleodb_dump_candidates: rejects({ message: 'offline', code: 'SPELEODB_RETRY' }),
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    expect(await run()).toEqual({ kind: 'reverted' });
  });

  it('hands the lock back and reports the choice, NOT a restore failure', async () => {
    // The reported bug: this used to come back as `restore_failed`, so the dialog
    // painted "The write lock was released and nothing was changed" in red while a
    // picker sat behind it offering a retry the backend would refuse. Both halves
    // are pinned here - the release DID happen (so a retry can re-acquire), and the
    // outcome says what is actually going on.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects(AMBIGUOUS),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'awaiting_dump_choice', candidates: AMBIGUOUS.dump_candidates });
    expect(issued()).toContain('speleodb_done');
  });

  it('re-runs the WHOLE composite on the retry, re-acquiring the lock', async () => {
    // The heart of the fix. A retry that re-ran only the restore step would be
    // refused: `restore: true` needs a held lock, and the attempt above gave it back.
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run({ entryPath: 'misc/b.stalx' });

    expect(outcome).toEqual({ kind: 'reverted' });
    expect(issued()).toEqual([
      'speleodb_start_editing',
      'speleodb_restore_revision',
      'speleodb_open_file_flags',
      'speleodb_done',
    ]);
    expect(argsOf('speleodb_restore_revision')?.entryPath).toBe('misc/b.stalx');
  });

  it('does NOT offer the choice while the copy is dirty, because the retry could not work', async () => {
    // A dump can be ambiguous AND the copy un-pushed: the backend resolves the entry
    // before it consults the local-edits policy. There the lock is deliberately KEPT,
    // so a composite retry would be refused by the revert's own un-pushed pre-flight.
    // Offering a picker there would reproduce the reported bug in a rarer form.
    // Clean at the pre-flight (or the revert would refuse before it ever restored),
    // then dirty by the time the catch re-reads it - an autosave landing in between
    // is exactly the race the unwind's refresh exists to catch.
    useSpeleoStore.setState({
      link: { ...LINK },
      refreshLinkStatus: vi.fn(async () => {
        useSpeleoStore.setState({ link: { ...LINK, hasUnpushedEdits: true } });
      }),
    });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: rejects(AMBIGUOUS),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome.kind).toBe('restore_failed');
    expect(outcome).toMatchObject({ lockHeld: true });
    expect(issued()).not.toContain('speleodb_done');
  });
});

/**
 * #576 RV4-FE-SP4 (decided RV2-R-15: Restore revision is refused in the read-only Plan and
 * Export workspaces). The History tab is reachable there through the hamburger's SpeleoDB
 * menu, and a Revert took the server write lock (and could fast-forward) before the
 * restore was refused - a lock round trip for an action that could not succeed. It now
 * refuses before any IPC, and again right before the acquire.
 */
describe('useRevertRevision - the read-only Plan/Export workspace (#576 RV4-FE-SP4)', () => {
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey' });
  });

  it.each(['plan', 'export'] as const)('refuses in %s without taking the lock', async (mode) => {
    useUIStore.setState({ appMode: mode });
    ipc({
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    console.log(`[SP4] revert in ${mode}: outcome=${JSON.stringify(outcome)} ipc=${JSON.stringify(issued())}`);
    expect(outcome).toEqual({ kind: 'workspace_read_only' });
    expect(issued()).toEqual([]);
  });

  it('refuses before the acquire when the workspace changed during the probe', async () => {
    ipc({
      speleodb_dump_candidates: () => {
        useUIStore.setState({ appMode: 'plan' });
        return null;
      },
      speleodb_start_editing: ok({ kind: 'editing' }),
      speleodb_restore_revision: ok(RESTORED),
      speleodb_done: ok(report({ kind: 'released' })),
    });

    const outcome = await run();

    expect(outcome).toEqual({ kind: 'workspace_read_only' });
    expect(issued()).toEqual(['speleodb_dump_candidates']);
  });
});
