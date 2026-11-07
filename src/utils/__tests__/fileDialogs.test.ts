import { afterEach, beforeEach, describe, it, expect } from 'vitest';
import {
  ensureExtension,
  projectSaveFilters,
  openOrImportFilters,
  exportSaveFilters,
  PROJECT_EXTENSION,
  DMP_EXTENSION,
  COMPASS_EXTENSIONS,
  IMPORTABLE_EXTENSIONS,
  DROPPABLE_EXTENSIONS,
  isDroppableFilePath,
  KNOWN_FILE_EXTENSIONS,
  openDialog,
  saveDialog,
  takeTestFileDialogAnswer,
  testFileDialogHooksEnabled,
} from '../fileDialogs';

// ---------------------------------------------------------------------------
// ensureExtension
// ---------------------------------------------------------------------------

describe('ensureExtension', () => {
  it('appends extension when no extension present', () => {
    expect(ensureExtension('myfile', 'stalx')).toBe('myfile.stalx');
  });

  it('appends extension with leading dot normalized', () => {
    expect(ensureExtension('myfile', '.stalx')).toBe('myfile.stalx');
  });

  it('returns path unchanged when already has correct extension', () => {
    expect(ensureExtension('myfile.stalx', 'stalx')).toBe('myfile.stalx');
  });

  it('handles case-insensitive extension matching', () => {
    expect(ensureExtension('myfile.STALX', 'stalx')).toBe('myfile.STALX');
    expect(ensureExtension('myfile.Stalx', 'stalx')).toBe('myfile.Stalx');
  });

  it('replaces an extension that names another known file type', () => {
    expect(ensureExtension('myfile.csv', 'json')).toBe('myfile.json');
    expect(ensureExtension('myfile.tml', 'stalx')).toBe('myfile.stalx');
  });

  it('handles path with directory separators', () => {
    expect(ensureExtension('/home/user/myfile', 'stalx')).toBe('/home/user/myfile.stalx');
    expect(ensureExtension('/home/user/myfile.gpx', 'stalx')).toBe('/home/user/myfile.stalx');
  });

  it('handles Windows-style path separators', () => {
    expect(ensureExtension('C:\\Users\\test\\file', 'stalx')).toBe('C:\\Users\\test\\file.stalx');
    expect(ensureExtension('C:\\Users\\test\\file.dmp', 'stalx')).toBe('C:\\Users\\test\\file.stalx');
  });

  it('handles dot in directory name but no file extension', () => {
    // The dot in "v1.0" is in a directory, not an extension
    expect(ensureExtension('/home/v1.0/myfile', 'stalx')).toBe('/home/v1.0/myfile.stalx');
  });

  it('handles file with multiple dots', () => {
    expect(ensureExtension('my.cave.project.csv', 'stalx')).toBe('my.cave.project.stalx');
  });

  // [#576 D35-D1] "Replace known, else append": only a trailing extension that
  // names a file type Stalagmight reads or writes is a mistyped format; anything
  // else is part of the name. `CaveA 1.5` used to lose its tail (`CaveA 1.csv`)
  // and could land on a different existing file.
  it.each([
    ['map.svg', 'png', 'map.png'],
    ['map.PNG', 'svg', 'map.svg'],
    ['/mock/CaveA 1.5', 'csv', '/mock/CaveA 1.5.csv'],
    ['/mock/CaveA 1.5', 'stalx', '/mock/CaveA 1.5.stalx'],
    ['CaveA.STALX', 'stalx', 'CaveA.STALX'],
    ['cave', 'tml', 'cave.tml'],
    ['cave.tmlu', 'tml', 'cave.tml'],
    ['plan.plnx', 'xml', 'plan.xml'],
    ['notes.txt', 'stalx', 'notes.txt.stalx'],
    ['Survey v2.final', 'json', 'Survey v2.final.json'],
    ['/home/diver/.hidden', 'csv', '/home/diver/.hidden.csv'],
    ['/home/diver/.svg', 'png', '/home/diver/.svg.png'],
    ['CaveA.', 'csv', 'CaveA.csv'],
  ])('%s as .%s -> %s', (typed, ext, written) => {
    expect(ensureExtension(typed, ext)).toBe(written);
  });

  // [#576 NF-W2FE-3] On Linux and macOS `\` is an ordinary file-name character
  // (utils/path.ts isWindowsPath): only a Windows-shaped path splits on it. A
  // POSIX name whose last dot follows a backslash used to read as a leading-dot
  // name, so its known extension was appended to instead of replaced.
  it.each([
    ['/home/diver/CaveA\\.svg', 'png', '/home/diver/CaveA\\.png'],
    ['/home/diver/CaveA\\.', 'csv', '/home/diver/CaveA\\.csv'],
    ['/home/diver/a\\b.tml', 'stalx', '/home/diver/a\\b.stalx'],
    ['C:\\caves\\.svg', 'png', 'C:\\caves\\.svg.png'],
    ['\\\\srv\\share\\.hidden', 'csv', '\\\\srv\\share\\.hidden.csv'],
  ])('splits %s on the platform separator only: as .%s -> %s', (typed, ext, written) => {
    expect(ensureExtension(typed, ext)).toBe(written);
  });

  it('knows every format the app opens, imports or exports', () => {
    for (const ext of [PROJECT_EXTENSION, ...IMPORTABLE_EXTENSIONS, 'csv', 'json', 'tml', 'tmlu', 'png', 'svg', 'pdf', 'xml', 'plnx']) {
      expect(KNOWN_FILE_EXTENSIONS).toContain(ext);
    }
  });
});

// ---------------------------------------------------------------------------
// Filter functions
// ---------------------------------------------------------------------------

describe('filter functions', () => {
  it('projectSaveFilters includes only project extension', () => {
    const filters = projectSaveFilters();
    expect(filters).toHaveLength(1);
    expect(filters[0]!.extensions).toContain(PROJECT_EXTENSION);
  });

  // One door (#407): openOrImportFilters is the ONLY open/import picker filter
  // set - every entry point (toolbar, hamburger Open and Import, Ctrl+O,
  // Ctrl+I, EmptyState) offers it. These pins guard the properties that make
  // that safe and complete.
  it('openOrImportFilters: the combined entry accepts the project extension and every importable format', () => {
    const combined = openOrImportFilters()[0]!.extensions;
    expect(combined).toContain(PROJECT_EXTENSION);
    for (const ext of IMPORTABLE_EXTENSIONS) {
      expect(combined).toContain(ext);
    }
    // Derived from the same source of truth the router dispatches on, so a new
    // format added to IMPORT_EXTENSIONS is automatically pickable.
    expect(combined).toEqual([PROJECT_EXTENSION, ...IMPORTABLE_EXTENSIONS]);
  });

  it('openOrImportFilters offers a per-format narrowing entry for each import family, plus "All Files"', () => {
    const filters = openOrImportFilters();
    for (const name of ['Stalagmight Project', 'mNemo DMP', 'Ariane Survey', 'Compass Survey', 'Walls Survey', 'GPX Files']) {
      expect(filters.some((f) => f.name === name)).toBe(true);
    }
    // "All Files" stays: the router rejects unsupported extensions with an
    // error dialog instead of feeding them to a parser, so it is safe here.
    expect(filters.some((f) => f.extensions.includes('*'))).toBe(true);
  });

  it('exportSaveFilters creates filter with label and normalized extension', () => {
    const filters = exportSaveFilters('PNG Image', '.png');
    expect(filters).toHaveLength(1);
    expect(filters[0]!.name).toBe('PNG Image');
    expect(filters[0]!.extensions).toContain('png');
  });

  it('exportSaveFilters handles extension without leading dot', () => {
    const filters = exportSaveFilters('CSV', 'csv');
    expect(filters[0]!.extensions).toContain('csv');
  });

  it('constants are correct', () => {
    expect(PROJECT_EXTENSION).toBe('stalx');
    expect(DMP_EXTENSION).toBe('dmp');
    expect(COMPASS_EXTENSIONS).toEqual(['mak', 'dat']);
  });
});

describe('E2E file dialog answers', () => {
  const originalWindow = globalThis.window;

  beforeEach(() => {
    Object.defineProperty(globalThis, 'window', {
      configurable: true,
      writable: true,
      value: { __E2E_TEST__: { fileDialogAnswers: { open: [], save: [] }, fileDialogConsumptions: {} } },
    });
  });

  afterEach(() => {
    if (originalWindow === undefined) {
      Reflect.deleteProperty(globalThis, 'window');
    } else {
      Object.defineProperty(globalThis, 'window', {
        configurable: true,
        writable: true,
        value: originalWindow,
      });
    }
  });

  it('consumes queued open paths without opening a native picker', async () => {
    window.__E2E_TEST__ = {
      fileDialogAnswers: {
        open: [{ id: 'open-1', value: ['/tmp/a.dmp', '/tmp/b.dmp'], triggerTestId: 'open-button', armed: true }],
        save: [],
      },
      fileDialogConsumptions: {},
      activeFileDialogTriggerTestId: 'open-button',
    };
    await expect(openDialog({ multiple: true })).resolves.toEqual(['/tmp/a.dmp', '/tmp/b.dmp']);
    expect(window.__E2E_TEST__.fileDialogAnswers?.open).toEqual([]);
    expect(window.__E2E_TEST__.fileDialogConsumptions?.['open-1']).toBe('open');
  });

  it('consumes save paths and explicit cancellation', async () => {
    window.__E2E_TEST__ = {
      fileDialogAnswers: {
        open: [],
        save: [
          { id: 'save-1', value: '/tmp/out.stalx', triggerTestId: 'save-button', armed: true },
          { id: 'save-2', value: null, triggerTestId: 'save-button', armed: true },
        ],
      },
      fileDialogConsumptions: {},
      activeFileDialogTriggerTestId: 'save-button',
    };
    await expect(saveDialog()).resolves.toBe('/tmp/out.stalx');
    window.__E2E_TEST__.activeFileDialogTriggerTestId = 'save-button';
    await expect(saveDialog()).resolves.toBeNull();
    expect(window.__E2E_TEST__.fileDialogConsumptions).toEqual({ 'save-1': 'save', 'save-2': 'save' });
  });

  it('rejects forged hooks in production builds', () => {
    expect(testFileDialogHooksEnabled({ DEV: false, MODE: 'production' })).toBe(false);
  });

  it('does not consume an answer armed by a different trigger', async () => {
    window.__E2E_TEST__ = {
      fileDialogAnswers: {
        open: [{ id: 'open-2', value: '/tmp/wrong.dmp', triggerTestId: 'expected-button', armed: true }],
        save: [],
      },
      fileDialogConsumptions: {},
      activeFileDialogTriggerTestId: 'other-button',
    };

    expect(takeTestFileDialogAnswer('open')).toEqual({ queued: false, value: undefined });
    expect(window.__E2E_TEST__.fileDialogAnswers?.open).toHaveLength(1);
  });

  it('keeps async answers trigger-qualified until the picker wrapper consumes them', async () => {
    window.__E2E_TEST__ = {
      fileDialogAnswers: {
        open: [],
        save: [{ id: 'save-async', value: '/tmp/salvaged.json', triggerTestId: 'recovery-export-salvage', armed: true }],
      },
      fileDialogConsumptions: {},
      activeFileDialogTriggerTestId: 'recovery-export-salvage',
    };

    await Promise.resolve();
    await expect(saveDialog({}, 'recovery-export-salvage')).resolves.toBe('/tmp/salvaged.json');
    expect(window.__E2E_TEST__.fileDialogConsumptions?.['save-async']).toBe('save');
  });

  it('rejects a pending answer when a different picker route tries to consume it', () => {
    window.__E2E_TEST__ = {
      fileDialogAnswers: {
        open: [],
        save: [{ id: 'save-wrong', value: '/tmp/wrong.json', triggerTestId: 'export-button', armed: true }],
      },
      fileDialogConsumptions: {},
      activeFileDialogTriggerTestId: 'export-button',
    };

    expect(() => takeTestFileDialogAnswer('save', 'recovery-export-salvage')).toThrow(
      'cannot be consumed by recovery-export-salvage',
    );
  });
});

// ---------------------------------------------------------------------------
// isDroppableFilePath (#392/#394 F11 - dropping a .stalx must not be swallowed)
// ---------------------------------------------------------------------------

describe('isDroppableFilePath', () => {
  it('includes PROJECT_EXTENSION in DROPPABLE_EXTENSIONS alongside every importable extension', () => {
    expect(DROPPABLE_EXTENSIONS).toContain(PROJECT_EXTENSION);
    for (const ext of IMPORTABLE_EXTENSIONS) {
      expect(DROPPABLE_EXTENSIONS).toContain(ext);
    }
  });

  it('accepts a dropped .stalx path', () => {
    expect(isDroppableFilePath('/cave/project.stalx')).toBe(true);
  });

  it('accepts a dropped importable path (case-insensitive)', () => {
    expect(isDroppableFilePath('/cave/survey.DMP')).toBe(true);
  });

  it('rejects an unrecognized extension', () => {
    expect(isDroppableFilePath('/cave/notes.xyz')).toBe(false);
  });

  it('rejects a filename that merely CONTAINS the extension with no leading dot', () => {
    // Pins the `.${ext}` match in isDroppableFilePath/endsWith: a bare
    // "mystalx" filename must not be mistaken for a ".stalx" suffix.
    expect(isDroppableFilePath('/cave/mystalx')).toBe(false);
  });

  it('respects a narrower extension list when provided', () => {
    expect(isDroppableFilePath('/cave/project.stalx', [DMP_EXTENSION])).toBe(false);
  });
});
