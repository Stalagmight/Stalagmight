import { beforeEach, describe, expect, it, vi } from 'vitest';
import { failOnRefreshErrors } from '../../test/refreshGuard';

import { useProjectStore } from '../projectStore';
import { emptyBinaryRefresh, invokeIpcMock } from '../../test/factories/ipc';

vi.mock('../../utils/ipc');
// [#576 NF-W6A-5] The refresh every edit/import ends with reads bytes; a test that
// gives none answers "no project open" rather than making the decoder throw.
emptyBinaryRefresh();

// [#576 NF-W6A-5] Every refresh this file drives must be answered by its mock.
failOnRefreshErrors();

describe('projectStore integrity banner dismiss lifecycle', () => {
  let issues: string[];
  let saveBlocked: boolean;

  beforeEach(() => {
    vi.clearAllMocks();
    issues = [];
    saveBlocked = false;
    invokeIpcMock.mockImplementation(async (command: string) => {
      // check_project_integrity returns IntegrityCheckResult { messages, save_blocked }.
      if (command === 'check_project_integrity') return { messages: issues, save_blocked: saveBlocked };
      return undefined;
    });
    useProjectStore.setState({
      // checkProjectIntegrity now drops its result if no project is loaded (the
      // stale-result guard), and in the real app it only ever runs with a project
      // open - so give these dismissal-lifecycle tests a loaded project by default.
      project: { file_path: '/test.stalx' } as never,
      integrityIssues: [],
      integrityDismissed: false,
      integritySaveBlocked: false,
    });
  });

  it('keeps the banner dismissed across a re-check with the same issue set', async () => {
    // A detect-only issue surfaces on load.
    issues = ['drifted loop closure'];
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityIssues).toEqual(['drifted loop closure']);
    expect(useProjectStore.getState().integrityDismissed).toBe(false);

    // The user dismisses the banner - issues stay populated, only the flag flips.
    useProjectStore.getState().dismissIntegrityIssues();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);
    expect(useProjectStore.getState().integrityIssues).toEqual(['drifted loop closure']);

    // A save (or failed autosave) re-checks with the SAME set: must stay dismissed
    // - previously this resurrected the banner the user just dismissed.
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);
  });

  it('re-shows the banner when a genuinely new issue appears', async () => {
    issues = ['drifted loop closure'];
    await useProjectStore.getState().checkProjectIntegrity();
    useProjectStore.getState().dismissIntegrityIssues();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);

    // A different issue set resets the dismissal so the new problem is seen.
    issues = ['drifted loop closure', 'dangling merge'];
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityDismissed).toBe(false);
  });

  it('does not carry a dismissal across a project switch (identical messages)', async () => {
    // Two different projects can produce byte-identical issue messages (section
    // ids start at 0 in every project), so a dismissal must NOT leak across a
    // switch and suppress the new project's real banner.
    const msg = 'Section 0 has a shot with a non-finite (NaN/inf) value (at index 0)';
    issues = [msg];
    await useProjectStore.getState().checkProjectIntegrity();
    useProjectStore.getState().dismissIntegrityIssues();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);

    // Closing the project must reset the integrity state.
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'close_project') return { success: true };
      if (command === 'check_project_integrity') return { messages: issues, save_blocked: saveBlocked };
      return undefined;
    });
    await useProjectStore.getState().closeProject();
    expect(useProjectStore.getState().integrityIssues).toEqual([]);
    expect(useProjectStore.getState().integrityDismissed).toBe(false);

    // Opening a different project whose check returns the IDENTICAL message must
    // show its banner (dismissal did not leak). A real load installs the new
    // project before its integrity check runs - without a project loaded the check
    // now drops its result.
    useProjectStore.setState({ project: { file_path: '/other.stalx' } as never });
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityIssues).toEqual([msg]);
    expect(useProjectStore.getState().integrityDismissed).toBe(false);
  });

  it('keeps the dismissal across a re-check that only reorders the messages', async () => {
    // The backend's message order isn't stable (HashMap iteration); a reorder of
    // the same set must not resurface a dismissed banner.
    issues = ['orphan offset A', 'orphan comment B'];
    await useProjectStore.getState().checkProjectIntegrity();
    useProjectStore.getState().dismissIntegrityIssues();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);

    // Same set, different order.
    issues = ['orphan comment B', 'orphan offset A'];
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityDismissed).toBe(true);
  });

  it('tracks save-blocked state from the check result', async () => {
    // A save-blocking (unresolvable cross-reference) project sets the flag; the
    // banner uses it to stay non-dismissable (App gates on integritySaveBlocked).
    issues = ['Merge references a deleted station'];
    saveBlocked = true;
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integritySaveBlocked).toBe(true);
    // Even if the user clicks Dismiss, the App banner ignores it while blocked;
    // here we just assert the flag the App reads is set.
    useProjectStore.getState().dismissIntegrityIssues();
    expect(useProjectStore.getState().integritySaveBlocked).toBe(true);

    // Once repaired (issues gone, not blocked), the flag clears.
    issues = [];
    saveBlocked = false;
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integritySaveBlocked).toBe(false);
  });

  it('re-checks integrity after undo so the banner is not stale', async () => {
    // Start clean (e.g. just after a Repair).
    issues = [];
    saveBlocked = false;
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityIssues).toEqual([]);

    // Undoing the Repair restores the dangling ref: the next integrity check
    // reports it, and undo must re-run that check (otherwise the banner stays
    // stale showing "clean" while the project is broken).
    issues = ['Merge anchor no longer resolves in section 0'];
    saveBlocked = true;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'undo') return { success: true };
      if (command === 'get_project_state_binary') return new ArrayBuffer(0); // refreshProjectState
      if (command === 'check_project_integrity') return { messages: issues, save_blocked: saveBlocked };
      return undefined;
    });

    await useProjectStore.getState().undo();

    expect(useProjectStore.getState().integrityIssues).toEqual([
      'Merge anchor no longer resolves in section 0',
    ]);
    expect(useProjectStore.getState().integritySaveBlocked).toBe(true);
  });

  it('marks the project dirty after a repair that actually changed it', async () => {
    // A repair that changed the project leaves unsaved edits: the store must go
    // dirty so auto-save runs and the close prompt fires. Otherwise a project that
    // was clean (isDirty false) when the user clicked Repair still looks "saved"
    // and the fix is silently lost on close/reopen.
    useProjectStore.setState({ isDirty: false });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') return { fixed: 2, remaining: 0, changed: true };
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined; // refreshProjectState / refreshUndoState are best-effort here
    });

    const result = await useProjectStore.getState().repairProjectIntegrity();

    expect(result).toEqual({ fixed: 2, remaining: 0, changed: true });
    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('dirties on a silent sub-tolerance repair (changed but fixed === 0)', async () => {
    // The repair always re-solves offsets, healing sub-reporting-tolerance drift
    // that was never counted as a violation - so fixed can be 0 while the project
    // genuinely changed. Keying on `changed` (not `fixed`) still dirties here, so
    // the silent fix is auto-saved / prompted on close instead of being lost.
    useProjectStore.setState({ isDirty: false });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') return { fixed: 0, remaining: 0, changed: true };
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().repairProjectIntegrity();

    expect(useProjectStore.getState().isDirty).toBe(true);
  });

  it('does not dirty the project after a true no-op repair', async () => {
    // changed === false is the backend "nothing was mutated" signal - don't
    // spuriously dirty a project the repair left byte-identical (so the close
    // prompt doesn't fire on an unchanged project).
    useProjectStore.setState({ isDirty: false });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') return { fixed: 0, remaining: 1, changed: false };
      if (command === 'check_project_integrity') return { messages: ['detect-only'], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().repairProjectIntegrity();

    expect(useProjectStore.getState().isDirty).toBe(false);
  });

  it('drops a stale integrity result when the project switches mid-check', async () => {
    // A check is in flight for project A when the user opens B. The backend scans
    // whatever project is currently loaded (no path arg), so the late A result
    // must NOT overwrite B's freshly-reset banner/save-block state.
    useProjectStore.setState({
      project: { file_path: '/A.stalx' } as never,
      loading: { ...useProjectStore.getState().loading, clientOpId: 1 },
      integrityIssues: [],
      integritySaveBlocked: false,
    });

    let resolveA!: (v: { messages: string[]; save_blocked: boolean }) => void;
    invokeIpcMock.mockImplementationOnce(
      () => new Promise((r) => { resolveA = r as typeof resolveA; }),
    );
    const checkA = useProjectStore.getState().checkProjectIntegrity();

    // Switch to B: new project, integrity reset, clientOpId bumped (as a load does).
    useProjectStore.setState({
      project: { file_path: '/B.stalx' } as never,
      loading: { ...useProjectStore.getState().loading, clientOpId: 2 },
      integrityIssues: [],
      integritySaveBlocked: false,
    });

    // A's slow check now resolves with A's save-blocking issues.
    resolveA({ messages: ['A: merge references a deleted station'], save_blocked: true });
    await checkA;

    // B's state is untouched - the stale A result was dropped.
    expect(useProjectStore.getState().integrityIssues).toEqual([]);
    expect(useProjectStore.getState().integritySaveBlocked).toBe(false);
  });

  it('re-checks integrity after a mutating command so the banner cannot go stale', async () => {
    // Removing a drifted loop closure resolves its integrity issue. executeCommand
    // must re-check integrity so the banner clears - refreshProjectState does not,
    // and the next save may be an autosave (which skips its own post-save re-check).
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: ['drifted aligned loop closure'],
      integrityDismissed: false,
      integritySaveBlocked: false,
    });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'remove_loop_closure') return { success: true };
      if (command === 'get_project_state_binary') return new ArrayBuffer(0); // refreshProjectState
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined; // refreshUndoState / refreshWarnings - best-effort
    });

    await useProjectStore.getState().removeLoopClosure(1);

    expect(useProjectStore.getState().integrityIssues).toEqual([]);
  });

  it('drops a stale integrity result when an UNSAVED project is closed mid-check', async () => {
    // Unsaved project (file_path null); a check is in flight; the user closes it.
    // closeProject does not bump clientOpId and file_path stays null, so the result
    // must be dropped via the project===null guard - otherwise it leaks the banner
    // onto the no-project global view.
    useProjectStore.setState({
      project: { file_path: null } as never,
      loading: { ...useProjectStore.getState().loading, clientOpId: 1 },
      integrityIssues: [],
      integritySaveBlocked: false,
    });

    let resolveCheck!: (v: { messages: string[]; save_blocked: boolean }) => void;
    invokeIpcMock.mockImplementationOnce(
      () => new Promise((r) => { resolveCheck = r as typeof resolveCheck; }),
    );
    const check = useProjectStore.getState().checkProjectIntegrity();

    // Close: project → null, integrity reset, clientOpId unchanged.
    useProjectStore.setState({ project: null, integrityIssues: [], integritySaveBlocked: false });

    resolveCheck({ messages: ['stale issue'], save_blocked: true });
    await check;

    expect(useProjectStore.getState().integrityIssues).toEqual([]);
    expect(useProjectStore.getState().integritySaveBlocked).toBe(false);
  });

  it('drops a superseded integrity check (last-issued-wins on the same project)', async () => {
    // Two checks overlap on the SAME project (e.g. a manual save's fire-and-forget
    // check racing a Repair's check). The earlier-issued one resolving LAST must
    // not clobber the newer result - the project-identity guard can't tell them
    // apart, so the monotonic sequence does.
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      loading: { ...useProjectStore.getState().loading, clientOpId: 1 },
      integrityIssues: [],
      integritySaveBlocked: false,
    });

    let resolveFirst!: (v: { messages: string[]; save_blocked: boolean }) => void;
    invokeIpcMock.mockImplementationOnce(
      () => new Promise((r) => { resolveFirst = r as typeof resolveFirst; }),
    );
    const first = useProjectStore.getState().checkProjectIntegrity(); // call A (older)

    // Call B (newer) resolves immediately with the truth (clean).
    invokeIpcMock.mockImplementationOnce(async () => ({ messages: [], save_blocked: false }));
    await useProjectStore.getState().checkProjectIntegrity();
    expect(useProjectStore.getState().integrityIssues).toEqual([]);

    // A now resolves with a STALE pre-repair result - must be dropped (superseded).
    resolveFirst({ messages: ['stale pre-repair issue'], save_blocked: true });
    await first;

    expect(useProjectStore.getState().integrityIssues).toEqual([]);
    expect(useProjectStore.getState().integritySaveBlocked).toBe(false);
  });

  it('closes the integrity dialog when the last issue clears', async () => {
    // The dialog flag must not linger true once issues are gone, or it could pop
    // the modal back open when a later issue reappears.
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: ['some issue'],
      integrityDialogOpen: true,
      integritySaveBlocked: false,
    });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().checkProjectIntegrity();

    expect(useProjectStore.getState().integrityIssues).toEqual([]);
    expect(useProjectStore.getState().integrityDialogOpen).toBe(false);
  });

  it('surfaces a thrown repair instead of faking a no-op success', async () => {
    // A repair IPC rejection (window/app teardown, panic) must REJECT - not return
    // a fake { fixed:0, remaining:0 } that the caller reads as a successful repair
    // (fake toast + lost entry point). The banner/save-block must stay intact.
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: ['dangling merge'],
      integritySaveBlocked: true,
    });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'repair_project_integrity') throw new Error('ipc down');
      return undefined;
    });

    await expect(useProjectStore.getState().repairProjectIntegrity()).rejects.toThrow('ipc down');
    expect(useProjectStore.getState().integrityIssues).toEqual(['dangling merge']);
    expect(useProjectStore.getState().integritySaveBlocked).toBe(true);
  });

  it('refreshes project state after a save that healed a corrupt project', async () => {
    // The Rust save heals dup UUIDs / non-finite / orphan offsets in memory, so the
    // frontend must refresh (get_project_state_binary) to match the saved file.
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: ['non-finite shot geometry'],
      integritySaveBlocked: false,
    });
    let getProjectStateCalled = false;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') return { success: true };
      if (command === 'get_project_state_binary') { getProjectStateCalled = true; return new ArrayBuffer(0); }
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().saveProject('/p.stalx');
    expect(getProjectStateCalled).toBe(true);
  });

  it('does not refresh project state after a clean save', async () => {
    // No integrity issues → the save can't have healed anything → skip the extra
    // get_project_state_binary fetch (no churn on the common path).
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: [],
      integritySaveBlocked: false,
    });
    let getProjectStateCalled = false;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') return { success: true };
      if (command === 'get_project_state_binary') { getProjectStateCalled = true; return new ArrayBuffer(0); }
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().saveProject('/p.stalx');
    expect(getProjectStateCalled).toBe(false);
  });

  it('re-checks integrity after an AUTOSAVE that healed the project', async () => {
    // The save path auto-heals dup UUIDs / orphan offsets on autosave too. Even on
    // autosave we must re-check when the project entered the save flagged, or the
    // banner stays stale after the heal silently cleared it.
    useProjectStore.setState({
      project: { file_path: '/p.stalx' } as never,
      integrityIssues: ['duplicate shot UUID'],
      integrityDismissed: false,
      integritySaveBlocked: false,
    });
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'save_project') return { success: true };
      if (command === 'get_project_state_binary') return new ArrayBuffer(0); // refreshProjectState
      if (command === 'check_project_integrity') return { messages: [], save_blocked: false };
      return undefined;
    });

    await useProjectStore.getState().saveProject('/p.stalx', true); // isAutosave
    // checkProjectIntegrity is fire-and-forget; let it resolve.
    await new Promise((r) => setTimeout(r, 0));

    expect(useProjectStore.getState().integrityIssues).toEqual([]);
  });
});
