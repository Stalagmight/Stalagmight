/**
 * Unit tests for the unified Open/Import router. These pin the data-loss-sensitive
 * branches of `routeOpenedFile`: the dirty-`.stalx` deferral (must NOT load over an
 * unsaved project), the new-project-only-import rejection (GPX/TML/Compass while a
 * project is open), the unsupported-extension error, and that the pending
 * entry-point telemetry source is cleared on every abandon/reject/null/error
 * branch (so it can't mis-attribute a later, unrelated command).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

const openMock = vi.fn();
vi.mock('@tauri-apps/plugin-dialog', () => ({
  open: (...args: unknown[]) => openMock(...args),
}));

import { routeOpenedFile, pickAndRoute } from '../openOrImport';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { setProjectActionSource, peekProjectActionSource } from '../telemetry';

const READ_ONLY_LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE',
  editState: 'read-only',
  lockHolder: 'someone-else',
  lockedByOther: true,
  hasUnpushedEdits: false,
  unwound: false,

  undoMode: 'full',
  remoteState: 'up_to_date',
  noAccess: false,
  serverError: null,
};

const loadProject = vi.fn().mockResolvedValue({ success: true });
const importDmp = vi.fn().mockResolvedValue(null);
const parseGpx = vi.fn().mockResolvedValue(null);

beforeEach(() => {
  vi.clearAllMocks();
  importDmp.mockResolvedValue(null);
  useUIStore.setState({ activeDialog: null, dialogData: {} });
  useProjectStore.setState({
    project: null,
    isDirty: false,
    loadProject,
    importDmp,
    parseGpx,
  } as never);
  // The open project (where a test opens one) is a CONFIRMED-local project: the
  // SpeleoDB read-only gate fails closed since #270, so the editable baseline has
  // to say so rather than leave the gate unresolved.
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  // Each test starts with an armed entry-point source so we can assert clearing.
  setProjectActionSource('keyboard');
});

describe('routeOpenedFile', () => {
  it('defers a .stalx open over a DIRTY project to unsaved-changes (no loadProject)', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: true });
    await routeOpenedFile('/cave/project.stalx');
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toMatchObject({ pendingOpenPath: '/cave/project.stalx' });
    expect(loadProject).not.toHaveBeenCalled();
    // Handed off to the dialog - the source must survive for the deferred load.
    expect(peekProjectActionSource()).toBe('keyboard');
  });

  it('loads a .stalx directly when no project is open', async () => {
    await routeOpenedFile('/cave/project.stalx');
    expect(loadProject).toHaveBeenCalledWith('/cave/project.stalx');
    expect(useUIStore.getState().activeDialog).not.toBe('unsaved-changes');
  });

  it('raises the swap-confirm dialog for a .stalx over a CLEAN project instead of opening directly', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    await routeOpenedFile('/cave/other.stalx');
    expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
    expect(useUIStore.getState().dialogData).toMatchObject({
      path: '/cave/other.stalx',
      fileName: 'other.stalx',
    });
    expect(loadProject).not.toHaveBeenCalled();
    // Handed off to the dialog - the source must survive for confirm to complete it.
    expect(peekProjectActionSource()).toBe('keyboard');
  });

  it('opens on confirm: swapConfirmed skips the dialog and re-enters at loadProject', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    await routeOpenedFile('/cave/other.stalx', 'Open Failed', { swapConfirmed: true });
    expect(useUIStore.getState().activeDialog).not.toBe('launch-open-confirm');
    expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx');
  });

  it('cancel (never re-invoking) leaves the current project untouched', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    await routeOpenedFile('/cave/other.stalx');
    expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
    // No re-route follows - a decline never calls routeOpenedFile again.
    expect(loadProject).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project).toEqual({ name: 'X' });
  });

  it('offers "import as new project?" for a GPX import while a project is open, handing off the source', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    await routeOpenedFile('/cave/track.gpx');
    expect(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect(useUIStore.getState().dialogData).toMatchObject({
      path: '/cave/track.gpx',
      fileName: 'track.gpx',
    });
    expect(parseGpx).not.toHaveBeenCalled();
    // Handed off to the dialog - the source must survive until it resolves.
    expect(peekProjectActionSource()).toBe('keyboard');
  });

  describe('a .stalx through an in-project Import door (#674)', () => {
    it('asks open-or-add instead of opening, handing off the source', async () => {
      useProjectStore.setState({ project: { name: 'X' } as never, isDirty: true });
      await routeOpenedFile('/cave/other.stalx', 'Import Failed', { importDoor: true });
      expect(useUIStore.getState().activeDialog).toBe('add-or-open-project');
      expect(useUIStore.getState().dialogData).toMatchObject({
        path: '/cave/other.stalx',
        fileName: 'other.stalx',
      });
      expect(loadProject).not.toHaveBeenCalled();
      expect(peekProjectActionSource()).toBe('keyboard');
    });

    it('picking the open project\'s own file shows an already-open toast, not the dialog', async () => {
      useProjectStore.setState({
        project: { name: 'X', file_path: '/cave/self.stalx' } as never,
        isDirty: false,
      });
      await routeOpenedFile('/cave/self.stalx', 'Import Failed', { importDoor: true });
      expect(useUIStore.getState().activeDialog).toBeNull();
      expect(loadProject).not.toHaveBeenCalled();
      expect(useUIStore.getState().toasts.map((x) => x.key)).toContain('import-already-open');
    });

    it('is unchanged for the Open doors: a dirty project still defers to unsaved-changes', async () => {
      useProjectStore.setState({ project: { name: 'X' } as never, isDirty: true });
      await routeOpenedFile('/cave/other.stalx', 'Open Failed');
      expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    });

    it('is unchanged for the Open doors: a clean project still raises the swap confirm', async () => {
      useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
      await routeOpenedFile('/cave/other.stalx', 'Open Failed', { importDoor: false });
      expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
    });

    it('with no project open the Import door opens the file directly (nothing to add to)', async () => {
      await routeOpenedFile('/cave/other.stalx', 'Import Failed', { importDoor: true });
      expect(loadProject).toHaveBeenCalledWith('/cave/other.stalx');
    });

    it('pickAndRoute forwards importDoor to the router', async () => {
      useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
      openMock.mockResolvedValue('/cave/other.stalx');
      await pickAndRoute([{ name: 'Project', extensions: ['stalx'] }], 'Import Failed', { importDoor: true });
      expect(useUIStore.getState().activeDialog).toBe('add-or-open-project');
    });
  });

  it('shows an unsupported-file error for an unknown extension and clears the source', async () => {
    await routeOpenedFile('/cave/notes.xyz');
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(String(useUIStore.getState().dialogData.message)).toContain('Unsupported file type');
    expect(peekProjectActionSource()).toBeUndefined();
  });

  it('clears the source when a DMP parse returns null (abandoned)', async () => {
    importDmp.mockResolvedValue(null);
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).toHaveBeenCalledWith('/cave/survey.dmp');
    // No dialog handed off; the parse bailed → source cleared in finally.
    expect(peekProjectActionSource()).toBeUndefined();
  });

  it('does not leak the full path in the unsupported-file message for a path ending in a separator', async () => {
    await routeOpenedFile('/Users/someone/caves/');
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(String(useUIStore.getState().dialogData.message)).not.toContain('/');
  });
});

describe('routeOpenedFile - allow: "project-only" (#450 security)', () => {
  it('refuses a non-.stalx recents entry and starts no import flow', async () => {
    await routeOpenedFile('/x/evil.dat', 'Open Failed', { allow: 'project-only' });
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(String(useUIStore.getState().dialogData.message)).toContain('Unsupported file type');
    expect(loadProject).not.toHaveBeenCalled();
    expect(importDmp).not.toHaveBeenCalled();
    expect(parseGpx).not.toHaveBeenCalled();
    expect(peekProjectActionSource()).toBeUndefined();
  });

  it('still opens a .stalx recents entry normally', async () => {
    await routeOpenedFile('/cave/project.stalx', 'Open Failed', { allow: 'project-only' });
    expect(loadProject).toHaveBeenCalledWith('/cave/project.stalx');
    expect(useUIStore.getState().activeDialog).not.toBe('error');
  });
});

describe('routeOpenedFile - read-only-map import guard (#243)', () => {
  beforeEach(() => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
  });
  // Reset the mode so a case that sets Plan/adjusted can't leak into later tests.
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  });

  it('allows a DMP import in Survey + measured (the editable baseline)', async () => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).toHaveBeenCalledWith('/cave/survey.dmp');
    expect(useUIStore.getState().activeDialog).not.toBe('error');
  });

  it('blocks a DMP import in the closure-adjusted survey view with the import-blocked prompt (cause: view)', async () => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'closureAdjusted' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('import-blocked');
    expect(useUIStore.getState().dialogData).toMatchObject({ path: '/cave/survey.dmp', cause: 'view' });
  });

  // The regression the #243 guard closes: Plan/Export force closureAdjusted, but that
  // auto-reverts to 'measured' on a no-closures project, so a geometryMode-only guard
  // would let the import through and mutate the survey under the read-only map.
  // Mode-only blocking raises the import-blocked CHOICE dialog (not an error): the
  // fix is one safe click, and the dialog carries the picked path so "Switch to
  // Survey" can continue this exact import.
  it('blocks a DMP import in Plan even when geometryMode fell back to measured (import-blocked prompt)', async () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('import-blocked');
    expect(useUIStore.getState().dialogData).toMatchObject({ path: '/cave/survey.dmp', cause: 'mode' });
  });

  it('blocks a DMP import in Export (measured fallback) with the same import-blocked prompt', async () => {
    useUIStore.setState({ appMode: 'export', geometryMode: 'measured' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('import-blocked');
  });

  it('allows a DMP import in Data + measured: Data is editable like Survey', async () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'measured' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).toHaveBeenCalledWith('/cave/survey.dmp');
    expect(useUIStore.getState().activeDialog).not.toBe('error');
  });

  // Data is not a read-only mode, so the closure view is the whole cause: the
  // prompt must offer the view switch, not "Switch to Survey" (which would not fix it).
  it('blocks a DMP import in the closure-adjusted Data view with cause: view, not mode', async () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'closureAdjusted' });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('import-blocked');
    expect(useUIStore.getState().dialogData).toMatchObject({ path: '/cave/survey.dmp', cause: 'view' });
  });

  it('a locked remote project in the closure-adjusted Data view names the view, not Survey mode', async () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'closureAdjusted' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    await routeOpenedFile('/cave/survey.dmp');
    const message = String(useUIStore.getState().dialogData.message);
    expect(message).toContain('Raw Data View');
    expect(message).not.toContain('Survey mode');
  });

  it('still opens a native .stalx in Plan (opening a project is not an import)', async () => {
    useProjectStore.setState({ project: null, isDirty: false });
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    await routeOpenedFile('/cave/project.stalx');
    expect(loadProject).toHaveBeenCalledWith('/cave/project.stalx');
    expect(useUIStore.getState().activeDialog).not.toBe('error');
  });

  it('blocks a DMP import on a locked remote SpeleoDB project (Survey + measured otherwise editable)', async () => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(String(useUIStore.getState().dialogData.message)).toContain('SpeleoDB write lock');
  });

  // Compound read-only state (#275): a locked remote project ALSO viewed in a
  // non-survey mode must still mention the write lock, not just "switch to Survey" -
  // silently dropping the SpeleoDB cause from the message would leave the user
  // stuck after switching modes with no lock acquired.
  it('mentions the SpeleoDB write lock even when ALSO in Plan mode (compound state)', async () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('error');
    const message = String(useUIStore.getState().dialogData.message);
    expect(message).toContain('SpeleoDB write lock');
    expect(message).toContain('Survey mode');
  });

  // The switch-and-continue re-route from ImportBlockedDialog: the one cause a
  // switch cannot fix (the SpeleoDB write lock) must inform via toast and get
  // out of the way - the user already clicked once.
  it('reports a residual SpeleoDB-lock cause as a toast, not a second modal, when re-routed with residualReadOnlyToast', async () => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    await routeOpenedFile('/cave/survey.dmp', 'Import Failed', { residualReadOnlyToast: true });
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
    const toasts = useUIStore.getState().toasts;
    expect(JSON.stringify(toasts)).toContain('SpeleoDB write lock');
  });

  it('allows a DMP import once the SpeleoDB write lock is held', async () => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: { ...READ_ONLY_LINK, editState: 'editing', lockedByOther: false } });
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).toHaveBeenCalledWith('/cave/survey.dmp');
    expect(useUIStore.getState().activeDialog).not.toBe('error');
  });
});

describe('routeOpenedFile - the read-only guard is scoped to DMP only (#392)', () => {
  beforeEach(() => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
  });
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  });

  it('still offers "import as new project?" for a TML while editing is blocked (close-and-import does not mutate)', async () => {
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect(useUIStore.getState().dialogData).toMatchObject({
      path: '/cave/track.tml',
      fileName: 'track.tml',
    });
    // Handed off to the dialog, not cleared as a reject.
    expect(peekProjectActionSource()).toBe('keyboard');
  });

  it('still blocks a DMP import while editing is blocked, via the import-blocked prompt that hands off the source', async () => {
    await routeOpenedFile('/cave/survey.dmp');
    expect(importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('import-blocked');
    // The dialog owns the source now: "Switch to Survey" continues this import
    // (source survives into the DMP flow); a cancel clears it via
    // checkDialogCancel, not the router's finally.
    expect(peekProjectActionSource()).toBe('keyboard');
  });
});

/**
 * #411: `importContext` decides whether `saveAfterImport` WRITES the .stalx (a new
 * project's initial save is deferred) or only marks dirty. Only the DMP branch used
 * to set it, so a TML/Compass/Walls new-project import that reached orphan
 * placement took the existing-project branch and never wrote the merges to disk -
 * and because the flag has no auto-reset, whether it did depended on which import
 * happened to run earlier in the session. GPX is covered here for consistency
 * only: no GPX path reaches orphan placement or calls `saveAfterImport`, so it
 * declares a context nothing reads today.
 */
describe('routeOpenedFile - import context (#411)', () => {
  const realSaveProject = useProjectStore.getState().saveProject;
  const realShowToast = useUIStore.getState().showToast;
  const saveProject = vi.fn().mockResolvedValue({ success: true });
  const showToast = vi.fn();
  const parseTml = vi.fn();
  const parseCompass = vi.fn();
  const parseWalls = vi.fn();

  beforeEach(() => {
    parseTml.mockResolvedValue({ sections: [] });
    parseCompass.mockResolvedValue({ survey_count: 0 });
    parseWalls.mockResolvedValue({ project_name: 'W' });
    parseGpx.mockResolvedValue({ waypoints: [] });
    saveProject.mockResolvedValue({ success: true });
    useUIStore.setState({ importContext: null, showToast });
    useProjectStore.setState({ parseTml, parseCompass, parseWalls, saveProject } as never);
  });

  afterEach(() => {
    // `vi.clearAllMocks()` keeps implementations, so hand every mock this describe
    // injected back to the module-level / real baseline - otherwise the last test's
    // null-returning parse would decide a later spec in this file.
    parseGpx.mockResolvedValue(null);
    parseTml.mockResolvedValue(null);
    parseCompass.mockResolvedValue(null);
    parseWalls.mockResolvedValue(null);
    useUIStore.setState({ importContext: null, showToast: realShowToast, placementMode: null });
    useProjectStore.setState({ saveProject: realSaveProject } as never);
  });

  it.each([
    ['/cave/track.tml', 'tml'],
    ['/cave/track.tmlu', 'tml'],
    ['/cave/survey.mak', 'compass'],
    ['/cave/survey.wpj', 'walls'],
    ['/cave/track.gpx', 'gpx'],
  ])('declares a new-project import context for %s', async (path, importType) => {
    await routeOpenedFile(path);
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().dialogData).toMatchObject({ importType });
    expect(useUIStore.getState().importContext).toBe('new-project');
  });

  // The DMP branch is the one that always declared its context; pin both halves so
  // deleting either `setImportContext` call fails here rather than in the field.
  it('declares a new-project import context for a DMP with no project open', async () => {
    importDmp.mockResolvedValue({ sections: [] });
    await routeOpenedFile('/cave/survey.dmp');
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBe('new-project');
  });

  it('declares an existing-project import context for a DMP into an open project', async () => {
    useProjectStore.setState({ project: { name: 'X' } as never });
    importDmp.mockResolvedValue({ sections: [] });
    await routeOpenedFile('/cave/survey.dmp');
    expect(useUIStore.getState().activeDialog).toBe('dmp-selection');
    expect(useUIStore.getState().importContext).toBe('existing-project');
  });

  it('saves to disk when an orphan placement finishes a TML new-project import', async () => {
    await routeOpenedFile('/cave/track.tml');
    // What OrphanPlacementDialog.finish() does once the merges are applied.
    const proceeded = await useProjectStore.getState().saveAfterImport();
    expect(saveProject).toHaveBeenCalledTimes(1);
    expect(proceeded).toBe(true);
  });

  it('does not inherit an EXISTING-project context from an earlier import', async () => {
    // A DMP import into an open project ran earlier this session.
    useUIStore.setState({ importContext: 'existing-project' });
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().importContext).toBe('new-project');
    await useProjectStore.getState().saveAfterImport();
    expect(saveProject).toHaveBeenCalledTimes(1);
  });

  it('does not leave a NEW-project context behind when the flow never starts', async () => {
    // A new-project import ran earlier this session; this one abandons at the parse.
    useUIStore.setState({ importContext: 'new-project' });
    parseTml.mockResolvedValue(null);
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().activeDialog).not.toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBeNull();
  });

  // Drag-drop routes unconditionally (App.tsx's drop listener has no modal check),
  // so a second file arriving mid-flow re-enters the router. The context reset must
  // not fire there: the in-flight flow's saveAfterImport is still to come, and a
  // second file that ALSO fails to parse would leave it null - the same lost save,
  // reached through concurrency.
  it('refuses a second file while an import flow owns the screen, keeping its context', async () => {
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBe('new-project');

    parseTml.mockResolvedValue(null); // the dropped second file is unreadable
    await routeOpenedFile('/cave/other.tml');

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'import-in-progress', 'warning');
    expect(parseTml).toHaveBeenCalledTimes(1); // the second route never got that far
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBe('new-project');
    // The flow that owns the screen still reaches disk when it finishes.
    await useProjectStore.getState().saveAfterImport();
    expect(saveProject).toHaveBeenCalledTimes(1);
  });

  // Was "still routes when a NON-import dialog owns the screen" - that let a
  // dropped (or OS-launched, or recents-opened) file call loadProject/openDialog
  // straight over settings, an error dialog, or a pending unsaved-changes
  // prompt, because `importFlowOwnsScreen` only recognizes the import wizard's
  // OWN dialogs. Now mirrors `useKeyboardShortcuts`' `activeDialog && key !==
  // 'Escape'` gate: ANY dialog refuses a new route, with the toast the
  // in-flight-import case already used.
  it('refuses to route while a non-import dialog (e.g. settings) owns the screen', async () => {
    useUIStore.setState({ activeDialog: 'settings', dialogData: {} });
    await routeOpenedFile('/cave/track.tml');
    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'import-in-progress', 'warning');
    expect(useUIStore.getState().activeDialog).toBe('settings');
    expect(useUIStore.getState().importContext).toBeNull();
  });

  it('refuses to route while an error dialog owns the screen', async () => {
    useUIStore.setState({ activeDialog: 'error', dialogData: {} });
    await routeOpenedFile('/cave/track.tml');
    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'import-in-progress', 'warning');
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(parseTml).not.toHaveBeenCalled();
  });

  // The one deliberate exception: LaunchOpenConfirmDialog re-routes the SAME
  // path through here while `launch-open-confirm` is still the active dialog
  // (routes BEFORE closing itself, to keep the dialog slot occupied for the
  // whole flow).
  it('still routes with swapConfirmed while launch-open-confirm owns the screen', async () => {
    useUIStore.setState({ activeDialog: 'launch-open-confirm', dialogData: {} });
    await routeOpenedFile('/cave/track.tml', 'Open Failed', { swapConfirmed: true });
    expect(showToast).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBe('new-project');
  });

  // The one in-flight phase with NO dialog: the DMP wizard's manual placement
  // calls startPlacementMode() and then closeDialog() so the user can work the
  // map, leaving activeDialog null while saveAfterImport is still to come.
  it('refuses a file during manual placement, when no dialog is open at all', async () => {
    importDmp.mockResolvedValue({ sections: [] });
    await routeOpenedFile('/cave/survey.dmp');
    expect(useUIStore.getState().importContext).toBe('new-project');

    useUIStore.getState().startPlacementMode({} as never, 0);
    useUIStore.getState().closeDialog();
    expect(useUIStore.getState().activeDialog).toBeNull();

    parseTml.mockResolvedValue(null); // a second file arrives mid-placement
    await routeOpenedFile('/cave/track.tml');

    expect(showToast).toHaveBeenCalledWith(expect.any(String), 'import-in-progress', 'warning');
    expect(parseTml).not.toHaveBeenCalled();
    expect(useUIStore.getState().importContext).toBe('new-project');
    // The placing flow still reaches disk when it finishes.
    await useProjectStore.getState().saveAfterImport();
    expect(saveProject).toHaveBeenCalledTimes(1);
  });

  // Both placement exits must clear the signal, or the guard would wedge the
  // router for the rest of the session.
  it.each([
    ['confirmed', () => useUIStore.getState().confirmPlacement()],
    ['cancelled', () => useUIStore.getState().cancelPlacementMode()],
  ])('routes again once placement is %s', async (_label, endPlacement) => {
    useUIStore.getState().startPlacementMode({} as never, 0);
    useUIStore.getState().closeDialog();
    endPlacement();
    expect(useUIStore.getState().placementMode).toBeNull();

    await routeOpenedFile('/cave/track.tml');

    expect(showToast).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().importContext).toBe('new-project');
  });
});

describe('routeOpenedFile - default project name prefers the file-carried name (#447)', () => {
  const parseTml = vi.fn();
  const parseCompass = vi.fn();
  const parseWalls = vi.fn();
  const parseGpxFile = vi.fn();

  beforeEach(() => {
    useProjectStore.setState({
      parseTml,
      parseCompass,
      parseWalls,
      parseGpx: parseGpxFile,
    } as never);
  });

  // One row per format: the in-file name field, the path, and the filename
  // fallback ('Imported' has no extension to strip). Covers all four formats
  // that carry an in-file name (DMP does not - no case for it).
  it.each([
    {
      label: 'TML cave_name',
      path: '/cave/exports/some-file.tml',
      mock: parseTml,
      base: { sections: [] },
      field: 'cave_name',
      fallback: 'some-file',
    },
    {
      label: 'Compass cave_name',
      path: '/cave/exports/some-file.mak',
      mock: parseCompass,
      base: { survey_count: 0, shot_count: 0 },
      field: 'cave_name',
      fallback: 'some-file',
    },
    {
      label: 'Walls project_name',
      path: '/cave/exports/some-file.wpj',
      mock: parseWalls,
      base: { resolved_survey_count: 0, shot_count: 0 },
      field: 'project_name',
      fallback: 'some-file',
    },
    {
      label: 'GPX name',
      path: '/cave/exports/some-file.gpx',
      mock: parseGpxFile,
      base: { waypoints: [], tracks: [], routes: [] },
      field: 'name',
      fallback: 'some-file',
    },
  ])('$label: uses the in-file name over the filename', async ({ path, mock, base, field, fallback }) => {
    mock.mockResolvedValue({ ...base, [field]: 'Naharon' });
    await routeOpenedFile(path);
    expect(useUIStore.getState().dialogData).toMatchObject({ defaultName: 'Naharon' });
    void fallback; // unused in this case, kept for a single shared row shape
  });

  it.each([
    { label: 'TML cave_name', path: '/cave/exports/some-file.tml', mock: parseTml, base: { sections: [] }, field: 'cave_name' },
    { label: 'Compass cave_name', path: '/cave/exports/some-file.mak', mock: parseCompass, base: { survey_count: 0, shot_count: 0 }, field: 'cave_name' },
    { label: 'Walls project_name', path: '/cave/exports/some-file.wpj', mock: parseWalls, base: { resolved_survey_count: 0, shot_count: 0 }, field: 'project_name' },
    { label: 'GPX name', path: '/cave/exports/some-file.gpx', mock: parseGpxFile, base: { waypoints: [], tracks: [], routes: [] }, field: 'name' },
  ])('$label: trims surrounding whitespace', async ({ path, mock, base, field }) => {
    mock.mockResolvedValue({ ...base, [field]: '  Naharon  ' });
    await routeOpenedFile(path);
    expect(useUIStore.getState().dialogData).toMatchObject({ defaultName: 'Naharon' });
  });

  it.each([
    { label: 'TML cave_name empty', path: '/cave/exports/some-file.tml', mock: parseTml, base: { sections: [] }, field: 'cave_name', value: '' },
    { label: 'Compass cave_name empty', path: '/cave/exports/some-file.mak', mock: parseCompass, base: { survey_count: 0, shot_count: 0 }, field: 'cave_name', value: '' },
    { label: 'Walls project_name empty', path: '/cave/exports/some-file.wpj', mock: parseWalls, base: { resolved_survey_count: 0, shot_count: 0 }, field: 'project_name', value: '' },
    { label: 'GPX name null', path: '/cave/exports/some-file.gpx', mock: parseGpxFile, base: { waypoints: [], tracks: [], routes: [] }, field: 'name', value: null },
    { label: 'TML cave_name whitespace-only', path: '/cave/exports/some-file.tml', mock: parseTml, base: { sections: [] }, field: 'cave_name', value: '   ' },
  ])('$label: falls back to the filename', async ({ path, mock, base, field, value }) => {
    mock.mockResolvedValue({ ...base, [field]: value });
    await routeOpenedFile(path);
    expect(useUIStore.getState().dialogData).toMatchObject({ defaultName: 'some-file' });
  });
});

describe('routeOpenedFile - hideReportHint is scoped to the parse call, not the whole router (#447)', () => {
  const parseTml = vi.fn();
  // routeOpenedFile calls `ui.setImportContext(null)` unconditionally near
  // the top, before any extension branch - overriding it to throw (below)
  // would poison every later test in this file unless restored.
  const realSetImportContext = useUIStore.getState().setImportContext;

  beforeEach(() => {
    useProjectStore.setState({ parseTml } as never);
  });

  afterEach(() => {
    useUIStore.setState({ setImportContext: realSetImportContext });
  });

  it('hides the report-a-bug hint when the TML parser itself rejects (a malformed file)', async () => {
    parseTml.mockRejectedValue(new Error('unexpected end of XML'));
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(useUIStore.getState().dialogData).toMatchObject({
      message: 'unexpected end of XML',
      hideReportHint: true,
    });
  });

  it('keeps the report-a-bug hint when something OTHER than the parser throws (a routing/store bug)', async () => {
    parseTml.mockResolvedValue({ sections: [], cave_name: 'Naharon' });
    // A throw from routing code AFTER a successful parse - not a file problem.
    useUIStore.setState({
      setImportContext: () => {
        throw new Error('unexpected store fault');
      },
    });
    await routeOpenedFile('/cave/track.tml');
    expect(useUIStore.getState().activeDialog).toBe('error');
    const data = useUIStore.getState().dialogData;
    expect(data.message).toBe('unexpected store fault');
    expect(data.hideReportHint).toBeUndefined();
  });
});

describe('pickAndRoute', () => {
  it('clears the source when the picker is cancelled (null result)', async () => {
    openMock.mockResolvedValue(null);
    await pickAndRoute([{ name: 'Project', extensions: ['stalx'] }]);
    expect(peekProjectActionSource()).toBeUndefined();
  });

  it('routes the chosen path through routeOpenedFile (string result)', async () => {
    openMock.mockResolvedValue('/cave/project.stalx');
    await pickAndRoute([{ name: 'Project', extensions: ['stalx'] }]);
    expect(loadProject).toHaveBeenCalledWith('/cave/project.stalx');
  });

  it('clears the source and shows an error when the picker itself throws', async () => {
    openMock.mockRejectedValue(new Error('picker exploded'));
    await pickAndRoute([{ name: 'Project', extensions: ['stalx'] }], 'Open Failed');
    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(peekProjectActionSource()).toBeUndefined();
  });
});
