/**
 * Unit tests for the shared file-action helper used by both the SingleSystemView
 * and GlobalView keyboard dispatchers. The key behavior under test is the
 * New-project unsaved-changes guard (the Ctrl+N data-loss fix): it must defer to
 * the unsaved-changes dialog when a dirty project is open, and open the
 * new-project dialog directly otherwise.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

// Spy on the picker so the read-only guard can be asserted without touching dialogs.
const pickAndRouteMock = vi.fn();
vi.mock('../openOrImport', () => ({
  pickAndRoute: (...args: unknown[]) => pickAndRouteMock(...args),
}));

import { runFileShortcutAction } from '../fileShortcutActions';
import { openOrImportFilters, dmpImportFilters } from '../fileDialogs';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../store/speleoStore';
import { makeSpeleoLink } from '../../test/factories/speleo';

const READ_ONLY_LINK: SpeleoLinkState = makeSpeleoLink({ lockHolder: 'someone-else', lockedByOther: true });

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

  it('with a project open, Ctrl+O stays unified and Ctrl+I narrows to .dmp', () => {
    runFileShortcutAction('open_file');
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledTimes(2);
    const [openFilters, openTitle] = pickAndRouteMock.mock.calls[0]!;
    const [importFilters, importTitle] = pickAndRouteMock.mock.calls[1]!;
    expect(openFilters).toEqual(openOrImportFilters());
    expect(importFilters).toEqual(dmpImportFilters());
    // Not just "different from Open": pin the exact extension set, so a filter
    // list that quietly regrew .tml/.stalx entries fails here rather than
    // re-offering formats that cannot be imported INTO the open project.
    expect((importFilters as Array<{ extensions: string[] }>).flatMap((f) => f.extensions))
      .toEqual(['dmp']);
    expect(openTitle).toBe('Open Failed');
    expect(importTitle).toBe('Import Failed');
  });

  // With no project open there is nothing to import INTO, so Ctrl+I keeps the
  // full picker (the recents screen drops the Import row but keeps the binding).
  it('with no project open, Ctrl+I falls back to the unified picker', () => {
    useProjectStore.setState({ project: null });
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledWith(openOrImportFilters(), 'Import Failed');
  });

  // The pre-#407 Ctrl+I pre-guard silently swallowed the keystroke whenever
  // editing was blocked, which also blocked the legal non-mutating routes
  // (.stalx open, import-as-new-project). The picker now always opens; the
  // additive DMP-into-open-project case is rejected by routeOpenedFile's
  // read-only guard with a cause-specific message (pinned in
  // openOrImport.test.ts, the #243 guard's home).
  it('import_dmp opens the picker even when editing is blocked (router guards the DMP case)', () => {
    useUIStore.setState({ appMode: 'plan', geometryMode: 'measured' });
    useSpeleoStore.setState({ link: READ_ONLY_LINK });
    runFileShortcutAction('import_dmp');
    expect(pickAndRouteMock).toHaveBeenCalledWith(dmpImportFilters(), 'Import Failed');
  });
});
