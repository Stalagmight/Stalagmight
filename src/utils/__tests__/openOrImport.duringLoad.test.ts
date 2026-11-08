/**
 * [#576 W87a-1] A routed file while a project load holds the screen.
 *
 * A load shows the loading window and registers it as a screen occupant, so the
 * shortcut hooks and the OS-launch queue stand down until it ends. A file dropped
 * on the window is a native webview event, though, and the router read neither the
 * loading state nor the occupant count: a `.dmp` dropped while project B loaded
 * over A opened A's import wizard over the loading window (the backend refused the
 * import later with PROJECT_CHANGED, so nothing was lost, but the dialog was for a
 * project on its way out).
 *
 * Now every routed file but a `.stalx` is refused with a toast while a load is in
 * progress. A `.stalx` is a newer switch, and the latest switch wins, so it still
 * goes through. All data is synthetic.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { routeOpenedFile } from '../openOrImport';
import { TOAST_KEYS } from '../toastKeys';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { initialLoadingState } from '../../store/projectLoadingState';
import { useSpeleoStore } from '../../store/speleoStore';

const parsers = {
  importDmp: vi.fn(),
  parseTml: vi.fn(),
  parseCompass: vi.fn(),
  parseWalls: vi.fn(),
  parseGpx: vi.fn(),
};
const loadProject = vi.fn();
const realShowToast = useUIStore.getState().showToast;
const showToast = vi.fn();

/** The loading state of project B's load, in progress. */
const LOADING_B = { ...initialLoadingState, clientOpId: 7, isLoading: true, projectName: 'CaveB', projectPath: '/caves/CaveB.stalx' };

/** Every non-project file a drop can route, each with the parser it would reach. */
const NOT_A_PROJECT: Array<[path: string, parser: keyof typeof parsers | null]> = [
  ['/caves/CaveA.dmp', 'importDmp'],
  ['/caves/CaveA.tml', 'parseTml'],
  ['/caves/CaveA.mak', 'parseCompass'],
  ['/caves/CaveA.wpj', 'parseWalls'],
  ['/caves/CaveA.gpx', 'parseGpx'],
  ['/caves/notes.txt', null],
];

beforeEach(() => {
  vi.clearAllMocks();
  for (const parser of Object.values(parsers)) parser.mockResolvedValue(null);
  loadProject.mockResolvedValue({ success: true });
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    importContext: null,
    placementMode: null,
    importParse: null,
    showToast,
  });
  // Project A is open and clean while B loads over it.
  useProjectStore.setState({
    project: { name: 'CaveA', created_at: '2026-03-03T10:00:00Z', file_path: '/caves/CaveA.stalx' },
    isDirty: false,
    loading: LOADING_B,
    loadProject,
    ...parsers,
  } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

afterEach(() => {
  useProjectStore.setState({ project: null, isDirty: false, loading: initialLoadingState });
  useUIStore.setState({ activeDialog: null, dialogData: {}, showToast: realShowToast });
});

describe('a routed file while a project load holds the screen (#576 W87a-1)', () => {
  it('a dropped .dmp opens no import and says why', async () => {
    await routeOpenedFile('/caves/CaveA.dmp');

    expect(parsers.importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(useUIStore.getState().importContext).toBeNull();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith('Wait for the project to finish opening.', TOAST_KEYS.IMPORT_IN_PROGRESS, 'warning');
  });

  it.each(NOT_A_PROJECT)('%s is refused the same way, with no project open either', async (path, parser) => {
    useProjectStore.setState({ project: null });
    await routeOpenedFile(path);

    if (parser) expect(parsers[parser]).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(showToast).toHaveBeenCalledWith('Wait for the project to finish opening.', TOAST_KEYS.IMPORT_IN_PROGRESS, 'warning');
  });

  it('a load that failed still holds the screen until it is dismissed', async () => {
    useProjectStore.setState({ loading: { ...LOADING_B, error: { message: 'Failed to open database' } } });
    await routeOpenedFile('/caves/CaveA.dmp');

    expect(parsers.importDmp).not.toHaveBeenCalled();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('once the load has ended, a dropped .dmp imports into the open project as before', async () => {
    useProjectStore.setState({ loading: initialLoadingState });
    parsers.importDmp.mockResolvedValue({ sections: [] });
    await routeOpenedFile('/caves/CaveA.dmp');

    expect(parsers.importDmp).toHaveBeenCalledWith('/caves/CaveA.dmp');
    expect(useUIStore.getState().activeDialog).toBe('dmp-selection');
    expect(showToast).not.toHaveBeenCalled();
  });

  describe('control: a .stalx is a newer switch, and the latest wins', () => {
    it('with no project open, a dropped .stalx loads at once, superseding the load in progress', async () => {
      useProjectStore.setState({ project: null });
      await routeOpenedFile('/caves/CaveC.stalx');

      expect(loadProject).toHaveBeenCalledWith('/caves/CaveC.stalx');
      expect(showToast).not.toHaveBeenCalled();
    });

    it('over the clean project A, a dropped .stalx asks to swap, as it does when no load runs', async () => {
      await routeOpenedFile('/caves/CaveC.stalx');

      expect(useUIStore.getState().activeDialog).toBe('launch-open-confirm');
      expect(useUIStore.getState().dialogData).toMatchObject({ path: '/caves/CaveC.stalx' });
      expect(showToast).not.toHaveBeenCalled();
    });
  });

  it('the import-as-new re-entry still parses: it continues a flow already on screen', async () => {
    useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData: { path: '/caves/CaveA.tml' } });
    const closeCurrent = vi.fn().mockResolvedValue({ success: true });
    await routeOpenedFile('/caves/CaveA.tml', 'Import Failed', { importAsNew: { closeCurrent } });

    expect(parsers.parseTml).toHaveBeenCalledWith('/caves/CaveA.tml');
    expect(showToast).not.toHaveBeenCalled();
  });
});
