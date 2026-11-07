// @vitest-environment jsdom
/**
 * Hook test (#218) - the shared pull+open flow for remote SpeleoDB projects.
 * Covers the outcome routing GlobalView + the browser dialog both depend on: a
 * normal open, the Ariane-adopt route, a load failure that rolls the link back, and
 * an open error that reports a re-login.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { failOnRefreshErrors } from '../test/refreshGuard';
import { renderHook } from '@testing-library/react';

vi.mock('../utils/ipc');
// [#576 NF-W6A-5] The refresh every edit/import ends with reads bytes; a test that
// gives none answers "no project open" rather than making the decoder throw.
emptyBinaryRefresh();
stubGetErrorMessage(fallbackOr('error'));
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ close: vi.fn(async () => {}), destroy: vi.fn(async () => {}) }),
}));

import { useOpenRemoteProject } from './useOpenRemoteProject';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore, selectSpeleoReadOnly, type SpeleoLinkState } from '../store/speleoStore';
import { useUIStore } from '../store/uiStore';
import { guardQuit } from '../utils/quit';
import type { SpeleoProjectSummary } from '../types/ipc.generated';
import { emptyBinaryRefresh, fallbackOr, invokeIpcMock, stubGetErrorMessage } from '../test/factories/ipc';
import { makeSpeleoLink } from '../test/factories/speleo';

// The genuine store action, captured before `beforeEach` swaps in the spy - so one
// case can exercise the real seed end to end (seed -> guardQuit).
const realSetLinkOnOpen = useSpeleoStore.getState().setLinkOnOpen;

const SUMMARY: SpeleoProjectSummary = {
  id: 'p1',
  name: 'Cave A',
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  type_tag: 'stalx',
  latest_commit_sha: 'abc123',
};

/** A settled link for whatever project is OPEN, for the entry-guard cases below. */
const openLink = (o: Partial<SpeleoLinkState> = {}): SpeleoLinkState =>
  makeSpeleoLink({ ...o });

let loadProjectMock: ReturnType<typeof vi.fn>;
let setLinkOnOpenMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  invokeIpcMock.mockReset();
  loadProjectMock = vi.fn(async () => ({ success: true }));
  setLinkOnOpenMock = vi.fn();
  // `keepLocalCopy` compares the refused path against the LIVE open project, so the
  // "nothing open" baseline has to be explicit.
  useProjectStore.setState({
    loadProject: loadProjectMock,
    project: null,
    isDirty: false,
    checkProjectIntegrity: vi.fn(async () => []),
  });
  useSpeleoStore.setState({
    setLinkOnOpen: setLinkOnOpenMock,
    link: null,
    // The open remote project's id, which the entry guard reads to tell "the target
    // IS what is on screen" from a swap. Reset explicitly - the store is shared.
    remoteProjectId: null,
  });
  useUIStore.setState({ showToast: vi.fn(), openDialog: vi.fn() });
});

const hook = (opts: Parameters<typeof useOpenRemoteProject>[0] = {}) =>
  renderHook(() => useOpenRemoteProject(opts)).result.current;

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('useOpenRemoteProject (#218)', () => {
  it('opens a project: seeds the link, loads, and reports opened', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      // The path the seed belongs to, so the `syncProjectLink` inside the load
      // that follows keeps THIS project's link instead of the previous one's.
      forPath: '/cache/p1.stalx',
      // A PULL has, by construction, nothing local the server has not seen.
      hasUnpushedEdits: false,
      // And WHICH project, so `remoteProjectId` names it from the same synchronous
      // `set()` rather than after a status poll - which left every consumer keyed on
      // it reading the PREVIOUS project for the length of the poll interval.
      projectId: 'p1',
    });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
  });

  it('names the opened project immediately, so A -> B does not leave A on the store', async () => {
    // Driven through the REAL seed, because the bug is invisible to a test that sets
    // `remoteProjectId` by hand: it was written only by the status poll, so after
    // opening remote B from remote A the store named A for as long as a poll took
    // (30 s to a user-configurable 24 h). Everything keyed on it read A - the revision
    // history that lists revisions, and the revert guards that compare against the id
    // that history handed them, which is why both sides agreed on a stale A.
    useSpeleoStore.setState({
      setLinkOnOpen: realSetLinkOnOpen,
      remoteProjectId: 'p-previous',
      linkPath: null,
      link: null,
    });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });

    expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });

    // No poll has run - `loadProject` is mocked and `refreshLinkStatus` is not driven -
    // so this can only be the seed.
    expect(useSpeleoStore.getState().remoteProjectId).toBe('p1');
  });

  it('raises the file picker when the dump holds more than one openable file (#258)', async () => {
    // The refusal must NOT come back as `error`: nothing failed, the app simply has
    // no basis to choose. It reports `ambiguous` so the host clears its busy flag
    // without toasting a dead end, and the picker is raised from HERE so all three
    // hosts of this hook get the choice.
    const candidates = [
      { entry_path: 'misc/project.stalx', size_bytes: 4096, modified: null },
      { entry_path: 'backup/old.stalx', size_bytes: 2048, modified: null },
    ];
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_open_project') {
        // The retry carries the chosen entry, and only the retry succeeds - which is
        // what proves the picker's callback re-invokes the real open rather than
        // some parallel path.
        if (args?.entryPath === 'backup/old.stalx')
          return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
        });
      }
      return Promise.resolve(null);
    });

    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'ambiguous' });
    const picker = useSpeleoStore.getState();
    expect(picker.dumpPickerOpen).toBe(true);
    expect(picker.dumpPickerCandidates).toEqual(candidates);
    expect(picker.dumpPickerKind).toBe('stalx');
    expect(loadProjectMock).not.toHaveBeenCalled();

    // Answering it opens the chosen file.
    await useSpeleoStore.getState().resolveDumpPicker('backup/old.stalx');
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
  });

  it('hands an Ariane pick back for confirmation instead of ending silently (#258)', async () => {
    // The backend refuses to EXTRACT a chosen `.tml` and routes to adoption, so the
    // retry comes back `adopt`. Nobody is awaiting that call - the host asked once,
    // got `ambiguous`, and stopped listening - so without an explicit hand-back the
    // picker just closed: no confirmation, and `speleodb_adopt_ariane` never called.
    const candidates = [
      { entry_path: 'ariane.tml', size_bytes: 1, modified: null },
      { entry_path: 'misc/ariane.tml', size_bytes: 2, modified: null },
    ];
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_open_project') {
        if (args?.entryPath === 'misc/ariane.tml')
          return Promise.reject({ message: 'ariane', code: 'SPELEODB_ADOPT', status: null });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
        });
      }
      return Promise.resolve(null);
    });
    const onAdoptAfterPick = vi.fn();

    const outcome = await hook({ onAdoptAfterPick }).openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'ambiguous' });

    await useSpeleoStore.getState().resolveDumpPicker('misc/ariane.tml');
    expect(onAdoptAfterPick).toHaveBeenCalledWith(SUMMARY, false);
  });

  it('re-opens the picker when ADOPTION itself turns ambiguous (#258)', async () => {
    // The Ariane tree can change while the adopt confirmation sits open - one `.tml`
    // becoming several, or the remembered pick being replaced - so the adopt answers
    // with a fresh candidate set rather than a file. That is a routing signal;
    // collapsing it into a flat error stranded the user, who could not choose from
    // the new tree without restarting the entire open flow.
    const candidates = [
      { entry_path: 'one/ariane.tml', size_bytes: 1, modified: null },
      { entry_path: 'two/ariane.tml', size_bytes: 2, modified: null },
    ];
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_adopt_ariane') {
        if (args?.entryPath === 'two/ariane.tml')
          return Promise.resolve({
            local_path: '/cache/p1.stalx',
            type_flipped: true,
            concurrent_session: false,
          });
        return Promise.reject({
          message: 'more than one Ariane file',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
          dump_candidate_kind: 'ariane',
        });
      }
      return Promise.resolve(null);
    });

    const outcome = await hook().adoptRemote(SUMMARY, { discardLocal: true });
    expect(outcome).toEqual({ status: 'ambiguous' });
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);
    // Offered with the ADOPT verb, which is only reachable via the carried kind.
    expect(useSpeleoStore.getState().dumpPickerKind).toBe('ariane');
    expect(loadProjectMock).not.toHaveBeenCalled();

    await useSpeleoStore.getState().resolveDumpPicker('two/ariane.tml');
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    // The replay carries the discard the user already confirmed, so the backend's
    // un-pushed-edits guard does not re-ask a question they have answered.
    const adoptCalls = invokeIpcMock.mock.calls.filter((c) => c[0] === 'speleodb_adopt_ariane');
    const replay = adoptCalls[adoptCalls.length - 1];
    expect(replay?.[1]).toMatchObject({ entryPath: 'two/ariane.tml', discardLocal: true });
  });

  it('routes a fresh STALX ambiguity out of adoption instead of looping (#258)', async () => {
    // If a `.stalx` appears while the adopt confirmation sits open, the answer is to
    // extract it - and adoption accepts nothing but an Ariane entry. Replaying
    // adoption there re-raised the identical picker on every confirmation, so the
    // user could never get out of it.
    const candidates = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    let adoptCalls = 0;
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_adopt_ariane') {
        adoptCalls += 1;
        return Promise.reject({
          message: 'more than one Stalagmight file',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
          dump_candidate_kind: 'stalx',
        });
      }
      if (cmd === 'speleodb_open_project' && args?.entryPath === 'b.stalx')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });

    const outcome = await hook().adoptRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'ambiguous' });
    // Offered with the OPEN verb, matching what the replay will actually do.
    expect(useSpeleoStore.getState().dumpPickerKind).toBe('stalx');

    await useSpeleoStore.getState().resolveDumpPicker('b.stalx');
    // The pick opened the project rather than bouncing off adoption again.
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    expect(adoptCalls).toBe(1);
  });

  it('replays the DISCARDING open for a stalx ambiguity raised by adoption (#258)', async () => {
    // Same routing, but the user reached adoption through "take the server copy and
    // discard my local changes". Replaying the plain open would meet the un-pushed
    // edits guard and refuse a question they had already answered.
    const candidates = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.reject({
          message: 'more than one Stalagmight file',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
          dump_candidate_kind: 'stalx',
        });
      if (cmd === 'speleodb_open_project_discarding_local' && args?.entryPath === 'b.stalx')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY, { discardLocal: true });
    await useSpeleoStore.getState().resolveDumpPicker('b.stalx');
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    const opened = invokeIpcMock.mock.calls.filter(
      (c) => c[0] === 'speleodb_open_project_discarding_local',
    );
    expect(opened.length).toBe(1);
  });

  it('raises the picker from the CONFIRMED-DISCARD open too (#258)', async () => {
    // The only route that reaches an ambiguous dump with un-pushed local edits: the
    // guard runs before the download, so a protected open is refused before anything
    // is classified. Without this the user met a flat error immediately after
    // confirming they wanted their local copy discarded - the worst moment to dead-end.
    const candidates = [
      { entry_path: 'misc/a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'misc/b.stalx', size_bytes: 2, modified: null },
    ];
    invokeIpcMock.mockImplementation((cmd: string, args?: Record<string, unknown>) => {
      if (cmd === 'speleodb_open_project_discarding_local') {
        if (args?.entryPath === 'misc/b.stalx')
          return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: candidates,
        });
      }
      return Promise.resolve(null);
    });

    const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
    expect(outcome).toEqual({ status: 'ambiguous' });
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);
    expect(loadProjectMock).not.toHaveBeenCalled();

    // And the retry replays THIS verb, not the protected one - replaying the
    // protected open would be refused all over again by the guard.
    await useSpeleoStore.getState().resolveDumpPicker('misc/b.stalx');
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
  });

  it('does not raise the picker when the rejection carries no candidates (#258)', async () => {
    // The fast-forward verbs produce `Ambiguous` with nothing to act on, because
    // they have no way to send a choice back. Those must fall through to the
    // ordinary error path, whose message names the remedy - an empty picker would
    // be a dead end with extra steps.
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: [],
        });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome.status).toBe('error');
    expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
  });

  it('routes an Ariane (.tml-only) project to adopt', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({ message: 'ariane', code: 'SPELEODB_ADOPT', status: null });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'adopt' });
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  /** The receipt an adoption's conversion produces, with something to report. */
  const ADOPT_RECEIPT = {
    source_format: 'Ariane TML',
    file_name: 'ariane.tml',
    sections_found: 3,
    sections_imported: 2,
    shot_count: 12,
    closures_requested: 0,
    closures_created: 0,
    unit_conversion: null,
    warnings: [
      '2 excluded shots in the remote file were not imported',
      '1 section had no shots after filtering and was dropped',
    ],
    info: [],
  };

  const adoptReplying = (result: Record<string, unknown>) => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane') return Promise.resolve(result);
      return Promise.resolve(null);
    });
  };

  /**
   * [#411] An adoption is an Ariane import, so it ends in the same receipt dialog -
   * the only place this flow says what the conversion dropped. The backend used to
   * discard the whole thing at the call site, so a pulled project lost its excluded
   * shots and every parse anomaly in silence.
   */
  it('shows the import receipt for what the adoption converted', async () => {
    adoptReplying({
      local_path: '/cache/p1.stalx',
      type_flipped: true,
      concurrent_session: false,
      upload: { kind: 'pushed' },
      import_receipt: ADOPT_RECEIPT,
    });

    expect(await hook().adoptRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('import-receipt', {
      receipt: ADOPT_RECEIPT,
    });
  });

  it('shows no receipt for an adoption whose project never opened', async () => {
    // A receipt describes what is on screen. After a failed load nothing is, and the
    // load has already surfaced its own error - a modal over it says the import
    // landed when it did not.
    loadProjectMock.mockResolvedValue({ success: false });
    adoptReplying({
      local_path: '/cache/p1.stalx',
      type_flipped: true,
      concurrent_session: false,
      upload: { kind: 'pushed' },
      import_receipt: ADOPT_RECEIPT,
    });

    expect(await hook().adoptRemote(SUMMARY)).toEqual({ status: 'load_failed' });
    expect(useUIStore.getState().openDialog).not.toHaveBeenCalledWith(
      'import-receipt',
      expect.anything(),
    );
  });

  it('shows no receipt when the backend sent none', async () => {
    // An older backend answers without the field at all; an empty modal is worse
    // than none.
    adoptReplying({
      local_path: '/cache/p1.stalx',
      type_flipped: true,
      concurrent_session: false,
      upload: { kind: 'pushed' },
    });

    expect(await hook().adoptRemote(SUMMARY)).toEqual({ status: 'opened' });
    expect(useUIStore.getState().openDialog).not.toHaveBeenCalledWith(
      'import-receipt',
      expect.anything(),
    );
  });

  /**
   * [#270] A failed load must leave the user exactly where they were - the PREVIOUS
   * project is still on screen and still open backend-side.
   *
   * The rollback used `clearLink`, which sets `projectLinkKind: 'unknown'` - read-only
   * under the fail-closed gate - and left the repair to an unawaited
   * `refreshLinkStatus` that swallows its errors, in the very conditions that break a
   * load. So a LOCAL project the user was editing could go read-only for the rest of
   * the session because an unrelated remote open failed. Driven through the REAL seed
   * and rollback, since the assertion is about the state the user is left in.
   */
  it('restores the previous link when the pulled project fails to load', async () => {
    useSpeleoStore.setState({
      setLinkOnOpen: realSetLinkOnOpen,
      link: null,
      projectLinkKind: 'local',
      linkPath: null,
    });
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);

    loadProjectMock.mockResolvedValue({ success: false });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'load_failed' });

    // Back to 'local', NOT 'unknown' - and still editable, with no network involved.
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(useSpeleoStore.getState().link).toBeNull();
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
  });

  it('reports an open error and flags a re-login on a dead token', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({ message: 'unauthorized', code: 'SPELEODB_RELOGIN', status: 401 });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome.status).toBe('error');
    if (outcome.status === 'error') expect(outcome.reLogin).toBe(true);
  });

  it('raises the keep-local prompt when the open is refused over unpushed edits', async () => {
    // GlobalView's shape: no onBeforeLoad, outcome routed straight to its toast/
    // busy handling. The refusal must NOT come back as `error` (that would toast a
    // dead-end message) - it raises the prompt and reports `unpushed_local`.
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({
          message: 'refused',
          code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
          status: null,
          unpushed_local: {
            project_id: 'p1',
            local_path: '/cache/p1.stalx',
            last_edit_at: '2026-07-20T10:30:00Z',
          },
        });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'unpushed_local' });
    expect(openDialogMock).toHaveBeenCalledWith('speleodb-unpushed-local', {
      summary: SUMMARY,
      unpushedLocal: {
        project_id: 'p1',
        local_path: '/cache/p1.stalx',
        last_edit_at: '2026-07-20T10:30:00Z',
      },
    });
    expect(loadProjectMock).not.toHaveBeenCalled();
  });

  it('does not swallow an unrelated failure into the prompt', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({ message: 'boom', code: 'FAILED', status: 500 });
      return Promise.resolve(null);
    });
    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome.status).toBe('error');
    expect(openDialogMock).not.toHaveBeenCalled();
  });

  it('keepLocalCopy loads the local path without any SpeleoDB command', async () => {
    const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
    expect(outcome).toEqual({ status: 'opened' });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    expect(invokeIpcMock.mock.calls.filter((c) => String(c[0]).startsWith('speleodb_'))).toEqual([]);
  });

  // The core of the #270 keep-local defect: this branch pulls NOTHING and is
  // reachable ONLY because the backend refused the open over un-pushed local edits,
  // so the copy is provably dirty. The shared seed defaults to `false`, and that
  // exact flag is what `guardQuit` and the status button's warning read.
  it('keepLocalCopy seeds the dirty bit TRUE - the refusal is proof the copy is dirty', async () => {
    await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      forPath: '/cache/p1.stalx',
      hasUnpushedEdits: true,
      projectId: 'p1',
    });
  });

  it('keepLocalCopy seeds NOTHING when the refusal names the already-open project', async () => {
    // "Open from SpeleoDB" is in the in-project menu, so re-opening the current
    // project is an ordinary mis-step. The live link is fully resolved (lock held,
    // permission, remote state); the optimistic seed would replace it with a
    // pessimistic guess, and `loadProject` then short-circuits on ALREADY_OPEN so
    // nothing re-resolves it synchronously - dropping the survey UI to read-only for
    // a project we hold the lock on. Skipping the seed is correct offline too.
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).not.toHaveBeenCalled();
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
  });

  it('does not touch the live link when the already-open keep-local load fails', async () => {
    // There was no seed to roll back, and the link on screen is the open project's
    // own resolved state - downgrading it is the very thing the rollback undoes.
    const live = {
      permission: 'READ_AND_WRITE' as const,
      editState: 'editing' as const,
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: true,
      unwound: false,
      forceEditing: false,
      forceEditAllowed: false,
      undoMode: 'full' as const,
      remoteState: 'up_to_date' as const,
      noAccess: false,
      serverError: null,
    };
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    useSpeleoStore.setState({ link: live, projectLinkKind: 'remote', linkPath: '/cache/p1.stalx' });
    loadProjectMock.mockResolvedValue({ success: false });
    const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
    expect(outcome).toEqual({ status: 'load_failed' });
    expect(useSpeleoStore.getState().link).toEqual(live);
    expect(useSpeleoStore.getState().projectLinkKind).toBe('remote');
  });

  it('passes the refusal prompt a route back to the host dialog when given one', async () => {
    const openDialogMock = vi.fn();
    useUIStore.setState({ openDialog: openDialogMock });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.reject({
          message: 'refused',
          code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
          status: null,
          unpushed_local: {
            project_id: 'p1',
            local_path: '/cache/p1.stalx',
            last_edit_at: null,
          },
        });
      return Promise.resolve(null);
    });
    const withReturn = renderHook(() =>
      useOpenRemoteProject({ cancelReturnsTo: 'open-from-speleodb' }),
    ).result.current;
    await withReturn.openRemote(SUMMARY);
    expect(openDialogMock).toHaveBeenCalledWith(
      'speleodb-unpushed-local',
      expect.objectContaining({ cancelReturnsTo: 'open-from-speleodb' }),
    );
  });

  it('takeRemoteDiscardingLocal re-issues the refused open with the same arguments', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project_discarding_local')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });
    const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    const call = invokeIpcMock.mock.calls.find(
      (c) => c[0] === 'speleodb_open_project_discarding_local',
    );
    expect(call?.[1]).toEqual({
      projectId: 'p1',
      projectName: 'Cave A',
      latestCommitSha: 'abc123',
      // [#258] No chosen entry on a first attempt - this route can hit an ambiguous
      // dump too, and sends the pick only on the picker's retry.
      entryPath: null,
    });
    // The shared post-open tail ran, exactly as it does for a plain open.
    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      // The path the seed belongs to, so the `syncProjectLink` inside the load
      // that follows keeps THIS project's link instead of the previous one's.
      forPath: '/cache/p1.stalx',
      // A PULL has, by construction, nothing local the server has not seen.
      hasUnpushedEdits: false,
      // And WHICH project, so `remoteProjectId` names it from the same synchronous
      // `set()` rather than after a status poll - which left every consumer keyed on
      // it reading the PREVIOUS project for the length of the poll interval.
      projectId: 'p1',
    });
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
  });

  // End to end through the REAL store action: the whole point of the dirty-bit seed
  // is that the app's exit guard is armed afterwards. Before this, "Keep the local
  // copy" produced a link claiming nothing outstanding, so menu Quit, Ctrl+Q and the
  // OS window X all walked straight past `SpeleoQuitDialog` and the edits that had
  // just refused an overwrite could be lost on the next quit.
  it('arms the quit guard after keepLocalCopy (real store seed)', async () => {
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen });
    useUIStore.setState({ openDialog: vi.fn() as never, activeDialog: null, dialogData: {} });
    await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    // guardQuit needs a project open to guard.
    useProjectStore.setState({ project: { name: 'Cave A' } as never, isDirty: false });
    expect(guardQuit()).toBe(true);
    expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  /**
   * [#270] A superseded load is NOT a failure to roll back from.
   *
   * `loadProject` answers `success: false` for two different things, and only one
   * of them leaves the user where they were. When a newer open superseded this
   * one, the pre-attempt snapshot describes a project that is no longer on
   * screen - restoring it wrote `projectLinkKind: 'unknown'` (read-only under the
   * fail-closed gate) over the LOCAL project the user had just opened, with the
   * repair left to the unawaited, error-swallowing status refresh. Reachable:
   * GlobalView guards its remote rows with `remoteBusyRef`, but the recents-list
   * open and Publish-to-SpeleoDB have no such guard.
   */
  it('does NOT roll the link back when the load was superseded by a newer open', async () => {
    // What the newer open left behind: a local project, fully editable.
    const settled = () =>
      useSpeleoStore.setState({ link: null, projectLinkKind: 'local', linkPath: null });
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen, projectLinkKind: 'unknown' });
    loadProjectMock.mockImplementation(async () => {
      settled();
      useProjectStore.setState({ project: { file_path: '/other/local.stalx' } as never });
      return { success: false, superseded: true };
    });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });

    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'load_failed' });
    // The project the user actually has open is untouched - and still editable.
    expect(useSpeleoStore.getState().projectLinkKind).toBe('local');
    expect(selectSpeleoReadOnly(useSpeleoStore.getState())).toBe(false);
  });

  /**
   * [#270] `keepLocalCopy` skipped the seed for the already-open project; the two
   * other routes into the same tail did not. "Open from SpeleoDB" is in the
   * IN-PROJECT menu, so `openRemote` re-opening the current project is an ordinary
   * mis-step - and for a CLEAN open project the backend does not refuse it, so it
   * sails past the refusal prompt straight into the seed, which writes
   * `read-only` / nothing-unpushed / no-force-edit over a lock we hold.
   */
  it('openRemote seeds NOTHING when the pull names the already-open project', async () => {
    useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_open_project')
        return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
      return Promise.resolve(null);
    });

    const outcome = await hook().openRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    expect(setLinkOnOpenMock).not.toHaveBeenCalled();
    expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
  });

  /**
   * [#270] "Open from SpeleoDB" is a way OUT of the open project - `finishOpen` ends
   * in `loadProject`, which replaces it backend-side - and it was the only one that
   * ran no guard whatsoever: not the shared `needsSpeleoHandBackPrompt` the other
   * five exits share, and not even the disk-dirty check the two file-open exits had
   * before that. It went straight to the seed and the load, so picking a DIFFERENT
   * project in the browser discarded unsaved edits with no prompt and walked away
   * from un-pushed work with the server lock still held.
   */
  describe('the entry guard - the open project is never swapped out silently', () => {
    // The pull SUCCEEDS throughout, so an ungarded open runs all the way to the load
    // that replaces the open project - i.e. these cases fail on the behavior they are
    // about, not on a half-mocked call.
    beforeEach(() => {
      invokeIpcMock.mockImplementation((cmd: string) =>
        cmd === 'speleodb_open_project'
          ? Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false })
          : Promise.resolve(null),
      );
    });

    /** Another remote project is open, its first status poll settled. */
    const anotherOpen = (link: SpeleoLinkState, isDirty = false) => {
      useProjectStore.setState({ project: { file_path: '/cache/p9.stalx' } as never, isDirty });
      useSpeleoStore.setState({ link, remoteProjectId: 'p9' });
    };

    /** The refusal message, or '' when the open went ahead. */
    const refusal = async (): Promise<string> => {
      const outcome = await hook().openRemote(SUMMARY);
      return outcome.status === 'error' ? outcome.message : '';
    };

    it('refuses over work SpeleoDB has never seen, before it fetches anything', async () => {
      anotherOpen(openLink({ hasUnpushedEdits: true }));
      const outcome = await hook().openRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      // Not a dead token - there is nothing to re-login for.
      if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
      // Nothing pulled, nothing seeded, nothing loaded: the guard has to run BEFORE
      // the pull (the seed overwrites the very link it reads) and before the load
      // (which is what closes the open project backend-side).
      expect(invokeIpcMock).not.toHaveBeenCalled();
      expect(setLinkOnOpenMock).not.toHaveBeenCalled();
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('refuses while we still hold the write lock', async () => {
      // Nothing unpushed, but the app keeps running, so nothing else would ever
      // release it - the second half of `needsSpeleoHandBackPrompt`.
      anotherOpen(openLink({ editState: 'editing' }));
      expect(await refusal()).not.toBe('');
      expect(invokeIpcMock).not.toHaveBeenCalled();
    });

    it('refuses over unsaved-to-disk changes in a plain local project', async () => {
      useProjectStore.setState({ project: { file_path: '/cave/local.stalx' } as never, isDirty: true });
      expect(await refusal()).not.toBe('');
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('names the SpeleoDB side when the project is dirty in both senses', async () => {
      anotherOpen(openLink({ hasUnpushedEdits: true }), true);
      const both = await refusal();
      anotherOpen(openLink({ hasUnpushedEdits: true }));
      const speleoOnly = await refusal();
      useProjectStore.setState({ project: { file_path: '/cave/local.stalx' } as never, isDirty: true });
      useSpeleoStore.setState({ link: null, remoteProjectId: null });
      const diskOnly = await refusal();
      // Close resolves both (its prompt carries a "save changes locally" checkbox),
      // so pointing at it settles a doubly-dirty project in one hop, not two.
      expect(both).toBe(speleoOnly);
      expect(both).not.toBe(diskOnly);
    });

    it('lets a clean open project be swapped out', async () => {
      anotherOpen(openLink());
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    });

    it('re-opening the project already on screen is exempt from the hand-back check', async () => {
      // Nothing is handed back to a project we are re-opening, and this is the route
      // "take the server copy and discard my local changes" reaches the hook by, over
      // a cache copy that is un-pushed BY DEFINITION. Refusing on un-pushed work here
      // would close the only door out of that state.
      useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never, isDirty: false });
      useSpeleoStore.setState({
        link: openLink({ hasUnpushedEdits: true, editState: 'editing' }),
        remoteProjectId: 'p1',
      });
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
    });

    it('still refuses re-opening the open project over unsaved-to-disk edits', async () => {
      // The exemption above covers the HAND-BACK question only. `speleodb_open_project`
      // replaces the cache `.stalx`, and `replace_cache_stalx_abandoning` closes the
      // live backend project without saving before the rename - so unsaved in-memory
      // survey is dropped. The persisted dirty bit usually makes the backend refuse
      // first, but writing it is best-effort (`mark_remote_dirty` swallows a failure)
      // and a frontend-only edit can leave `isDirty` as the only evidence.
      useProjectStore.setState({ project: { file_path: '/cache/p1.stalx' } as never, isDirty: true });
      useSpeleoStore.setState({ link: openLink(), remoteProjectId: 'p1' });

      const outcome = await hook().openRemote(SUMMARY);

      expect(outcome.status).toBe('error');
      expect(await refusal()).toMatch(/sav/i);
      // Refused BEFORE the pull, so the cache file it would have replaced is untouched.
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_open_project', expect.anything());
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    /**
     * [#270] The refused-open prompt's two branches are project-replacing exits of
     * their own, and they ran no guard - only whatever the `openRemote` that raised
     * the prompt had concluded, which by then is stale by an unbounded amount.
     * `SpeleoUnpushedLocalDialog` sits on screen for as long as the user takes to
     * decide, while `SpeleoStatusButton`'s interval keeps calling `refreshLinkStatus`
     * for the project underneath (30 s to a user-configurable 24 h) and its
     * `seedFromStatus` rewrites `link` wholesale - `hasUnpushedEdits`, `editState`,
     * `lockHolder`. So the project on screen can acquire un-pushed work, or have its
     * lock resolved, between the check and the load that closes it backend-side.
     *
     * The answer the user gave is no waiver: it is about the TARGET's cached copy,
     * not about the work in the project being swapped out. (This replaces a case that
     * asserted the opposite - "does not re-ask on the branches reached THROUGH a
     * guarded open" - which pinned the swap-away of an un-pushed, disk-dirty p9 as
     * correct. The half of its intent that IS correct is kept below: the same-project
     * routes must stay open.)
     */
    it('refuses keep-local when the open project goes unpushed while the prompt is up', async () => {
      // The real sequence rather than a synthetic one: the open IS guarded and the
      // project on screen is clean, so the backend's un-pushed refusal raises the
      // prompt; the status poll then lands and marks that project un-pushed while the
      // user is still reading it.
      invokeIpcMock.mockImplementation((cmd: string) =>
        cmd === 'speleodb_open_project'
          ? Promise.reject({
              message: 'refused',
              code: 'SPELEODB_UNPUSHED_LOCAL_EDITS',
              status: null,
              unpushed_local: {
                project_id: 'p1',
                local_path: '/cache/p1.stalx',
                last_edit_at: null,
              },
            })
          : Promise.resolve(null),
      );
      anotherOpen(openLink());
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'unpushed_local' });

      useSpeleoStore.setState({ link: openLink({ hasUnpushedEdits: true }) });

      const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
      expect(outcome.status).toBe('error');
      // Not a dead token - there is nothing to re-login for.
      if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
      // The load is what closes p9 backend-side, so it must not have run.
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('refuses keep-local over unsaved-to-disk changes in the project being swapped out', async () => {
      // Keeping the TARGET's local copy still ends in a `loadProject` for a different
      // file, which replaces the open project and drops its unsaved in-memory survey.
      useProjectStore.setState({
        project: { file_path: '/cave/local.stalx' } as never,
        isDirty: true,
      });
      const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');
      expect(outcome.status).toBe('error');
      if (outcome.status === 'error') expect(outcome.message).toMatch(/sav/i);
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('keep-local still opens the path already on screen - that call replaces nothing', async () => {
      // Do-not-regress (passes before the guard was added too). This branch issues no
      // SpeleoDB command, and `loadProject` short-circuits on ALREADY_OPEN for its own
      // path - it flips the view and re-resolves the link, never re-reading the file -
      // so neither half of the guard has anything to protect, and a disk-dirty refusal
      // would raise "save your changes first" over an action that cannot lose them.
      // Driven with `remoteProjectId` deliberately UNSET: the skip is decided by path,
      // so it holds in the window before the first status poll records the open
      // project's id, where the guard's own id-based exemption does not.
      useProjectStore.setState({
        project: { file_path: '/cache/p1.stalx' } as never,
        isDirty: true,
      });
      useSpeleoStore.setState({
        link: openLink({ hasUnpushedEdits: true, editState: 'editing' }),
        remoteProjectId: null,
      });
      expect(await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx')).toEqual({ status: 'opened' });
      expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    });

    /** The discarding re-open SUCCEEDS, so an unguarded branch runs all the way to the load. */
    const takeRemoteSucceeds = () => {
      invokeIpcMock.mockImplementation((cmd: string) =>
        cmd === 'speleodb_open_project_discarding_local'
          ? Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false })
          : Promise.resolve(null),
      );
    };

    it('refuses the take-remote when the open project goes unpushed while the prompt is up', async () => {
      takeRemoteSucceeds();
      anotherOpen(openLink({ hasUnpushedEdits: true }));
      const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
      expect(outcome.status).toBe('error');
      if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
      // Nothing pulled, nothing seeded, nothing loaded.
      expect(invokeIpcMock).not.toHaveBeenCalledWith(
        'speleodb_open_project_discarding_local',
        expect.anything(),
      );
      expect(setLinkOnOpenMock).not.toHaveBeenCalled();
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('refuses the take-remote while we still hold the write lock on the open project', async () => {
      takeRemoteSucceeds();
      anotherOpen(openLink({ editState: 'editing' }));
      const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
      expect(outcome.status).toBe('error');
      expect(invokeIpcMock).not.toHaveBeenCalledWith(
        'speleodb_open_project_discarding_local',
        expect.anything(),
      );
    });

    it('still takes the server copy for the already-open project over its OWN un-pushed cache copy', async () => {
      // Do-not-regress (passes before the guard was added too), and the load-bearing
      // half of the case this block replaced: the hand-back exemption is the door this
      // branch exists to open, over a cache copy that is un-pushed BY DEFINITION - that
      // is why the prompt was up. Refusing here would shut the only way out of it.
      takeRemoteSucceeds();
      useProjectStore.setState({
        project: { file_path: '/cache/p1.stalx' } as never,
        isDirty: false,
      });
      useSpeleoStore.setState({
        link: openLink({ hasUnpushedEdits: true, editState: 'editing' }),
        remoteProjectId: 'p1',
      });
      expect(await hook().takeRemoteDiscardingLocal(SUMMARY)).toEqual({ status: 'opened' });
      expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    });

    it('still refuses the take-remote for the open project over unsaved-to-disk edits', async () => {
      // Disk-dirty is NOT part of that exemption here either:
      // `speleodb_open_project_discarding_local` routes through
      // `replace_cache_stalx_abandoning`, which closes the live project WITHOUT saving
      // and renames the fresh dump over its `.stalx`. Nothing is stranded by refusing -
      // the user saves and presses it again.
      takeRemoteSucceeds();
      useProjectStore.setState({
        project: { file_path: '/cache/p1.stalx' } as never,
        isDirty: true,
      });
      useSpeleoStore.setState({ link: openLink(), remoteProjectId: 'p1' });
      const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
      expect(outcome.status).toBe('error');
      if (outcome.status === 'error') expect(outcome.message).toMatch(/sav/i);
      expect(invokeIpcMock).not.toHaveBeenCalledWith(
        'speleodb_open_project_discarding_local',
        expect.anything(),
      );
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    /**
     * [#270] The adopt route is the OTHER command that replaces the open project, and
     * it ran no guard of its own - only whatever `openRemote` had concluded, which by
     * then is stale by an unbounded amount. `openRemote` hands `adopt` back to the
     * HOST, which parks the summary in its own state and puts up a confirm the user
     * may sit on indefinitely, while the status poll (30 s to a user-configurable
     * 24 h) keeps writing to the very link the guard reads. `adoptRemote` is also an
     * exported entry point three components call from their own state, so its safety
     * rested on a call-site convention rather than on anything in the hook.
     *
     * Driven at the hook boundary, the way the entry-guard cases above drive
     * `openRemote`, with the adopt SUCCEEDING - so an unguarded adopt runs all the
     * way to the load that replaces the open project.
     */
    const arianeThenAdopts = () => {
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'speleodb_open_project')
          return Promise.reject({ message: 'ariane', code: 'SPELEODB_ADOPT', status: null });
        if (cmd === 'speleodb_adopt_ariane')
          return Promise.resolve({
            local_path: '/cache/p1.stalx',
            type_flipped: true,
            concurrent_session: false,
          });
        return Promise.resolve(null);
      });
    };

    it('adoptRemote refuses over work SpeleoDB has never seen, before it adopts anything', async () => {
      arianeThenAdopts();
      anotherOpen(openLink({ hasUnpushedEdits: true }));
      const outcome = await hook().adoptRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      // Not a dead token - there is nothing to re-login for.
      if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
      // Nothing adopted, nothing seeded, nothing loaded: `speleodb_adopt_ariane`
      // replaces the cache `.stalx` too, so the guard has to run before it.
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_adopt_ariane', expect.anything());
      expect(setLinkOnOpenMock).not.toHaveBeenCalled();
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('adoptRemote refuses while we still hold the write lock', async () => {
      arianeThenAdopts();
      anotherOpen(openLink({ editState: 'editing' }));
      const outcome = await hook().adoptRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_adopt_ariane', expect.anything());
    });

    it('adoptRemote refuses over unsaved-to-disk changes in a plain local project', async () => {
      arianeThenAdopts();
      useProjectStore.setState({ project: { file_path: '/cave/local.stalx' } as never, isDirty: true });
      const outcome = await hook().adoptRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('refuses the adopt when the open project goes unpushed while the confirm is up', async () => {
      // The real sequence rather than a synthetic one: the open IS guarded and the
      // project on screen is clean, so it routes to the host's adopt confirm; the
      // status poll then lands and marks that project un-pushed while the user is
      // still reading the confirm. The stale "clean" answer must not carry.
      arianeThenAdopts();
      anotherOpen(openLink());
      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'adopt' });

      useSpeleoStore.setState({ link: openLink({ hasUnpushedEdits: true }) });

      const outcome = await hook().adoptRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_adopt_ariane', expect.anything());
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    it('still adopts the already-open project over its OWN un-pushed cache copy', async () => {
      // The exemption is load-bearing on the adopt route too: the take-remote branch
      // routes to an adopt for a project that is un-pushed BY DEFINITION (that is why
      // the refused-open prompt was up), and it is the only way out of that state.
      // Nothing is handed back to a project we are re-adopting, so the hand-back half
      // must stay exempt - and the discard the user just confirmed is no waiver for
      // it, because it answers about the TARGET, not about the project on screen.
      arianeThenAdopts();
      useProjectStore.setState({
        project: { file_path: '/cache/p1.stalx' } as never,
        isDirty: false,
      });
      useSpeleoStore.setState({
        link: openLink({ hasUnpushedEdits: true, editState: 'editing' }),
        remoteProjectId: 'p1',
      });

      expect(await hook().adoptRemote(SUMMARY, { discardLocal: true })).toEqual({
        status: 'opened',
      });
      expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
    });

    it('still refuses re-adopting the open project over unsaved-to-disk edits', async () => {
      // Disk-dirty is NOT part of that exemption: `speleodb_adopt_ariane` replaces the
      // cache `.stalx` exactly as the open command does, dropping unsaved in-memory
      // survey with it.
      arianeThenAdopts();
      useProjectStore.setState({
        project: { file_path: '/cache/p1.stalx' } as never,
        isDirty: true,
      });
      useSpeleoStore.setState({ link: openLink(), remoteProjectId: 'p1' });

      const outcome = await hook().adoptRemote(SUMMARY);
      expect(outcome.status).toBe('error');
      if (outcome.status === 'error') expect(outcome.message).toMatch(/sav/i);
      expect(invokeIpcMock).not.toHaveBeenCalledWith('speleodb_adopt_ariane', expect.anything());
      expect(loadProjectMock).not.toHaveBeenCalled();
    });

    /**
     * [#270] The entry check runs BEFORE the command, which is where it has to run -
     * it is the only place that can stop it. But the pull verbs' await is the
     * DOWNLOAD, and unlike the two prompt windows this branch already closed, that is
     * not a user sitting on a dialog: it is the network, on EVERY open.
     * `download_dump` gets 120 s per attempt (`DUMP_DOWNLOAD_TIMEOUT`, idempotent so
     * retried) against a 640 MB body ceiling, then the pull fetches the revision
     * history and extracts. The poll's floor is 30 s, and `SpeleoStatusButton` stays
     * MOUNTED throughout - `App` renders dialogs as siblings of `SingleSystemView`,
     * so raising the browser does not unmount the toolbar - so `refreshLinkStatus`
     * keeps landing and `seedFromStatus` rewrites `link` wholesale.
     *
     * `finishOpen` cannot be left to catch it: its first act is the optimistic seed,
     * which overwrites the very link the poll just wrote, so by the time
     * `loadProject` closes the open project the newly-discovered work is hidden and
     * the newly-discovered lock is silently released.
     */
    describe('the link changes while the pull is in flight', () => {
      /**
       * A pull that lands, with `during` standing in for the poll rewriting the store
       * WHILE the command is in flight - run synchronously inside the mock, before it
       * resolves, so the hook resumes on a link the entry check never saw.
       */
      const pullDuringWhich = (during: () => void, cmd = 'speleodb_open_project') => {
        invokeIpcMock.mockImplementation((c: string) => {
          if (c !== cmd) return Promise.resolve(null);
          during();
          return Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false });
        });
      };

      it('refuses when the open project goes unpushed during the download', async () => {
        anotherOpen(openLink());
        pullDuringWhich(() =>
          useSpeleoStore.setState({ link: openLink({ hasUnpushedEdits: true }) }),
        );

        const outcome = await hook().openRemote(SUMMARY);

        expect(outcome.status).toBe('error');
        // Not a dead token - there is nothing to re-login for.
        if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
        // The entry check DID pass, so the pull ran - this is the late half. What
        // must not run is the seed (which overwrites the link the poll just wrote)
        // and the load (which closes p9 backend-side).
        expect(invokeIpcMock).toHaveBeenCalledWith('speleodb_open_project', expect.anything());
        expect(setLinkOnOpenMock).not.toHaveBeenCalled();
        expect(loadProjectMock).not.toHaveBeenCalled();
      });

      it('refuses when the write lock on the open project resolves during the download', async () => {
        // The other disjunct of `needsSpeleoHandBackPrompt`: nothing unpushed, but we
        // now hold the lock, and the app keeps running so nothing else releases it.
        anotherOpen(openLink());
        pullDuringWhich(() => useSpeleoStore.setState({ link: openLink({ editState: 'editing' }) }));

        const outcome = await hook().openRemote(SUMMARY);

        expect(outcome.status).toBe('error');
        expect(setLinkOnOpenMock).not.toHaveBeenCalled();
        expect(loadProjectMock).not.toHaveBeenCalled();
      });

      it('still opens when nothing changed during the download', async () => {
        // Do-not-regress: the late re-check refuses on a genuine CHANGE only. A clean
        // open project that stays clean must still be swapped out, or every remote
        // open pays for the guard.
        anotherOpen(openLink());
        pullDuringWhich(() => {});

        expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
        expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
      });

      it('still opens when the pull names the project already on screen', async () => {
        // Do-not-regress, and the reason the late re-check is skipped by PATH rather
        // than run unconditionally. `replace_cache_stalx_abandoning` calls
        // `close_project_if_open_at(dest)` before the rename, so this pull has ALREADY
        // closed the open project backend-side; refusing now would leave the shell
        // rendering a project the backend does not have open, where auto-save and
        // every later command fail with "no project open". Nothing is protected by
        // refusing either - `finishOpen` skips the seed for this path, so the poll's
        // update survives, and the load re-resolves the same project's link.
        //
        // Driven through the half of the guard that is NOT exempt for the same
        // project - disk-dirty - so the path skip is the only thing that can let it
        // through.
        useProjectStore.setState({
          project: { file_path: '/cache/p1.stalx' } as never,
          isDirty: false,
        });
        useSpeleoStore.setState({ link: openLink(), remoteProjectId: 'p1' });
        pullDuringWhich(() => useProjectStore.setState({ isDirty: true }));

        expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
        expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
      });

      it('refuses the take-remote when the open project goes unpushed during ITS download', async () => {
        // Same command family, same window: `speleodb_open_project_discarding_local`
        // waives the guard for the TARGET's cache copy, which says nothing about the
        // work in the project being swapped out.
        anotherOpen(openLink());
        pullDuringWhich(
          () => useSpeleoStore.setState({ link: openLink({ hasUnpushedEdits: true }) }),
          'speleodb_open_project_discarding_local',
        );

        const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);

        expect(outcome.status).toBe('error');
        if (outcome.status === 'error') expect(outcome.reLogin).toBe(false);
        expect(setLinkOnOpenMock).not.toHaveBeenCalled();
        expect(loadProjectMock).not.toHaveBeenCalled();
      });

      it('does NOT refuse the adopt when the link changes during it', async () => {
        // DO-NOT-REGRESS on a deliberate asymmetry, so nobody "fixes the drift" by
        // adding the late re-check to the adopt as well. The window is identical, but
        // `speleodb_adopt_ariane` rebuilds its destination through
        // `adopt_import_and_save` -> `do_create_project_full`, which commits the new
        // project into `AppState` UNCONDITIONALLY - for every target, not only a
        // re-adopt of the open one. The swap has therefore already happened by the
        // time this resolves, and refusing would not undo it: it would leave the shell
        // rendering the old project while every edit and every auto-save landed on the
        // adopted one. Going through with the load is the lesser failure. Closing this
        // window has to start in `speleodb_adopt_ariane`.
        anotherOpen(openLink());
        invokeIpcMock.mockImplementation((c: string) => {
          if (c !== 'speleodb_adopt_ariane') return Promise.resolve(null);
          useSpeleoStore.setState({ link: openLink({ hasUnpushedEdits: true }) });
          return Promise.resolve({
            local_path: '/cache/p1.stalx',
            type_flipped: true,
            concurrent_session: false,
          });
        });

        expect(await hook().adoptRemote(SUMMARY)).toEqual({ status: 'opened' });
        expect(loadProjectMock).toHaveBeenCalledWith('/cache/p1.stalx');
      });
    });
  });

  /**
   * DO-NOT-REGRESS. Both "is this the project already on screen?" tests in the hook
   * (`finishOpen`'s `alreadyOpen`, `keepLocalCopy`'s skip) compare paths as LITERAL
   * strings, and that is the point rather than an oversight: they predict whether
   * `loadProject` will short-circuit, and the backend decides that with the same
   * literal test (`do_load_project_full`: `project.file_path.as_ref() ==
   * Some(&path)`). An alias the backend cannot see through must therefore take the
   * NOT-already-open branch on this side too - the reload really does happen, so the
   * seed really is needed and the guard really does have something to protect.
   *
   * These cases fail the moment someone "fixes" either site with canonicalization or
   * a narrower normalization (collapsing `.`/`..`, case-folding): the first would
   * skip a seed the reload needs, the second would skip a guard that is standing in
   * front of unsaved in-memory survey. The webview has no filesystem to canonicalize
   * with in any case; alias tolerance would have to start in `do_load_project_full`
   * (`project_io::same_file`), with this side following.
   */
  describe('path spellings the backend would not match either', () => {
    it('openRemote still seeds when the open project is spelled differently', async () => {
      // Same file, dot-segment spelling. `do_load_project_full` compares literally,
      // so it reloads - and a reload with no seed resolves through `syncProjectLink`
      // to read-only / nothing-unpushed, dropping the permission on the floor.
      useProjectStore.setState({ project: { file_path: '/cache/./p1.stalx' } as never });
      invokeIpcMock.mockImplementation((cmd: string) =>
        cmd === 'speleodb_open_project'
          ? Promise.resolve({ local_path: '/cache/p1.stalx', concurrent_session: false })
          : Promise.resolve(null),
      );

      expect(await hook().openRemote(SUMMARY)).toEqual({ status: 'opened' });
      expect(setLinkOnOpenMock).toHaveBeenCalledWith({
        permission: 'READ_AND_WRITE',
        forPath: '/cache/p1.stalx',
        hasUnpushedEdits: false,
        projectId: 'p1',
      });
    });

    it('keepLocalCopy still guards when the open project is spelled differently', async () => {
      // Case alias - the same file on Windows/macOS, and NOT what the backend's
      // literal check matches. So this ends in a full reload that drops the unsaved
      // in-memory survey, which is precisely what the disk-dirty refusal is for.
      useProjectStore.setState({
        project: { file_path: '/cache/P1.stalx' } as never,
        isDirty: true,
      });

      const outcome = await hook().keepLocalCopy(SUMMARY, '/cache/p1.stalx');

      expect(outcome.status).toBe('error');
      if (outcome.status === 'error') expect(outcome.message).toMatch(/sav/i);
      expect(loadProjectMock).not.toHaveBeenCalled();
    });
  });

  it('adoptRemote adopts, loads, and reports opened', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: true,
          concurrent_session: false,
        });
      return Promise.resolve(null);
    });
    const outcome = await hook().adoptRemote(SUMMARY);
    expect(outcome).toEqual({ status: 'opened' });
    const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_adopt_ariane');
    expect(call?.[1]).toEqual({
      projectId: 'p1',
      projectName: 'Cave A',
      latestCommitSha: 'abc123',
      // The ordinary open->adopt route has confirmed no discard, so the backend's
      // un-pushed-edits guard stays armed for it.
      discardLocal: false,
      // [#258] Absent on a first attempt - that is what lets the backend classify,
      // and re-ask, normally. Only a replay from the picker carries a pick.
      entryPath: null,
    });
  });

  /**
   * [#270] An adopt is not a plain pull: the backend stamps the adopted `.stalx`
   * `has_unpushed_edits: true` (it has just converted an Ariane .tml into a
   * project the server has never been given). The shared seed defaults to the
   * pull's answer - nothing outstanding - which is the exact flag `guardQuit` and
   * `useProject`'s close guard read, so BOTH exit guards stayed disarmed over work
   * that exists only locally. The only repair was the unawaited, error-swallowing
   * status refresh: never, offline.
   */
  it('adoptRemote seeds the dirty bit TRUE when the hand-over did not push', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: false,
          concurrent_session: false,
          upload: { kind: 'lock_held', email: 'someone@else.example' },
        });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY);

    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      forPath: '/cache/p1.stalx',
      hasUnpushedEdits: true,
      projectId: 'p1',
    });
  });

  /**
   * The other half of the rule above, and the reason it is keyed on the outcome
   * rather than on "this was an adopt": adoption now hands the conversion back to
   * the server itself, and a landed hand-over clears the backend's dirty bit as part
   * of the push. Seeding TRUE over that arms `guardQuit` and the close guard for a
   * project with nothing outstanding - a "you have unsaved work" prompt on the way
   * out of a project whose every byte is on the server.
   */
  /**
   * The dirty seed asks ONE question - is the survey on the server - and a landed
   * upload whose lock release failed answers it yes. Folding that outcome in with
   * "never uploaded" marked a project dirty whose every byte was already safe, and
   * told the user it was "on this machine only": the most alarming way to be wrong
   * about where someone's only copy is.
   */
  it('adoptRemote seeds the dirty bit FALSE when the upload landed but the lock did not', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: true,
          concurrent_session: false,
          upload: { kind: 'pushed_lock_held' },
        });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY);

    expect(setLinkOnOpenMock).toHaveBeenCalledWith(
      expect.objectContaining({ hasUnpushedEdits: false }),
    );
  });

  it('adoptRemote seeds the dirty bit FALSE once the hand-over pushed', async () => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: true,
          concurrent_session: false,
          upload: { kind: 'pushed' },
        });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY);

    expect(setLinkOnOpenMock).toHaveBeenCalledWith({
      permission: 'READ_AND_WRITE',
      forPath: '/cache/p1.stalx',
      hasUnpushedEdits: false,
      projectId: 'p1',
    });
  });

  /**
   * The toast's COLOUR is the only part of the outcome most users will read, so it has
   * to track the outcome rather than the fact that a command returned. The neutral
   * default said "done" over an adoption whose conversion never left this machine -
   * the misleading direction, since that work is sitting in a cache directory nobody
   * has a reason to look in.
   *
   * Green is reserved for the arm with nothing left to do. A push that landed under a
   * project still typed `ariane` is amber with the local-only case, not green with the
   * finished one: the `.stalx` is up there, but the project still advertises itself as
   * an Ariane project, so it keeps offering Adopt and later `.tml` edits still look
   * authoritative.
   */
  it.each([
    ['pushed and re-typed', { kind: 'pushed' }, true, 'success', 'uploaded to SpeleoDB'],
    // NOT keyed on "you still hold it": both lock-branch strings now contain it
    // ("release the lock IF you still hold it"), because after the holder re-check
    // this outcome also covers "the release may have landed but we could not
    // establish whose the mutex is". The tail is what separates the two.
    [
      'pushed but the lock did not come back',
      { kind: 'pushed_lock_held' },
      true,
      'warning',
      'so others can edit',
    ],
    [
      'pushed but still typed ariane',
      { kind: 'pushed' },
      false,
      'warning',
      'type could not be changed',
    ],
    // Both halves unfinished, from ONE cause: connectivity dropping after the upload
    // lands fails the release AND the type call that follows it. The ladder used to
    // stop at the lock-held rung and never read `type_flipped`, so this row was told
    // about the lock alone - and after releasing it the project was still typed
    // `ariane`, still offering Adopt over a `.stalx` that already exists. No later
    // poll corrects that one; the toast is the only place it is ever said.
    [
      'pushed but neither the lock nor the type came back',
      { kind: 'pushed_lock_held' },
      false,
      'warning',
      'two things are unfinished',
    ],
    [
      'never uploaded',
      { kind: 'lock_held', email: null },
      false,
      'warning',
      'on this machine only',
    ],
  ])(
    'adoptRemote toasts %s as %s',
    async (_label, upload, typeFlipped, expectedKind, expectedPhrase) => {
    const showToast = vi.fn();
    useUIStore.setState({ showToast });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: typeFlipped,
          concurrent_session: false,
          upload,
        });
      return Promise.resolve(null);
    });

      await hook().adoptRemote(SUMMARY);

      // The MESSAGE is pinned too, not just the colour. Two of these three rows are
      // amber, so asserting the colour alone let the two amber messages swap places
      // and stay green - and "uploaded, but the type could not be changed" shown for
      // an adoption that never reached the server is a materially wrong thing to tell
      // someone about where their work is.
      expect(showToast).toHaveBeenCalledWith(
        expect.stringContaining(expectedPhrase),
        'speleodb-adopted',
        expectedKind,
      );
    },
  );

  /**
   * `pushed` is read from a shape that can grow arms, and only ONE of them means the
   * bytes are on the server. An outcome this build cannot read - an older backend, a
   * variant added later - must therefore fall to the protective answer, because the
   * two ways of being wrong do not cost the same: a false "pushed" disarms both exit
   * guards over a conversion that may exist only in this cache, while a false
   * "unpushed" costs one redundant prompt until the next status poll.
   */
  it.each([
    ['an outcome this build does not know', { kind: 'invented_later' }],
    ['no outcome at all', undefined],
  ])('adoptRemote treats %s as NOT pushed', async (_label, upload) => {
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: true,
          concurrent_session: false,
          upload,
        });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY);

    expect(setLinkOnOpenMock).toHaveBeenCalledWith(
      expect.objectContaining({ hasUnpushedEdits: true }),
    );
  });

  // End to end through the REAL seed, since the point is the state the exit guards
  // are left reading (mirrors the keepLocalCopy case above).
  it('arms the quit guard after adoptRemote (real store seed)', async () => {
    useSpeleoStore.setState({ setLinkOnOpen: realSetLinkOnOpen });
    useUIStore.setState({ openDialog: vi.fn() as never, activeDialog: null, dialogData: {} });
    invokeIpcMock.mockImplementation((cmd: string) => {
      if (cmd === 'speleodb_adopt_ariane')
        return Promise.resolve({
          local_path: '/cache/p1.stalx',
          type_flipped: false,
          concurrent_session: false,
        });
      return Promise.resolve(null);
    });

    await hook().adoptRemote(SUMMARY);

    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(true);
    useProjectStore.setState({ project: { name: 'Cave A' } as never, isDirty: false });
    expect(guardQuit()).toBe(true);
    expect(useUIStore.getState().openDialog).toHaveBeenCalledWith('speleodb-quit');
  });

  /**
   * [#214/#270] The take-remote branch had to grow the Ariane route `openRemote`
   * already had, and to carry the discard the user just confirmed into the adopt.
   *
   * Both halves are needed and neither is hypothetical: an adopt stamps the cache
   * copy un-pushed and the server `type` flip is best-effort, so an adopt without
   * write access leaves a project that is BOTH dirty locally (raising the refused-
   * open prompt) and still classified Ariane (so taking the server copy lands on
   * AdoptAriane). Without the route the user hit a flat "this project is an Ariane
   * project" error with nothing opened; with the route but no waiver, the adopt's
   * own guard then refused the discard they had already confirmed.
   */
  describe('takeRemoteDiscardingLocal on a still-Ariane project', () => {
    const arianeRefusal = () => {
      invokeIpcMock.mockImplementation((cmd: string) => {
        if (cmd === 'speleodb_open_project_discarding_local')
          return Promise.reject({ message: 'ariane', code: 'SPELEODB_ADOPT' });
        if (cmd === 'speleodb_adopt_ariane')
          return Promise.resolve({
            local_path: '/cache/p1.stalx',
            type_flipped: false,
            concurrent_session: false,
          });
        return Promise.resolve(null);
      });
    };

    it('routes to adoption instead of dead-ending on the raw error', async () => {
      arianeRefusal();
      const outcome = await hook().takeRemoteDiscardingLocal(SUMMARY);
      expect(outcome).toEqual({ status: 'adopt', discardLocal: true });
    });

    it('and that adoption waives the guard the user already answered', async () => {
      arianeRefusal();
      await hook().adoptRemote(SUMMARY, { discardLocal: true });
      const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'speleodb_adopt_ariane');
      expect(call?.[1]).toMatchObject({ discardLocal: true });
    });
  });

  describe('a picker replay that needs a re-login (#258)', () => {
    const CANDIDATES = [
      { entry_path: 'a.stalx', size_bytes: 1, modified: null },
      { entry_path: 'b.stalx', size_bytes: 2, modified: null },
    ];

    const session = () => ({
      loggedIn: true,
      userEmail: 'someone@example.test',
      endpoint: 'https://example.test',
      keychainBacked: true,
      storage: 'keychain' as const,
      keychain: { available: true, declined: false, probed: true } as never,
    });

    /**
     * The picker is a user interaction of unbounded length, so the token can die
     * between the ambiguity that raised it and the pick that replays it. The backend
     * has already dropped its session by then; the replay's `reLogin` is the only
     * thing that tells this side to do the same, and all three replay callbacks used
     * to throw the message and discard it - leaving the frontend claiming to be
     * logged in, with the picker still up offering to retry against nothing.
     */
    /**
     * The replayed verb is chosen by the ambiguity's KIND, not by the verb that
     * raised it (an adoption whose new tree holds `.stalx` replays the OPEN), so each
     * case has to announce the kind that routes back to the verb under test.
     */
    const ambiguousThenDeadToken = (cmd: string, kind: 'stalx' | 'ariane') => {
      invokeIpcMock.mockImplementation((c: string, args?: Record<string, unknown>) => {
        if (c !== cmd) return Promise.resolve(null);
        if (args?.entryPath)
          return Promise.reject({
            message: 'your session has expired',
            code: 'SPELEODB_RELOGIN',
            status: 401,
          });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: CANDIDATES,
          dump_candidate_kind: kind,
        });
      });
    };

    type Case = [
      verb: string,
      cmd: string,
      kind: 'stalx' | 'ariane',
      run: (h: ReturnType<typeof hook>) => Promise<unknown>,
    ];
    const VERBS: Case[] = [
      ['openRemote', 'speleodb_open_project', 'stalx', (h) => h.openRemote(SUMMARY)],
      [
        'takeRemoteDiscardingLocal',
        'speleodb_open_project_discarding_local',
        'stalx',
        (h) => h.takeRemoteDiscardingLocal(SUMMARY),
      ],
      ['adoptRemote', 'speleodb_adopt_ariane', 'ariane', (h) => h.adoptRemote(SUMMARY)],
    ];

    it.each(VERBS)('drops the session when the %s replay reports one', async (_v, cmd, kind, run) => {
      useSpeleoStore.setState({ session: session() });
      const before = useSpeleoStore.getState().sessionGeneration;
      ambiguousThenDeadToken(cmd, kind);

      const outcome = await run(hook());
      expect(outcome).toEqual({ status: 'ambiguous' });
      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(true);

      await useSpeleoStore.getState().resolveDumpPicker('a.stalx');

      const s = useSpeleoStore.getState();
      expect(s.session).toBeNull();
      expect(s.sessionGeneration).toBe(before + 1);
      // `clearSession` is what closes the picker, and it must actually close: a
      // dialog offering to retry against a session that no longer exists is a dead
      // end whichever button is pressed.
      expect(s.dumpPickerOpen).toBe(false);
      // ...and the replay's throw must not paint an error onto the closed dialog,
      // where it would surface on whatever picker opens next.
      expect(s.dumpPickerError).toBeNull();
    });

    it('does not raise a picker for a session that ended while the request was out', async () => {
      // Ambiguity is a routing signal, so the catch that receives it opens a dialog
      // rather than reporting anything - and these verbs make a call of unbounded
      // length, so a 401 auto-drop on any concurrent call can end the session
      // underneath them. Routing anyway put a picker on screen for an account nobody
      // was signed in to, where every button fails.
      useSpeleoStore.setState({ session: session() });
      invokeIpcMock.mockImplementation((c: string) => {
        if (c !== 'speleodb_open_project') return Promise.resolve(null);
        // Something else drops the session while this request is in flight.
        useSpeleoStore.getState().clearSession();
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: CANDIDATES,
          dump_candidate_kind: 'stalx',
        });
      });

      const outcome = await hook().openRemote(SUMMARY);

      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
      expect(outcome).toMatchObject({ status: 'error' });
    });

    it('does not raise a picker once a refresh has reported the session logged out', async () => {
      // `setSession` writes whatever the backend reports and does NOT bump the
      // generation - it is the benign refresh a project load and Settings both
      // trigger - so a reply that comes back `logged_in: false` leaves a session
      // object behind at the same generation. The picker must not open for it.
      useSpeleoStore.setState({ session: session() });
      invokeIpcMock.mockImplementation((c: string) => {
        if (c !== 'speleodb_open_project') return Promise.resolve(null);
        useSpeleoStore.setState({ session: { ...session(), loggedIn: false } });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: CANDIDATES,
          dump_candidate_kind: 'stalx',
        });
      });

      await hook().openRemote(SUMMARY);
      expect(useSpeleoStore.getState().dumpPickerOpen).toBe(false);
    });

    it('still reports an ordinary replay failure without dropping the session', async () => {
      // The other half of the same branch: only `reLogin` ends the session. A replay
      // that fails for any other reason has to leave the picker up, with its reason
      // on it, so the user can simply answer again.
      useSpeleoStore.setState({ session: session() });
      invokeIpcMock.mockImplementation((c: string, args?: Record<string, unknown>) => {
        if (c !== 'speleodb_open_project') return Promise.resolve(null);
        if (args?.entryPath)
          return Promise.reject({ message: 'that file is corrupt', action: null, status: 500 });
        return Promise.reject({
          message: 'more than one',
          code: 'SPELEODB_AMBIGUOUS',
          status: null,
          dump_candidates: CANDIDATES,
        });
      });

      await hook().openRemote(SUMMARY);
      await useSpeleoStore.getState().resolveDumpPicker('a.stalx');

      const s = useSpeleoStore.getState();
      expect(s.session).not.toBeNull();
      expect(s.dumpPickerOpen).toBe(true);
      // The message itself comes from the mocked `getErrorMessage` at the top of this
      // file; what matters here is that a reason was recorded ON the still-open
      // picker rather than the session being torn down under it.
      expect(s.dumpPickerError).not.toBeNull();
    });
  });
});
