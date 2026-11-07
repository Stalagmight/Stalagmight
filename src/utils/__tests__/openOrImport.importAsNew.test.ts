/**
 * #576 HFI-4 / decision HFI-4-D1 "parse first, then switch".
 *
 * "Import as new project" with a project open used to close the project at once
 * and only then parse the file, with nothing on screen: a large file left an
 * empty home screen for seconds, a failed parse had already thrown the project
 * away, and the new-project dialog arrived late - over a project the user had
 * opened meanwhile, too. Now the confirm dialog stays up showing the parse, the
 * project stays open while the file parses, only a successful parse closes it
 * (through the same guarded close) and opens the new-project dialog, a failed
 * parse leaves it untouched, and a result that settles after the project
 * changed (or after the confirm was cancelled) is dropped.
 *
 * Every parse is held on a deferred promise so the test decides what happens
 * while it runs. A project switch is the store's own session move
 * (`noteProjectSessionChanged`, which load / create / close call).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import { routeOpenedFile } from '../openOrImport';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { noteProjectSessionChanged } from '../../store/projectSession';
import { setProjectActionSource, peekProjectActionSource } from '../telemetry';
import { deferred, type Deferred } from '../../test/factories/async';
import { makeSpeleoLink } from '../../test/factories/speleo';

type Parser = 'parseWalls' | 'parseTml' | 'parseCompass' | 'parseGpx';

/** Every import-as-new format, with the parser it goes through and what that parser answers. */
const FORMATS: Array<[path: string, parser: Parser, importType: string, parsed: Record<string, unknown>]> = [
  ['/cave/CaveA.srv', 'parseWalls', 'walls', { project_name: 'CaveA' }],
  ['/cave/CaveA.wpj', 'parseWalls', 'walls', { project_name: 'CaveA' }],
  ['/cave/CaveA.tml', 'parseTml', 'tml', { cave_name: 'CaveA', sections: [] }],
  ['/cave/CaveA.tmlu', 'parseTml', 'tml', { cave_name: 'CaveA', sections: [] }],
  ['/cave/CaveA.dat', 'parseCompass', 'compass', { cave_name: 'CaveA', survey_count: 0 }],
  ['/cave/CaveA.mak', 'parseCompass', 'compass', { cave_name: 'CaveA', survey_count: 0 }],
  ['/cave/CaveA.gpx', 'parseGpx', 'gpx', { name: 'CaveA', waypoints: [] }],
];

let held: Deferred<unknown>;
const parsers = {
  parseWalls: vi.fn(),
  parseTml: vi.fn(),
  parseCompass: vi.fn(),
  parseGpx: vi.fn(),
  importDmp: vi.fn(),
};
/** The guarded close the dialog hands in (`useProject.handleCloseProject`): closes and moves the session. */
const closeCurrent = vi.fn(async () => {
  useProjectStore.setState({ project: null });
  noteProjectSessionChanged();
  return { success: true };
});

const PROJECT_A = { name: 'ProjectA' };
const PROJECT_B = { name: 'ProjectB' };

/** The confirm dialog as `openImportAsNewProject` opened it, over ProjectA. */
function confirmOver(path: string): Record<string, unknown> {
  const dialogData = { path, fileName: path.split('/').pop() };
  useUIStore.setState({ activeDialog: 'import-as-new-project', dialogData });
  return dialogData;
}

/** "Close and import": the router call ImportAsNewProjectDialog makes, left running. */
function confirm(path: string): Promise<void> {
  return routeOpenedFile(path, 'Open Failed', { importAsNew: { closeCurrent } });
}

/** Another project opens while the parse runs (a load: the session moves). */
function openAnotherProject(): void {
  noteProjectSessionChanged();
  useProjectStore.setState({ project: PROJECT_B as never });
}

/** Let the router's awaits after a settled parse run. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

beforeEach(() => {
  vi.clearAllMocks();
  held = deferred<unknown>();
  for (const parser of Object.values(parsers)) parser.mockImplementation(() => held.promise);
  closeCurrent.mockImplementation(async () => {
    useProjectStore.setState({ project: null });
    noteProjectSessionChanged();
    return { success: true };
  });
  useUIStore.setState({ activeDialog: null, dialogData: {}, importContext: null, placementMode: null });
  useProjectStore.setState({ project: PROJECT_A as never, isDirty: false, ...parsers } as never);
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  setProjectActionSource('keyboard');
});

afterEach(() => {
  useProjectStore.setState({ project: null, isDirty: false });
  useUIStore.setState({ activeDialog: null, dialogData: {} });
});

describe('import as new project: parse first, then switch (#576 HFI-4-D1)', () => {
  it.each(FORMATS)('%s: the project stays open, and the confirm stays up, while the file parses', async (path, parser) => {
    const dialogData = confirmOver(path);
    const run = confirm(path);
    await settle();

    expect.soft(parsers[parser]).toHaveBeenCalledWith(path);
    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    expect.soft(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect.soft(useUIStore.getState().dialogData).toBe(dialogData);

    held.resolve(null);
    await run;
  });

  it.each(FORMATS)(
    '%s: only a successful parse closes the project, then the new-project dialog opens with the parsed file',
    async (path, parser, importType, parsed) => {
      confirmOver(path);
      const run = confirm(path);
      await settle();
      expect(closeCurrent).not.toHaveBeenCalled();

      held.resolve(parsed);
      await run;

      expect.soft(closeCurrent).toHaveBeenCalledTimes(1);
      expect.soft(parsers[parser]).toHaveBeenCalledTimes(1);
      expect.soft(useProjectStore.getState().project).toBeNull();
      const ui = useUIStore.getState();
      expect.soft(ui.activeDialog).toBe('new-project-for-import');
      expect.soft(ui.dialogData).toMatchObject({ filePath: path, importType });
      expect.soft(ui.importContext).toBe('new-project');
      // Handed off: the new-project dialog owns the entry-point source now.
      expect.soft(peekProjectActionSource()).toBe('keyboard');
    },
  );

  it.each(FORMATS)('%s: a failed parse shows its error and leaves the open project untouched', async (path) => {
    confirmOver(path);
    const run = confirm(path);
    held.reject(new Error('Synthetic: unexpected end of file'));
    await run;

    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    expect.soft(useUIStore.getState().activeDialog).toBe('error');
    expect.soft(useUIStore.getState().dialogData).toMatchObject({
      message: 'Synthetic: unexpected end of file',
      hideReportHint: true,
    });
    expect.soft(peekProjectActionSource()).toBeUndefined();
  });

  it.each(FORMATS)(
    '%s: a project opened while the file parses drops the late result - no dialog over it, nothing closed',
    async (path, _parser, _type, parsed) => {
      confirmOver(path);
      const run = confirm(path);
      await settle();
      openAnotherProject();

      held.resolve(parsed);
      await run;

      expect.soft(closeCurrent).not.toHaveBeenCalled();
      expect.soft(useProjectStore.getState().project).toBe(PROJECT_B);
      // The stale confirm (it named ProjectA) is taken down, and nothing replaces it.
      expect.soft(useUIStore.getState().activeDialog).toBeNull();
      expect.soft(useUIStore.getState().importContext).toBeNull();
      expect.soft(peekProjectActionSource()).toBeUndefined();
    },
  );

  it('a project opened meanwhile also drops a late parse FAILURE: no error over the other project', async () => {
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    await settle();
    openAnotherProject();

    held.reject(new Error('Synthetic: unexpected end of file'));
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBeNull();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_B);
  });

  it.each([
    ['success', (d: Deferred<unknown>) => d.resolve({ project_name: 'CaveA' })],
    ['failure', (d: Deferred<unknown>) => d.reject(new Error('Synthetic: unexpected end of file'))],
  ] as const)('cancelling the confirm while the file parses drops the late %s', async (_kind, settleParse) => {
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    await settle();
    useUIStore.getState().closeDialog();

    settleParse(held);
    await run;

    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    expect.soft(useUIStore.getState().activeDialog).toBeNull();
  });

  it('a re-opened confirm is a different instance: the first run drops its late result', async () => {
    confirmOver('/cave/CaveA.srv');
    const first = confirm('/cave/CaveA.srv');
    await settle();
    // Cancel, then the same file picked again: a fresh confirm with a fresh `dialogData`.
    useUIStore.getState().closeDialog();
    const second = confirmOver('/cave/CaveA.srv');

    held.resolve({ project_name: 'CaveA' });
    await first;

    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect.soft(useUIStore.getState().dialogData).toBe(second);
  });

  it('a close refused after the parse ends the import: no new-project dialog, the source dropped', async () => {
    closeCurrent.mockResolvedValue({ success: false });
    const dialogData = confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    held.resolve({ project_name: 'CaveA' });
    await run;

    expect.soft(closeCurrent).toHaveBeenCalledTimes(1);
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    expect.soft(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect.soft(useUIStore.getState().dialogData).toBe(dialogData);
    expect.soft(peekProjectActionSource()).toBeUndefined();
  });

  it('without the confirm, a new-project-only file over an open project still only asks (no parse)', async () => {
    await routeOpenedFile('/cave/CaveA.srv');
    expect.soft(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect.soft(parsers.parseWalls).not.toHaveBeenCalled();
    expect.soft(closeCurrent).not.toHaveBeenCalled();
  });
});

/** A close that must ask first, and the prompt it asks with. */
const ASKING_CLOSES = [
  ['unsaved changes', 'unsaved-changes', () => useProjectStore.setState({ isDirty: true })],
  [
    'a SpeleoDB hand-back',
    'speleodb-close',
    () => useSpeleoStore.setState({ link: makeSpeleoLink({ editState: 'editing' }), projectLinkKind: 'remote' }),
  ],
] as const;

describe('import as new project: the close prompts come after a good parse (#576 HFI-4-D2)', () => {
  it.each(ASKING_CLOSES)('%s: the file parses first, with the project open and no prompt', async (_why, _prompt, arrange) => {
    arrange();
    const dialogData = confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    await settle();

    expect.soft(parsers.parseWalls).toHaveBeenCalledWith('/cave/CaveA.srv');
    expect.soft(useUIStore.getState().activeDialog).toBe('import-as-new-project');
    expect.soft(useUIStore.getState().dialogData).toBe(dialogData);
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    expect.soft(closeCurrent).not.toHaveBeenCalled();
    held.reject(new Error('stop'));
    await run;
  });

  it.each(ASKING_CLOSES)('%s: after a good parse that prompt asks, carrying the parsed file', async (_why, prompt, arrange) => {
    arrange();
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    held.resolve({ project_name: 'CaveA' });
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBe(prompt);
    expect.soft(useUIStore.getState().dialogData.pendingImport).toMatchObject({ filePath: '/cave/CaveA.srv', importType: 'walls' });
    // Nothing is closed yet: the prompt's own close does that (or its Cancel keeps it).
    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
    // The prompt owns the entry-point source now, as the .stalx open's prompt does.
    expect.soft(peekProjectActionSource()).toBe('keyboard');
  });

  it.each(FORMATS)('%s: the prompt carries exactly what the new-project dialog would have got', async (path, parser, importType, parsed) => {
    useProjectStore.setState({ isDirty: true });
    confirmOver(path);
    const run = confirm(path);
    held.resolve(parsed);
    await run;

    expect.soft(parsers[parser]).toHaveBeenCalledTimes(1);
    expect.soft(useUIStore.getState().activeDialog).toBe('unsaved-changes');
    const pending = useUIStore.getState().dialogData.pendingImport as Record<string, unknown> | undefined;
    expect.soft(pending).toMatchObject({ filePath: path, defaultName: 'CaveA', importType });
    expect.soft(Object.values(pending ?? {})).toContain(parsed);
  });

  it('a SpeleoDB hand-back asks first, ahead of unsaved changes', async () => {
    useProjectStore.setState({ isDirty: true });
    ASKING_CLOSES[1][2]();
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    held.resolve({ project_name: 'CaveA' });
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBe('speleodb-close');
    expect.soft(useUIStore.getState().dialogData.pendingImport).toMatchObject({ importType: 'walls' });
  });

  it.each(ASKING_CLOSES)('%s: a failed parse shows its error and no prompt', async (_why, _prompt, arrange) => {
    arrange();
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    held.reject(new Error('line 3: bad shot'));
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBe('error');
    expect.soft(String(useUIStore.getState().dialogData.message)).toContain('line 3: bad shot');
    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_A);
  });

  it.each(ASKING_CLOSES)('%s: a project opened during the parse still drops the result, with no prompt', async (_why, _prompt, arrange) => {
    arrange();
    confirmOver('/cave/CaveA.srv');
    const run = confirm('/cave/CaveA.srv');
    openAnotherProject();
    held.resolve({ project_name: 'CaveA' });
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBeNull();
    expect.soft(closeCurrent).not.toHaveBeenCalled();
    expect.soft(useProjectStore.getState().project).toBe(PROJECT_B);
  });
});

describe('a late parse result is dropped on every import route (#576 HFI-4-D1)', () => {
  it('a .dmp picked for ProjectA does not open its selection over ProjectB opened meanwhile', async () => {
    const run = routeOpenedFile('/cave/dive.dmp');
    await settle();
    openAnotherProject();

    held.resolve({ version: 5, sections: [] });
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBeNull();
    expect.soft(useUIStore.getState().importContext).toBeNull();
    expect.soft(peekProjectActionSource()).toBeUndefined();
  });

  it('a .dmp with no project switch still opens its selection over the same project', async () => {
    const run = routeOpenedFile('/cave/dive.dmp');
    held.resolve({ version: 5, sections: [] });
    await run;

    expect.soft(useUIStore.getState().activeDialog).toBe('dmp-selection');
    expect.soft(useUIStore.getState().importContext).toBe('existing-project');
  });

  it.each(FORMATS)(
    '%s from the home screen: a project opened while it parses gets no new-project dialog over it',
    async (path, _parser, _type, parsed) => {
      useProjectStore.setState({ project: null });
      const run = routeOpenedFile(path);
      await settle();
      openAnotherProject();

      held.resolve(parsed);
      await run;

      expect.soft(useUIStore.getState().activeDialog).toBeNull();
      expect.soft(useProjectStore.getState().project).toBe(PROJECT_B);
    },
  );
});
