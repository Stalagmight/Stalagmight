/**
 * Unit tests for the shared file-action helper used by both the SingleSystemView
 * and GlobalView keyboard dispatchers. The key behavior under test is the
 * New-project unsaved-changes guard (the Ctrl+N data-loss fix): it must defer to
 * the unsaved-changes dialog when a dirty project is open, and open the
 * new-project dialog directly otherwise.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn().mockResolvedValue({}),
}));

// Spy on the picker so the read-only guard can be asserted without touching dialogs.
const pickAndRouteMock = vi.fn();
vi.mock('../openOrImport', () => ({
  pickAndRoute: (...args: unknown[]) => pickAndRouteMock(...args),
}));

import { runFileShortcutAction } from '../fileShortcutActions';
import { openOrImportFilters, inProjectImportFilters } from '../fileDialogs';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';

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

describe('runFileShortcutAction("new_file")', () => {
  beforeEach(() => {
    useUIStore.setState({ activeDialog: null, dialogData: {} });
    useProjectStore.setState({ project: null, isDirty: false });
  });

  it('opens new-project directly when no project is open (GlobalView case)', () => {
    runFileShortcutAction('new_file');
    expect(useUIStore.getState().activeDialog).toBe('new-project');
  });

  it('opens new-project directly when the open project is clean', () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    runFileShortcutAction('new_file');
    expect(useUIStore.getState().activeDialog).toBe('new-project');
  });

  it('defers to the unsaved-changes dialog when a dirty project is open (Ctrl+N guard)', () => {
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: true });
    runFileShortcutAction('new_file');
    expect(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    expect(useUIStore.getState().dialogData).toMatchObject({ pendingNewProject: true });
  });
});

describe('runFileShortcutAction - Ctrl+O vs Ctrl+I (#508)', () => {
  beforeEach(() => {
    pickAndRouteMock.mockClear();
    useProjectStore.setState({ project: { name: 'X' } as never, isDirty: false });
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    // Confirmed-local project - the editable baseline (#270 fail-closed gate).
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  });
  afterEach(() => {
    useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  });

  it('with a project open, Ctrl+O stays unified and Ctrl+I narrows to .dmp/.stalx as an import door', () => {
    runFileShortcutAction('open_file');
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledTimes(2);
    const [openFilters, openTitle, openOpts] = pickAndRouteMock.mock.calls[0]!;
    const [importFilters, importTitle, importOpts] = pickAndRouteMock.mock.calls[1]!;
    expect(openFilters).toEqual(openOrImportFilters());
    expect(importFilters).toEqual(inProjectImportFilters());
    // Not just "different from Open": pin the exact extension set, so a filter
    // list that quietly regrew .tml entries fails here rather than
    // re-offering formats that cannot be imported INTO the open project.
    expect(new Set((importFilters as Array<{ extensions: string[] }>).flatMap((f) => f.extensions)))
      .toEqual(new Set(['dmp', 'stalx']));
    expect(openTitle).toBe('Open Failed');
    expect(importTitle).toBe('Import Failed');
    // Only the Import door asks open-or-add (#674); Ctrl+O is untouched.
    expect(openOpts).toBeUndefined();
    expect(importOpts).toEqual({ importDoor: true });
  });

  // With no project open there is nothing to import INTO, so Ctrl+I keeps the
  // full picker (the recents screen drops the Import row but keeps the binding).
  it('with no project open, Ctrl+I falls back to the unified picker', () => {
    useProjectStore.setState({ project: null });
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledWith(openOrImportFilters(), 'Import Failed', { importDoor: false });
  });

  // The pre-#407 Ctrl+I pre-guard silently swallowed the keystroke whenever
  // editing was blocked, which also blocked the legal non-mutating routes
  // (.stalx open, import-as-new-project). The picker now always opens; the
  // additive DMP-into-open-project case is rejected by routeOpenedFile's
  // read-only guard with a cause-specific message (pinned in
  // openOrImport.test.ts, the #243 guard's home).
  it('Ctrl+I in Data is the same in-project import door as in Survey', () => {
    useUIStore.setState({ appMode: 'data', geometryMode: 'measured' });
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledWith(inProjectImportFilters(), 'Import Failed', { importDoor: true });
  });

  it('import_dmp opens the picker even when editing is blocked (router guards the DMP case)', () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledWith(inProjectImportFilters(), 'Import Failed', { importDoor: true });
  });
});
