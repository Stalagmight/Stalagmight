/**
 * #576 Q-W27-1-D1 "one import at a time, extended to the parse".
 *
 * `routeOpenedFile` parses a file before it opens the flow's first dialog, and
 * with no project open nothing at all is on screen meanwhile. The #411 gate
 * only saw dialogs and placement, so a second file routed during that parse was
 * accepted: on the farm (app-idf-gen seed 10073) a slow malformed DMP's parse
 * error landed AFTER a `.dat` routed behind it had opened its new-project
 * dialog, and replaced it; a late GOOD parse did the same with its own dialog,
 * so the user named a project for a file other than the one on screen.
 *
 * Now the parse is marked in flight (`importParse`, uiStore), the gate counts
 * it, and the second file gets the in-progress toast while the first finishes.
 * A generation token checked in `stillWanted` is the backstop for a route that
 * gets past the gate anyway: a superseded parse's late result is dropped.
 *
 * Every parse is held on a deferred promise so the test decides when it lands.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }));

import { routeOpenedFile } from '../openOrImport';
import { TOAST_KEYS } from '../toastKeys';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { deferred, type Deferred } from '../../test/factories/async';

type Parser = 'parseWalls' | 'parseTml' | 'parseCompass' | 'parseGpx' | 'importDmp';

/** Each parsing format: its path, its parser, and what that parser answers. */
const FIRST: Array<[path: string, parser: Parser, parsed: Record<string, unknown>]> = [
  ['/cave/CaveA.dmp', 'importDmp', { sections: [] }],
  ['/cave/CaveA.tml', 'parseTml', { cave_name: 'CaveA', sections: [] }],
  ['/cave/CaveA.dat', 'parseCompass', { cave_name: 'CaveA', survey_count: 0 }],
  ['/cave/CaveA.srv', 'parseWalls', { project_name: 'CaveA' }],
  ['/cave/CaveA.gpx', 'parseGpx', { name: 'CaveA', waypoints: [] }],
];
/** The file routed second, behind the first one's parse. */
const SECOND = '/cave/CaveB.dat';

/** One held parse per call, in call order. */
let held: Array<Deferred<unknown>>;
const parsers = {
  parseWalls: vi.fn(),
  parseTml: vi.fn(),
  parseCompass: vi.fn(),
  parseGpx: vi.fn(),
  importDmp: vi.fn(),
};
const realShowToast = useUIStore.getState().showToast;
const showToast = vi.fn();

/** Let the router's awaits after a settled parse run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

function dialogFilePath(): unknown {
  return (useUIStore.getState().dialogData as { filePath?: unknown }).filePath;
}

beforeEach(() => {
  vi.clearAllMocks();
  held = [];
  for (const parser of Object.values(parsers)) {
    parser.mockImplementation(() => {
      const d = deferred<unknown>();
      held.push(d);
      return d.promise;
    });
  }
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    importContext: null,
    placementMode: null,
    importParse: null,
    showToast,
  });
  useProjectStore.setState({ project: null, isDirty: false, ...parsers } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
});

afterEach(() => {
  useProjectStore.setState({ project: null, isDirty: false });
  useUIStore.setState({ activeDialog: null, dialogData: {}, importParse: null, showToast: realShowToast });
});

describe('one import at a time, extended to the parse (#576 Q-W27-1-D1)', () => {
  it.each(FIRST)(
    '%s parsing: a second file gets the in-progress toast and no dialog; the first file then fails on its own screen',
    async (path) => {
      const first = routeOpenedFile(path);
      await settle();

      // Not awaited: on a router without the gate the second parse would be held
      // forever, and the assertions below say what that did instead.
      void routeOpenedFile(SECOND);
      await settle();

      expect.soft(showToast).toHaveBeenCalledWith(expect.any(String), TOAST_KEYS.IMPORT_IN_PROGRESS, 'warning');
      expect.soft(parsers.parseCompass).not.toHaveBeenCalledWith(SECOND);
      expect.soft(useUIStore.getState().activeDialog).toBeNull();

      held[0]!.reject(new Error('No valid sections found'));
      await first;

      expect(useUIStore.getState().activeDialog).toBe('error');
      expect(useUIStore.getState().dialogData).toMatchObject({ message: 'No valid sections found' });
    },
  );

  it.each(FIRST)(
    '%s parsing: a second file is refused, and the first good parse opens the dialog for the first file',
    async (path, _parser, parsed) => {
      const first = routeOpenedFile(path);
      await settle();
      void routeOpenedFile(SECOND);
      await settle();

      held[0]!.resolve(parsed);
      await first;

      expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
      expect(dialogFilePath()).toBe(path);
      expect(held).toHaveLength(1);
    },
  );

  it('the seed-10073 shape: the DMP error never replaces a dialog of the file routed after it', async () => {
    const dmp = routeOpenedFile('/cave/CaveA.dmp');
    await settle();
    void routeOpenedFile(SECOND);
    await settle();
    // The second file's parse (had it run) would have landed first.
    held[1]?.resolve({ cave_name: 'CaveB', survey_count: 0 });
    await settle();
    expect.soft(useUIStore.getState().activeDialog).toBeNull();

    held[0]!.reject(new Error('Invalid direction value'));
    await dmp;

    expect(useUIStore.getState().activeDialog).toBe('error');
    expect(useUIStore.getState().importContext).toBeNull();
  });
});

describe('the generation token backstop (#576 Q-W27-1-D1)', () => {
  // A route that gets past the gate while an earlier parse still runs: here the
  // in-flight mark is dropped from the outside, the way any future path that
  // clears it or skips the gate would let a second route through.
  // Answers the first route still running, boxed: an async function returning a
  // bare promise would wait for it.
  async function routePastTheGate(firstPath: string): Promise<{ first: Promise<void> }> {
    const first = routeOpenedFile(firstPath);
    await settle();
    useUIStore.setState({ importParse: null });
    const second = routeOpenedFile(SECOND);
    await settle();
    held[1]!.resolve({ cave_name: 'CaveB', survey_count: 0 });
    await second;
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(dialogFilePath()).toBe(SECOND);
    return { first };
  }

  it.each(FIRST)('%s: a superseded parse that fails late does not replace the newer dialog', async (path) => {
    const { first } = await routePastTheGate(path);

    held[0]!.reject(new Error('No valid sections found'));
    await first;

    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(dialogFilePath()).toBe(SECOND);
  });

  it.each(FIRST)('%s: a superseded parse that succeeds late does not replace the newer dialog', async (path, _p, parsed) => {
    const { first } = await routePastTheGate(path);

    held[0]!.resolve(parsed);
    await first;

    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(dialogFilePath()).toBe(SECOND);
  });

  it('a superseded parse settling does not clear the newer route\'s in-flight mark', async () => {
    const first = routeOpenedFile('/cave/CaveA.dmp');
    await settle();
    useUIStore.setState({ importParse: null });
    const second = routeOpenedFile(SECOND);
    await settle();
    const newerMark = useUIStore.getState().importParse;
    expect(newerMark?.path).toBe(SECOND);

    held[0]!.reject(new Error('late'));
    await first;
    expect(useUIStore.getState().importParse).toBe(newerMark);

    held[1]!.resolve(null);
    await second;
    expect(useUIStore.getState().importParse).toBeNull();
  });
});

describe('the in-flight mark is set for the parse and always cleared (#576 Q-W27-1-D1)', () => {
  it.each(FIRST)('%s: marked while the parse runs, cleared on success', async (path, _p, parsed) => {
    const run = routeOpenedFile(path);
    await settle();
    expect(useUIStore.getState().importParse?.path).toBe(path);

    held[0]!.resolve(parsed);
    await run;
    expect(useUIStore.getState().importParse).toBeNull();
  });

  it.each(FIRST)('%s: cleared on a failed parse', async (path) => {
    const run = routeOpenedFile(path);
    await settle();
    expect(useUIStore.getState().importParse?.path).toBe(path);

    held[0]!.reject(new Error('broken'));
    await run;
    expect(useUIStore.getState().importParse).toBeNull();
    expect(useUIStore.getState().activeDialog).toBe('error');
  });

  it.each(FIRST)('%s: cleared when the parse answers nothing (the quiet abandon)', async (path) => {
    const run = routeOpenedFile(path);
    await settle();
    held[0]!.resolve(null);
    await run;
    expect(useUIStore.getState().importParse).toBeNull();
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('cleared when the route throws after the parse (the close it hands over to fails)', async () => {
    useProjectStore.setState({ project: { name: 'ProjectA' } as never });
    useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData: { path: '/cave/CaveA.tml' } });
    const closeCurrent = vi.fn(() => Promise.reject(new Error('close failed')));
    const run = routeOpenedFile('/cave/CaveA.tml', 'Open Failed', { importAsNew: { closeCurrent } });
    await settle();
    expect(useUIStore.getState().importParse?.path).toBe('/cave/CaveA.tml');

    held[0]!.resolve({ cave_name: 'CaveA', sections: [] });
    await run;
    expect(closeCurrent).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().importParse).toBeNull();
    expect(useUIStore.getState().activeDialog).toBe('error');
  });

  it('never set by a route that parses nothing, and a later file routes normally', async () => {
    // A project is open, so a TML opens the import-as-new confirm and returns.
    useProjectStore.setState({ project: { name: 'ProjectA' } as never });
    await routeOpenedFile('/cave/CaveA.tml');
    expect(useUIStore.getState().importParse).toBeNull();
    expect(useUIStore.getState().activeDialog).toBe('import-as-new-project');

    // An unsupported file: the error dialog, no mark.
    useUIStore.setState({ activeDialog: null, dialogData: {} });
    await routeOpenedFile('/cave/notes.xyz');
    expect(useUIStore.getState().importParse).toBeNull();
    expect(useUIStore.getState().activeDialog).toBe('error');
  });
});
