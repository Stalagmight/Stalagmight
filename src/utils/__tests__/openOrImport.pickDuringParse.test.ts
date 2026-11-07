/**
 * #576 W62-NF1: while an import's file is still parsing, the file-picker doors
 * refuse up front - the in-progress toast, and no picker.
 *
 * The router refuses a second file while a parse is in flight (one import at a
 * time, openOrImport.parseInFlight.test.ts), but Ctrl+O, Ctrl+I and the Open /
 * Import button opened the native picker anyway, and the refusal arrived only
 * after the user had chosen a file, which was then dropped.
 *
 * Every door is driven through its real entry point down to the real
 * `pickAndRoute`; only the native picker (`@tauri-apps/plugin-dialog`'s `open`)
 * is a spy, and the in-flight parse is a real `routeOpenedFile` held on its
 * parser.
 */
import { describe, it, expect, beforeEach, afterEach, vi, type Mock } from 'vitest';

const openMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: (...args: unknown[]) => openMock(...args) }));

import { openOrImport, pickAndRoute, routeOpenedFile } from '../openOrImport';
import { runFileShortcutAction } from '../fileShortcutActions';
import { importMenuItem, leadingMenuItems } from '../../components/common/appMenuItems';
import { dmpImportFilters } from '../fileDialogs';
import { TOAST_KEYS } from '../toastKeys';
import { setProjectActionSource, peekProjectActionSource } from '../telemetry';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { deferred, nextTimer, type Deferred } from '../../test/factories/async';

const t = ((key: string) => key) as unknown as Parameters<typeof importMenuItem>[0];

/** The import already running: a DMP whose parse the test holds. */
const FIRST = '/cave/CaveA.dmp';
/** What the picker would answer, had it been opened. */
const PICKED = '/cave/CaveB.dat';

let held: Deferred<unknown>;
let first: Promise<void>;
const importDmp = vi.fn();
const parseCompass = vi.fn();
const realShowToast = useUIStore.getState().showToast;
const showToast = vi.fn();

/** Settles when `spy` is next called, which `impl` answers. */
function nextCallOf(spy: Mock, impl: (...args: unknown[]) => unknown): Promise<void> {
  return new Promise<void>((resolve) => {
    spy.mockImplementationOnce((...args: unknown[]) => {
      resolve();
      return impl(...args);
    });
  });
}

/** Each door, run the way its UI runs it (source first, then the door). */
const DOORS: Array<[door: string, run: () => Promise<void> | void]> = [
  ['Ctrl+O', () => runFileShortcutAction('open_file')],
  ['Ctrl+I', () => runFileShortcutAction('import_dmp')],
  [
    'the Open / Import button',
    () => {
      setProjectActionSource('toolbar');
      return openOrImport();
    },
  ],
  [
    'the in-project Import button',
    () => {
      setProjectActionSource('toolbar');
      return pickAndRoute(dmpImportFilters(), 'Import Failed');
    },
  ],
  [
    'the hamburger Open / Import row',
    () => {
      const row = leadingMenuItems(t, { onNewProject: () => {} }, 'hamburger').find(
        (item) => item.id === 'global-open-project-button',
      );
      (row?.onClick as () => void)();
    },
  ],
  ['the hamburger Import row', () => (importMenuItem(t).onClick as () => void)()],
];

beforeEach(async () => {
  vi.clearAllMocks();
  openMock.mockResolvedValue(PICKED);
  held = deferred<unknown>();
  importDmp.mockImplementation(() => held.promise);
  parseCompass.mockImplementation(() => new Promise(() => {}));
  useUIStore.setState({
    activeDialog: null,
    dialogData: {},
    importContext: null,
    placementMode: null,
    importParse: null,
    showToast,
  });
  useProjectStore.setState({ project: null, isDirty: false, importDmp, parseCompass } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });

  // The import already running, with nothing on screen yet.
  first = routeOpenedFile(FIRST);
  await nextTimer();
  expect(useUIStore.getState().importParse?.path).toBe(FIRST);
});

afterEach(async () => {
  held.resolve(null);
  await first;
  useUIStore.setState({ activeDialog: null, dialogData: {}, importParse: null, showToast: realShowToast });
});

describe('the file-picker doors while an import parses (#576 W62-NF1)', () => {
  it.each(DOORS)('%s: the in-progress toast now, and no picker', async (_door, run) => {
    await run();
    await nextTimer();

    expect(openMock).not.toHaveBeenCalled();
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      'Finish or cancel the current import first.',
      TOAST_KEYS.IMPORT_IN_PROGRESS,
      'warning',
    );
    // Nothing was handed on: the door's entry-point source is dropped, and the
    // running import is untouched.
    expect(peekProjectActionSource()).toBeUndefined();
    expect(parseCompass).not.toHaveBeenCalled();
    expect(useUIStore.getState().importParse?.path).toBe(FIRST);
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  it('once the parse has settled, the same door opens the picker again', async () => {
    held.resolve(null);
    await first;
    expect(useUIStore.getState().importParse).toBeNull();

    // [#576 W71-NF7] Wait for the picker to open and the picked file to reach its parser,
    // not for a count of turns: `openDialog` reaches the picker through a dynamic import
    // (fileDialogs.ts), the file's first of that module, and under a loaded worker one
    // timer turn was not always enough. Same answers as `beforeEach` gives.
    const opened = nextCallOf(openMock, async () => PICKED);
    const parsing = nextCallOf(parseCompass, () => new Promise(() => {}));
    runFileShortcutAction('open_file');
    await opened;
    await parsing;

    expect(openMock).toHaveBeenCalledTimes(1);
    expect(parseCompass).toHaveBeenCalledWith(PICKED);
    expect(showToast).not.toHaveBeenCalled();
  });
});
