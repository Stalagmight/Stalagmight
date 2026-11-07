/**
 * The pure half of `export-roundtrip-fuzz`'s oracles.
 *
 * Two of these functions are MIRRORS of app code that this tier cannot import
 * (`exportOracles.ts` states why), so the tables below are transcribed straight
 * from the source they mirror and are the whole reason the mirrors are allowed
 * to exist. A drift in `src/utils/fileDialogs.ts` or `src-tauri/src/export/csv.rs`
 * has to fail HERE, loudly, rather than turn the fuzzer into something that
 * checks a path the app never writes to.
 */
import { describe, expect, it } from 'vitest';

import {
  canonicalizeUuids,
  checkByteIdentity,
  checkConvergence,
  checkExportModeInvariants,
  checkExportStoreReset,
  checkFileResult,
  checkFormatDifferential,
  checkNoDuplicateUuids,
  checkOptionIndependence,
  checkProjectFileSurvived,
  checkShotMultiset,
  commentMultisetDelta,
  csvTextViolations,
  ensureExtensionMirror,
  escapeCsvMirror,
  exportBaseName,
  ExportOracleError,
  domRouteRewritesPath,
  finalizeExportPath,
  multisetDelta,
  naturalExtension,
  newToastKeys,
  neutralisedForm,
  parseCsvRows,
  PATH_SHAPES,
  shotKey,
  asExportedToTml,
  tmlAzimuthPlan,
  type ExportAttempt,
  type ModeSnapshot,
  type OptionSnapshot,
  type PathShape,
  type SectionAzimuthReference,
  type ShotIdentity,
  type TmlAzimuthPlan,
} from './exportOracles';

const REPRO = 'reproduce: pnpm agent:scenario export-roundtrip-fuzz --seed 1 --iterations 1';

// ---------------------------------------------------------------------------
// Path shapes
// ---------------------------------------------------------------------------

describe('exportBaseName', () => {
  it('produces a plain basename for every generatable shape', () => {
    const generatable = PATH_SHAPES.filter((s) => s !== 'openProject' && s !== 'plnxSibling');
    // Nine shapes are listed; two name a file the project owns and are supplied
    // by the scenario, so exactly eight are generatable.
    expect(generatable).toHaveLength(8);
    for (const shape of generatable) {
      const name = exportBaseName(shape, 'csv', 7);
      expect(name).not.toBe('');
      expect(name.startsWith('.')).toBe(false);
      expect(name).not.toContain('/');
      expect(name).not.toContain('\\');
      expect(name.split(/[/\\]/).includes('..')).toBe(false);
      // Under NAME_MAX on every filesystem the harness runs on.
      expect(Buffer.byteLength(name, 'utf8')).toBeLessThan(255);
    }
  });

  it('refuses to invent a name for a project-owned shape', () => {
    for (const shape of ['openProject', 'plnxSibling'] as PathShape[]) {
      expect(() => exportBaseName(shape, 'csv', 1)).toThrow(/open project owns/);
    }
  });

  it('gives each shape the property it exists to exercise', () => {
    expect(exportBaseName('normal', 'csv', 1)).toBe('export-1.csv');
    expect(exportBaseName('upperTmlu', 'tml', 2)).toBe('EXPORT-2.TMLU');
    expect(exportBaseName('doubleExtension', 'tml', 3)).toBe('export-3.tml.txt');
    expect(exportBaseName('noExtension', 'csv', 4)).toBe('export-4');
    expect(exportBaseName('trailingDot', 'csv', 5)).toBe('export-5.');
    expect(exportBaseName('unicodeRtl', 'svg', 6)).toContain('\u202e');
    expect(exportBaseName('longName', 'png', 7).length).toBeGreaterThan(190);
    expect(exportBaseName('percentQuoteSpace', 'pdf', 8)).toBe('export-8 100% "quoted".pdf');
  });

  it('keeps the long name deterministic in LENGTH regardless of the sequence number', () => {
    // The pad is computed from the stem, so a two-digit sequence must not push
    // the name past the ~200 target - otherwise a late iteration would test a
    // different thing from an early one.
    for (const seq of [1, 42, 999]) {
      expect(exportBaseName('longName', 'png', seq).length).toBeLessThanOrEqual(205);
    }
  });
});

describe('naturalExtension', () => {
  it('maps every format to what the app forces', () => {
    expect(naturalExtension('csv')).toBe('csv');
    expect(naturalExtension('json')).toBe('json');
    expect(naturalExtension('tml')).toBe('tml');
    expect(naturalExtension('tmlu')).toBe('tmlu');
    expect(naturalExtension('png')).toBe('png');
    expect(naturalExtension('svg')).toBe('svg');
    expect(naturalExtension('pdf')).toBe('pdf');
    // ProfileChart.onExport does resolveSavePath(path, 'xml') (ensureExtension underneath).
    expect(naturalExtension('diveProfile')).toBe('xml');
    expect(naturalExtension('preview')).toBe('svg');
  });
});

// ---------------------------------------------------------------------------
// Mirror 1: ensureExtension
// ---------------------------------------------------------------------------

describe('ensureExtensionMirror (src/utils/fileDialogs.ts ensureExtension)', () => {
  // Transcribed from the source: the rule is "already ends in the extension ->
  // unchanged; a file-name extension naming a KNOWN file type (or a bare
  // trailing dot) -> REPLACE from that dot; otherwise append" (#576 D35).
  const table: [string, string, string][] = [
    ['/x/out.csv', 'csv', '/x/out.csv'],
    ['/x/out.CSV', 'csv', '/x/out.CSV'],
    ['/x/out', 'csv', '/x/out.csv'],
    ['/x/out.svg', 'csv', '/x/out.csv'],
    ['/x/out.txt', 'csv', '/x/out.txt.csv'],
    ['/x/out.tml.txt', 'tml', '/x/out.tml.txt.tml'],
    ['/x/CaveA 1.5', 'csv', '/x/CaveA 1.5.csv'],
    ['/x/.hidden', 'csv', '/x/.hidden.csv'],
    ['/x/out.', 'csv', '/x/out.csv'],
    // The dot is in a DIRECTORY, not the file name: appended, not replaced.
    ['/a.b/out', 'csv', '/a.b/out.csv'],
    ['/.agent-artifacts/sessions/s1/out', 'json', '/.agent-artifacts/sessions/s1/out.json'],
    // A leading dot in the extension argument is stripped.
    ['/x/out', '.png', '/x/out.png'],
    // Windows separator.
    ['C:\\x\\out.dmp', 'svg', 'C:\\x\\out.svg'],
    ['C:\\x\\.svg', 'png', 'C:\\x\\.svg.png'],
    // On a POSIX path `\` is a file-name character, not a separator
    // (src/utils/path.ts isWindowsPath), so `CaveA\.svg` has the extension svg.
    ['/x/CaveA\\.svg', 'png', '/x/CaveA\\.png'],
    ['/x/CaveA\\.', 'csv', '/x/CaveA\\.csv'],
    // The open project's own path, which is why the modal route never reaches
    // the backend guard.
    ['/x/cave.stalx', 'csv', '/x/cave.csv'],
  ];

  it.each(table)('%s + %s -> %s', (input, extension, expected) => {
    expect(ensureExtensionMirror(input, extension)).toBe(expected);
  });

  it('replaces only a KNOWN extension - the mistake a from-memory mirror makes', () => {
    expect(ensureExtensionMirror('/x/out.png', 'csv')).toBe('/x/out.csv');
    expect(ensureExtensionMirror('/x/out.txt', 'csv')).toBe('/x/out.txt.csv');
  });
});

describe('finalizeExportPath (ExportControls.handleExport)', () => {
  it('keeps a .tml or .tmlu the user already typed', () => {
    expect(finalizeExportPath('/x/out.tmlu', 'tml')).toBe('/x/out.tmlu');
    expect(finalizeExportPath('/x/OUT.TMLU', 'tml')).toBe('/x/OUT.TMLU');
    expect(finalizeExportPath('/x/out.tml', 'tml')).toBe('/x/out.tml');
  });

  it('forces .tml when the typed path has any other extension', () => {
    expect(finalizeExportPath('/x/out.tml.csv', 'tml')).toBe('/x/out.tml.tml');
    expect(finalizeExportPath('/x/out.tml.txt', 'tml')).toBe('/x/out.tml.txt.tml');
    expect(finalizeExportPath('/x/out', 'tml')).toBe('/x/out.tml');
    expect(finalizeExportPath('/x/out.', 'tml')).toBe('/x/out.tml');
  });

  it('forces .tml for a tmlu DRAW too - the Format select has no tmlu option', () => {
    // `.tmlu` is reachable through the UI only by typing it into the save
    // dialog, which is what the `upperTmlu` path shape does. A tmlu draw at any
    // other shape lands on `.tml`, and predicting `.tmlu` made X1 call every
    // such export a silent failure.
    expect(finalizeExportPath('/x/out', 'tmlu')).toBe('/x/out.tml');
    expect(finalizeExportPath('/x/out.json', 'tmlu')).toBe('/x/out.tml');
    expect(finalizeExportPath('/x/OUT.TMLU', 'tmlu')).toBe('/x/OUT.TMLU');
  });

  it('forces the data format for csv/json and the image format for png/svg/pdf', () => {
    expect(finalizeExportPath('/x/out.svg', 'csv')).toBe('/x/out.csv');
    expect(finalizeExportPath('/x/out.csv', 'json')).toBe('/x/out.json');
    expect(finalizeExportPath('/x/out.pdf', 'png')).toBe('/x/out.png');
    expect(finalizeExportPath('/x/out.png', 'pdf')).toBe('/x/out.pdf');
    // An extension that names no file type the app knows is part of the name.
    expect(finalizeExportPath('/x/out.txt', 'csv')).toBe('/x/out.txt.csv');
  });

  it('REDIRECTS an export aimed at the open project, which is why X2 is per-route', () => {
    // The UI never reaches the backend's refusal for these: the extension
    // forcing has already turned the path into a different file.
    expect(finalizeExportPath('/x/cave.stalx', 'csv')).toBe('/x/cave.csv');
    expect(finalizeExportPath('/x/cave.plnx', 'png')).toBe('/x/cave.png');
    // ... except for TML, where `.stalx` is neither .tml nor .tmlu, so it is
    // still rewritten. There is no shape where the modal hands the raw path on.
    expect(finalizeExportPath('/x/cave.stalx', 'tml')).toBe('/x/cave.tml');
  });
});

// #576 NF-W2FE-2: the Replace File alert (overwriteGuard.resolveSavePath) can only
// follow a REWRITTEN answer, and the scenario answers it in clickAndAnswerSave.
describe('domRouteRewritesPath (when the Replace File alert can appear)', () => {
  it('is true for every format on the project-owned shapes - the .plnx probe can always meet the alert', () => {
    for (const format of ['csv', 'json', 'tml', 'png', 'svg', 'pdf'] as const) {
      expect(domRouteRewritesPath('/x/cave.plnx', format), `plnx ${format}`).toBe(true);
      expect(domRouteRewritesPath('/x/cave.stalx', format), `stalx ${format}`).toBe(true);
    }
  });

  it('is true for an appended extension and false for a name the app keeps', () => {
    expect(domRouteRewritesPath('/x/out.tml.txt', 'tml')).toBe(true);
    expect(domRouteRewritesPath('/x/export-1.csv', 'csv')).toBe(false);
    expect(domRouteRewritesPath('/x/export-1.tmlu', 'tml')).toBe(false);
    expect(domRouteRewritesPath('/x/NAME.TMLU', 'tmlu')).toBe(false);
  });
});

// ---------------------------------------------------------------------------
// Mirror 2: escape_csv
// ---------------------------------------------------------------------------

describe('escapeCsvMirror (src-tauri/src/export/csv.rs:66-79)', () => {
  const table: [string, string][] = [
    ['', ''],
    ['plain', 'plain'],
    // Formula injection: neutralised with a leading apostrophe, and NOT quoted
    // when nothing in the quote-trigger set is present.
    ["=cmd|' /C calc'!A0", "'=cmd|' /C calc'!A0"],
    ['+1+1', "'+1+1"],
    ['-2+3', "'-2+3"],
    ['@SUM(1)', "'@SUM(1)"],
    // A leading TAB is in the neutralisation set and in NO quote-trigger set,
    // so it comes out neutralised and unquoted. Easy to get wrong.
    ['\tleading tab', "'\tleading tab"],
    // A leading CR is in BOTH sets: neutralised, then quoted.
    ['\rleading carriage return', '"\'\rleading carriage return"'],
    // A leading LF is in neither neutralisation set but IS a quote trigger.
    ['\nleading newline', '"\nleading newline"'],
    // Quoting and doubling.
    ['a,b', '"a,b"'],
    ['say "hi"', '"say ""hi"""'],
    ['line1\nline2', '"line1\nline2"'],
    // Neutralise FIRST: the apostrophe is INSIDE the quotes.
    ['=a,b', '"\'=a,b"'],
    // A formula leader that is not FIRST is untouched.
    ['x=1', 'x=1'],
    // Non-ASCII passes through.
    ['\u6d1e\u7a74', '\u6d1e\u7a74'],
    ['part one\u200bpart two', 'part one\u200bpart two'],
  ];

  it.each(table)('%j -> %j', (input, expected) => {
    expect(escapeCsvMirror(input)).toBe(expected);
  });

  it('neutralises before quoting, never after', () => {
    // The failure mode the Rust comment calls out: an apostrophe stranded
    // OUTSIDE the quotes, which a spreadsheet would then not honour.
    expect(escapeCsvMirror('=a,b')).not.toBe('\'"=a,b"');
  });
});

describe('parseCsvRows', () => {
  it('reads quoted fields containing separators and newlines', () => {
    const rows = parseCsvRows('a,b\n"x,y","p\nq"\n');
    expect(rows).toEqual([
      ['a', 'b'],
      ['x,y', 'p\nq'],
    ]);
  });

  it('unescapes doubled quotes', () => {
    expect(parseCsvRows('"say ""hi"""\n')).toEqual([['say "hi"']]);
  });

  it('accepts CRLF and LF row separators but not a bare CR', () => {
    expect(parseCsvRows('a\r\nb\n')).toEqual([['a'], ['b']]);
    // `writeln!` emits LF, so a bare CR inside a record is DATA. Splitting on
    // it would tear a row the exporter meant to keep whole.
    expect(parseCsvRows('a\rb\n')).toEqual([['a\rb']]);
  });

  it('round-trips everything escapeCsvMirror can produce', () => {
    const values = [
      'plain',
      '=cmd',
      'a,b',
      'say "hi"',
      'line1\nline2',
      '\tleading tab',
      '',
      '\u202eoverridden',
    ];
    const line = values.map(escapeCsvMirror).join(',');
    expect(parseCsvRows(`${line}\n`)[0]).toEqual(values.map(neutralisedForm));
  });
});

describe('csvTextViolations', () => {
  const header =
    'section_id,section_name,shot_id,length,azimuth_in,azimuth_out,depth_in,depth_out,left,right,up,down,kind,comment';
  const row = (name: string, comment: string): string =>
    [
      '1',
      escapeCsvMirror(name),
      '1',
      '1.00',
      '0.0',
      '0.0',
      '0.00',
      '0.00',
      '',
      '',
      '',
      '',
      'Permanent',
      escapeCsvMirror(comment),
    ].join(',');

  it('accepts a correctly escaped file and finds every seeded comment', () => {
    const hostile = ["=cmd|' /C calc'!A0", 'a,b', 'say "hi"', '\tleading tab', 'line1\nline2'];
    const text = `${header}\n${hostile.map((c) => row('Cueva "Nueva", 1', c)).join('\n')}\n`;
    const result = csvTextViolations(text, hostile);
    expect(result.violations).toEqual([]);
    expect(result.missing).toEqual([]);
    expect(result.rows).toBe(hostile.length + 1);
  });

  it('reports an UN-NEUTRALISED formula cell', () => {
    // What a regression in escape_csv would produce: the raw `=` reaches the
    // file, and the next spreadsheet to open it evaluates the cell.
    const text = `${header}\n1,Sec,1,1.00,0.0,0.0,0.00,0.00,,,,,Permanent,=cmd\n`;
    const result = csvTextViolations(text, ['=cmd']);
    // The cell parses as `=cmd`, which escape_csv would have written as `'=cmd`.
    expect(result.violations.map((v) => v.column)).toContain('comment');
    // And the seeded comment is reported missing, because its written form
    // (neutralised) is not present.
    expect(result.missing).toEqual(['=cmd']);
  });

  it('reports a row torn by an unquoted separator', () => {
    const text = `${header}\n1,Sec,1,1.00,0.0,0.0,0.00,0.00,,,,,Permanent,a,b\n`;
    const result = csvTextViolations(text, []);
    // 15 fields against a 14-column header. Note every surviving cell here is
    // individually well-formed (`a` needs no escaping), which is why the field
    // COUNT has to be its own check rather than a consequence of the per-cell
    // shape check.
    expect(parseCsvRows(text)[1]).toHaveLength(15);
    expect(result.violations.map((v) => v.column)).toContain('(record)');
  });

  it('reports a comment that did not survive at all', () => {
    const text = `${header}\n1,Sec,1,1.00,0.0,0.0,0.00,0.00,,,,,Permanent,\n`;
    expect(csvTextViolations(text, ['important note']).missing).toEqual(['important note']);
  });
});

// ---------------------------------------------------------------------------
// Canonicalization
// ---------------------------------------------------------------------------

describe('canonicalizeUuids', () => {
  const A = '11111111-2222-4333-8444-555555555555';
  const B = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';

  it('replaces uuids positionally and keeps repeats consistent', () => {
    expect(canonicalizeUuids(`<a>${A}</a><b>${B}</b><c>${A}</c>`)).toBe(
      '<a><uuid-0></a><b><uuid-1></b><c><uuid-0></c>',
    );
  });

  it('keeps the REPEAT STRUCTURE, which a single constant token would flatten', () => {
    // One uuid on two shots vs two distinct uuids is exactly the difference
    // `remint_duplicate_uuids` makes, so it must survive canonicalization.
    expect(canonicalizeUuids(`${A}|${B}|${A}`)).not.toBe(canonicalizeUuids(`${A}|${B}|${B}`));
  });

  it('does NOT distinguish a bare permutation, and that is stated rather than hidden', () => {
    // Shots carry their geometry with them, so a real reordering moves every
    // Length/Azimuth/Depth around it and the comparison sees those instead.
    expect(canonicalizeUuids(`${A}${B}`)).toBe(canonicalizeUuids(`${B}${A}`));
  });

  it('leaves everything that is not uuid-shaped alone', () => {
    const xml = '<Length>12.5</Length><Comment>not-a-uuid-1234</Comment>';
    expect(canonicalizeUuids(xml)).toBe(xml);
  });
});

// ---------------------------------------------------------------------------
// The throwing oracles
// ---------------------------------------------------------------------------

const attempt = (over: Partial<ExportAttempt> = {}): ExportAttempt => ({
  format: 'csv',
  route: 'modal',
  shape: 'normal',
  requested: '/s/fuzz-exports/export-1.csv',
  effective: '/s/fuzz-exports/export-1.csv',
  ok: true,
  error: null,
  size: 128,
  toastKeys: [],
  strays: [],
  ...over,
});

/**
 * The toast diff X1's classification rests on.
 *
 * `export-failed` is an error toast, and ToastStack gives an error toast a
 * 9000 ms dwell plus a 1500 ms fade - ten and a half seconds, which is several
 * CSV exports. So "is the key on screen" answers a question about the SESSION,
 * not about the move, and the move is what X1 judges.
 */
describe('newToastKeys', () => {
  const snap = (marks: string[], keys?: string[]): { marks: string[]; keys: string[] } => ({
    marks,
    keys: keys ?? [...new Set(marks.map((mark) => mark.slice(0, mark.indexOf('#'))))],
  });

  it('reports nothing when nothing changed', () => {
    const live = snap(['export-failed#7@0']);
    expect(newToastKeys(live, live)).toEqual([]);
  });

  it('IGNORES a toast that was already there - the false failure this exists for', () => {
    // Move N failed and raised the toast; move N+1 succeeded a second later and
    // used to read that toast as its own result. X1 then reported "an export
    // reported FAILURE and left a N-byte file behind" against an app that had
    // written exactly the file it was asked for.
    const before = snap(['export-failed#7@0']);
    const after = snap(['export-failed#7@0']);
    expect(newToastKeys(before, after)).not.toContain('export-failed');
  });

  it('reports a genuinely new toast', () => {
    expect(newToastKeys(snap([]), snap(['export-failed#7@0']))).toEqual(['export-failed']);
    expect(newToastKeys(snap(['export-failed#7@0']), snap(['export-failed#7@0', 'export-failed#9@0']))).toEqual([
      'export-failed',
    ]);
  });

  it('reports a REFRESH, because a repeat inside the dedup window is not a new toast', () => {
    // `showToast` collapses a repeat of the same key within
    // TOAST_DEDUP_WINDOW_MS into the toast already on screen and bumps its
    // revision. A key-set diff sees no change and would call the second failure
    // a success - the same bug pointing the other way, with X1 then demanding
    // the file the failed export correctly did not write.
    expect(newToastKeys(snap(['export-failed#7@0']), snap(['export-failed#7@1']))).toEqual(['export-failed']);
  });

  it('does not report a toast that merely went away', () => {
    expect(newToastKeys(snap(['export-failed#7@0']), snap([]))).toEqual([]);
  });

  it('picks up a DOM-only key the store no longer lists', () => {
    expect(newToastKeys({ marks: [], keys: [] }, { marks: [], keys: ['export-failed'] })).toEqual([
      'export-failed',
    ]);
  });

  it('does not double-count a key that is both in the DOM and in the store', () => {
    expect(newToastKeys(snap([]), snap(['export-failed#7@0'], ['export-failed']))).toEqual(['export-failed']);
  });

  it('keeps unrelated keys apart', () => {
    expect(
      newToastKeys(snap(['command-failed#1@0']), snap(['command-failed#1@0', 'export-failed#2@0'])),
    ).toEqual(['export-failed']);
  });
});

describe('X1 file/result agreement', () => {
  it('passes a success that wrote a non-empty file', () => {
    expect(() => checkFileResult(attempt(), REPRO)).not.toThrow();
  });

  it('fails a success with no file', () => {
    expect(() => checkFileResult(attempt({ size: null }), REPRO)).toThrow(ExportOracleError);
  });

  it('fails a success with a zero-byte file', () => {
    expect(() => checkFileResult(attempt({ size: 0 }), REPRO)).toThrow(/ZERO-BYTE/);
  });

  it('fails a failure that left an artifact behind', () => {
    expect(() =>
      checkFileResult(attempt({ ok: false, error: 'boom', size: 12, toastKeys: ['export-failed'] }), REPRO),
    ).toThrow(/left a 12-byte file/);
  });

  it('fails a UI-route failure the user was never told about', () => {
    expect(() => checkFileResult(attempt({ ok: false, error: 'boom', size: null }), REPRO)).toThrow(
      /NOTHING TOLD THE USER/,
    );
  });

  it('passes a UI-route failure that raised the export-failed toast', () => {
    expect(() =>
      checkFileResult(attempt({ ok: false, error: 'boom', size: null, toastKeys: ['export-failed'] }), REPRO),
    ).not.toThrow();
  });

  it('exempts the store route from the toast requirement', () => {
    // A rejected promise IS the report for a raw IPC caller; the surfacing
    // lives in the component.
    expect(() =>
      checkFileResult(attempt({ route: 'store', ok: false, error: 'boom', size: null }), REPRO),
    ).not.toThrow();
  });

  it('fails when a move left a file it did not name', () => {
    expect(() => checkFileResult(attempt({ strays: ['export-1.csv.part'] }), REPRO)).toThrow(/did not name/);
  });
});

describe('X2 the open project survives', () => {
  const base = {
    route: 'store' as const,
    format: 'csv' as const,
    before: 'sha-before',
    after: 'sha-before',
    ok: false,
    error: 'Cannot export over the open project file',
    expectRefusal: true,
  };

  it('passes a refused store-route attempt that left the database alone', () => {
    expect(() => checkProjectFileSurvived(base, REPRO)).not.toThrow();
  });

  it('fails when the database bytes changed', () => {
    expect(() => checkProjectFileSurvived({ ...base, after: 'sha-after' }, REPRO)).toThrow(/MODIFIED/);
  });

  it('fails when the database is gone', () => {
    expect(() => checkProjectFileSurvived({ ...base, after: null }, REPRO)).toThrow(/GONE/);
  });

  it('fails when the raw command accepted the open project path', () => {
    expect(() => checkProjectFileSurvived({ ...base, ok: true, error: null }, REPRO)).toThrow(/ACCEPTED/);
  });

  it('does NOT require a refusal on a UI route - ensureExtension redirects there', () => {
    expect(() =>
      checkProjectFileSurvived(
        { ...base, route: 'modal', ok: true, error: null, expectRefusal: false },
        REPRO,
      ),
    ).not.toThrow();
  });
});

describe('X3 svg/png/pdf differential', () => {
  it('is silent when the SVG itself failed - it says nothing about the markup', () => {
    expect(() =>
      checkFormatDifferential({ format: 'svg', ok: false, error: 'no dir' }, [
        { format: 'png', ok: false, error: 'no dir' },
      ], REPRO),
    ).not.toThrow();
  });

  it('passes when all three succeeded', () => {
    expect(() =>
      checkFormatDifferential({ format: 'svg', ok: true, error: null }, [
        { format: 'png', ok: true, error: null },
        { format: 'pdf', ok: true, error: null },
      ], REPRO),
    ).not.toThrow();
  });

  it('fails when the SVG rendered and the raster could not parse it', () => {
    expect(() =>
      checkFormatDifferential({ format: 'svg', ok: true, error: null }, [
        { format: 'png', ok: false, error: 'Failed to parse SVG for PNG conversion' },
        { format: 'pdf', ok: true, error: null },
      ], REPRO),
    ).toThrow(/SVG export succeeded but png failed/);
  });
});

describe('X4 byte identity', () => {
  const clean = { missing: 0, duplicated: 0 };

  it('passes three identical exports', () => {
    const out = checkByteIdentity(
      { hashes: ['h', 'h', 'h'], sizes: [10, 10, 10], precondition: clean, extension: 'tmlu' },
      REPRO,
    );
    expect(out).toEqual({ stable: true, skipped: false });
  });

  it('fails when they differ and nothing in the exporter is allowed to re-mint', () => {
    expect(() =>
      checkByteIdentity(
        { hashes: ['a', 'b', 'b'], sizes: [10, 10, 10], precondition: clean, extension: 'tmlu' },
        REPRO,
      ),
    ).toThrow(/not byte-identical/);
  });

  it('SKIPS rather than asserts when a uuid is missing or duplicated', () => {
    // Both are documented exporter behaviour (shot_export_uuid's mint,
    // remint_duplicate_uuids), so asserting against them would be asserting the
    // app is different from what it says it is.
    for (const precondition of [{ missing: 1, duplicated: 0 }, { missing: 0, duplicated: 2 }]) {
      const out = checkByteIdentity(
        { hashes: ['a', 'b', 'c'], sizes: [10, 10, 10], precondition, extension: 'tmlu' },
        REPRO,
      );
      expect(out).toEqual({ stable: false, skipped: true });
    }
  });

  it('still fails an empty or absent artifact even when the uuid precondition is broken', () => {
    expect(() =>
      checkByteIdentity(
        {
          hashes: ['a', 'b', 'c'],
          sizes: [10, 0, 10],
          precondition: { missing: 3, duplicated: 0 },
          extension: 'tmlu',
        },
        REPRO,
      ),
    ).toThrow(/empty file/);
  });
});

describe('X5 convergence', () => {
  const A = '11111111-2222-4333-8444-555555555555';
  const B = 'aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee';

  it('settles at the second export when the first pair matches', () => {
    expect(checkConvergence({ exports: ['<x/>', '<x/>'] }, REPRO)).toEqual({
      via: 'raw',
      settledAt: 2,
    });
  });

  it('settles at the THIRD export, which a real survey needs', () => {
    // The first import inherits its section ORDER from the source project
    // rather than deriving it, so the composition takes one more pass to reach
    // its own fixed point. Measured: T2 != T3, T3 == T4, byte for byte, with
    // the same 40 sections and zero shots changing section. Finding 6.
    expect(checkConvergence({ exports: ['<a/>', '<b/>', '<b/>'] }, REPRO)).toEqual({
      via: 'raw',
      settledAt: 3,
    });
  });

  it('falls back to canonical when only the uuids moved, and SAYS so', () => {
    expect(
      checkConvergence({ exports: [`<s>${A}</s>`, `<s>${B}</s>`] }, REPRO),
    ).toEqual({ via: 'canonical', settledAt: 2 });
  });

  it('fails a chain that never settles', () => {
    // A new arrangement every pass: a user round-tripping twice would get a
    // differently-shaped project each time.
    expect(() =>
      checkConvergence({ exports: ['<a/>', '<b/>', '<c/>', '<d/>'] }, REPRO),
    ).toThrow(/never settles/);
  });

  it('fails when content is lost, because the loss makes every later pass differ', () => {
    expect(() =>
      checkConvergence(
        { exports: ['<Length>1</Length>', '<Length>2</Length>', '<Length>3</Length>'] },
        REPRO,
      ),
    ).toThrow(/never settles/);
  });

  it('refuses a chain too short to compare', () => {
    expect(() => checkConvergence({ exports: ['<x/>'] }, REPRO)).toThrow(/at least two/);
  });
});

describe('X6 shot multiset', () => {
  const shot = (over: Partial<ShotIdentity> = {}): ShotIdentity => ({
    length: 5,
    azimuthIn: 90,
    depthIn: 1,
    depthOut: 2,
    lrud: [1, 1, 1, 1],
    ...over,
  });

  it('keys on geometry values, not on order', () => {
    expect(shotKey(shot())).toBe(shotKey(shot()));
    expect(shotKey(shot({ azimuthIn: 91 }))).not.toBe(shotKey(shot()));
    expect(shotKey(shot({ lrud: [1, 1, 1, null] }))).not.toBe(shotKey(shot()));
  });

  it('rounds to the precision the format carries, so formatting is not data loss', () => {
    expect(shotKey(shot({ azimuthIn: 90.0000001 }))).toBe(shotKey(shot({ azimuthIn: 90 })));
  });

  it("reads Ariane's 0.0 LRUD as the absent sentinel it is, per field", () => {
    // `0.0` IS the format's null: the writer fills unwrap_or(0.0) and the reader
    // maps it back to None per field. A MEASURED zero therefore cannot round-trip
    // and an unmeasured one can. Documented at
    // docs/ariane-export.md#the-lrud-zeronull-collision.
    expect(shotKey(shot({ lrud: [0, 0, 0, 0] }))).toBe(shotKey(shot({ lrud: [null, null, null, null] })));
    expect(shotKey(shot({ lrud: [0, 3, 0, 2] }))).toBe(shotKey(shot({ lrud: [null, 3, null, 2] })));
  });

  it('keys a leg and its reciprocal the same - the exporter writes some backwards', () => {
    // Heading through 180, depths swapped, LEFT and RIGHT swapped with them
    // (up/down do NOT swap - turning round does not turn you over). This is the
    // transform export/tml.rs's reversed-leg constructors apply. Finding 5: 75
    // of 99 vanished keys paired exactly under it.
    const out = shot({ azimuthIn: 90, depthIn: 5, depthOut: 8, lrud: [1, 2, 3, 4] });
    const back = shot({ azimuthIn: 270, depthIn: 8, depthOut: 5, lrud: [2, 1, 3, 4] });
    expect(shotKey(out)).toBe(shotKey(back));
    expect(multisetDelta([out], [back])).toEqual([]);
  });

  it('handles the 0/360 wraparound in both directions', () => {
    expect(shotKey(shot({ azimuthIn: 0, depthIn: 1, depthOut: 2 }))).toBe(
      shotKey(shot({ azimuthIn: 180, depthIn: 2, depthOut: 1 })),
    );
    expect(shotKey(shot({ azimuthIn: 180, depthIn: 1, depthOut: 2 }))).toBe(
      shotKey(shot({ azimuthIn: 0, depthIn: 2, depthOut: 1 })),
    );
    // 360 is 0, not a 361st heading.
    expect(shotKey(shot({ azimuthIn: 360, depthIn: 1, depthOut: 2 }))).toBe(
      shotKey(shot({ azimuthIn: 180, depthIn: 2, depthOut: 1 })),
    );
  });

  it('is exact - a genuinely different shot is NOT folded into a reciprocal', () => {
    const out = shot({ azimuthIn: 90, depthIn: 5, depthOut: 8, lrud: [1, 2, 3, 4] });
    // heading off by one degree from the reciprocal
    expect(shotKey(out)).not.toBe(
      shotKey(shot({ azimuthIn: 271, depthIn: 8, depthOut: 5, lrud: [2, 1, 3, 4] })),
    );
    // LRUD not mirrored (up/down swapped instead of left/right)
    expect(shotKey(out)).not.toBe(
      shotKey(shot({ azimuthIn: 270, depthIn: 8, depthOut: 5, lrud: [1, 2, 4, 3] })),
    );
  });

  it('does not key depths or the tape - the junction rule may change them, and X6c checks it', () => {
    // #576 TML-DEPTH-D1, Q2-D1: a junction whose readings disagree is written
    // at their mean and the shots touching it take the length that keeps their
    // run; a shot whose drop exceeds its tape goes out vertical. Both change a
    // depth or a tape legitimately, so junctionDepthOracle.ts asserts them per
    // shot by uuid (junctionDepthOracle.test.ts), not this multiset.
    expect(shotKey(shot({ depthIn: 1, depthOut: 3 }))).toBe(shotKey(shot({ depthIn: 11, depthOut: 13 })));
    expect(shotKey(shot({ length: 0.15, depthIn: 13, depthOut: 16.0001 }))).toBe(
      shotKey(shot({ length: 3.0001, depthIn: 13, depthOut: 16.0001 })),
    );
  });

  it('folds ONLY that equivalence - a real LRUD change still differs', () => {
    // The line between a documented semantic and a tolerance.
    expect(shotKey(shot({ lrud: [0, 0, 0, 0] }))).not.toBe(shotKey(shot({ lrud: [3.5, 0, 0, 0] })));
    expect(shotKey(shot({ lrud: [3.5, 0, 0, 0] }))).not.toBe(shotKey(shot({ lrud: [null, 0, 0, 0] })));
    expect(shotKey(shot({ lrud: [3.5, 0, 0, 0] }))).not.toBe(shotKey(shot({ lrud: [3.6, 0, 0, 0] })));
  });

  it('does NOT key on the comment - the round trip moves it to the station', () => {
    // `commentsOnStations` defaults on, so a shot comment legitimately comes
    // back as a STATION comment. Keying the shot on it reported every comment
    // as lost while the text was sitting right there (measured, seed 1).
    expect(shotKey(shot())).toBe(shotKey(shot()));
  });

  it('pools comment survival over shots AND stations, split on ZWSP and trimmed', () => {
    // Moved from a shot to a station: still present, so no delta.
    expect(commentMultisetDelta(['one', 'two'], ['two', 'one'])).toEqual([]);
    // The co-encoding is undone by splitting before this is called.
    expect(commentMultisetDelta('a\u200bb'.split('\u200b'), ['b', 'a'])).toEqual([]);
    // Leading whitespace is trimmed by the reader by design.
    expect(commentMultisetDelta(['\tnote'], ['note'])).toEqual([]);
    // Text that is simply gone.
    expect(commentMultisetDelta(['kept', 'gone'], ['kept'])).toEqual([
      { key: 'gone', before: 1, after: 0 },
    ]);
    // A duplicate that lost one of its copies.
    expect(commentMultisetDelta(['x', 'x'], ['x'])).toEqual([{ key: 'x', before: 2, after: 1 }]);
  });

  it('passes an unchanged multiset with an unchanged uuid set', () => {
    expect(() =>
      checkShotMultiset(
        { deltas: [], uuidsBefore: ['u1'], uuidsAfter: ['u1'], countBefore: 1, countAfter: 1 },
        REPRO,
      ),
    ).not.toThrow();
  });

  it('fails when a shot GEOMETRY value changed', () => {
    const deltas = multisetDelta([shot()], [shot({ lrud: [1, 1, 1, 2] })]);
    expect(deltas).toHaveLength(2);
    expect(() =>
      checkShotMultiset({ deltas, uuidsBefore: ['u1'], uuidsAfter: ['u1'], countBefore: 1, countAfter: 1 }, REPRO),
    ).toThrow(/GEOMETRY key/);
  });

  it('fails when comment text disappeared, even with identical geometry and uuids', () => {
    expect(() =>
      checkShotMultiset(
        {
          deltas: [],
          commentDeltas: [{ key: 'a note', before: 1, after: 0 }],
          uuidsBefore: ['u1'],
          uuidsAfter: ['u1'],
          countBefore: 1,
          countAfter: 1,
        },
        REPRO,
      ),
    ).toThrow(/COMMENT text/);
  });

  it('fails when a uuid did not come back, even with identical geometry', () => {
    expect(() =>
      checkShotMultiset(
        { deltas: [], uuidsBefore: ['u1'], uuidsAfter: ['u2'], countBefore: 1, countAfter: 1 },
        REPRO,
      ),
    ).toThrow(/did not come back/);
  });

  it('flags a duplicate uuid WITHIN one section', () => {
    expect(() => checkNoDuplicateUuids([['u1', 'u2', 'u1']], REPRO)).toThrow(/same uuid/);
    expect(() => checkNoDuplicateUuids([['u1', 'u2']], REPRO)).not.toThrow();
  });

  it('ACCEPTS the same uuid in two different sections', () => {
    // Shot-uuid uniqueness is per-section by design: `resolve_key` and the
    // `(section_id, uuid)` cache are section-scoped, and the exporter handles
    // the document-level collision itself (`remint_duplicate_uuids`). Importing
    // one file twice lands the copies in NEW sections, so this is the ordinary
    // outcome, not a defect.
    expect(() => checkNoDuplicateUuids([['u1', 'u2'], ['u1', 'u2']], REPRO)).not.toThrow();
  });
});

describe('X6 folds the TML mixed-declination rotation (#576 HFI-8, decision HFI-8-D1)', () => {
  const shot = (azimuthIn: number, over: Partial<ShotIdentity> = {}): ShotIdentity => ({
    length: 5,
    azimuthIn,
    depthIn: 1,
    depthOut: 2,
    lrud: [1, 1, 1, 1],
    ...over,
  });
  const magnetic = (correction: number): SectionAzimuthReference => ({ north: 'magnetic', correction, onRecord: true });
  const unknown: SectionAzimuthReference = { north: 'magnetic', correction: 0, onRecord: false };
  const trueNorth: SectionAzimuthReference = { north: 'true', correction: 0, onRecord: true };
  const grid = (correction: number): SectionAzimuthReference => ({ north: 'grid', correction, onRecord: true });

  // A SYNTHETIC two-declination project: section A magnetic with an IGRF
  // correction of 3.71 (survey year 2018), section B magnetic with 4.11 (2020).
  // That is the multi-year-IGRF shape that made X6 red on every shot.
  const sectionA = [shot(90), shot(180.5, { length: 7 }), shot(358)];
  const sectionB = [shot(10), shot(270, { length: 3 }), shot(357, { length: 4 })];
  const before = [...sectionA, ...sectionB];
  const references = [...sectionA.map(() => magnetic(3.71)), ...sectionB.map(() => magnetic(4.11))];
  // What the re-import of the exported .tmlu reads, TRANSCRIBED BY HAND from
  // `AzimuthReference::to_true` (normalize(bearing + correction), per SECTION)
  // rather than computed by the function under test - so this pins the mirror
  // against the Rust rule instead of against itself. 358 + 3.71 and 357 + 4.11
  // wrap through north.
  const reimported = [
    shot(93.71),
    shot(184.21, { length: 7 }),
    shot(1.71),
    shot(14.11),
    shot(274.11, { length: 3 }),
    shot(1.11, { length: 4 }),
  ];
  const uuids = ['a1', 'a2', 'a3', 'b1', 'b2', 'b3'];
  const x6 = (deltas: ReturnType<typeof multisetDelta>, azimuthPlan?: TmlAzimuthPlan): void =>
    checkShotMultiset(
      {
        deltas,
        uuidsBefore: uuids,
        uuidsAfter: uuids,
        countBefore: before.length,
        countAfter: reimported.length,
        ...(azimuthPlan === undefined ? {} : { azimuthPlan }),
      },
      REPRO,
    );

  it('decides mixed-to-true for two sections that disagree on their correction', () => {
    expect(tmlAzimuthPlan(references)).toBe('mixed-to-true');
  });

  it('passes X6 once the BEFORE side is rotated the way the exporter rotates it', () => {
    const exported = asExportedToTml(before, references);
    // The X6 verdict first, so a regression reports the oracle's own message.
    expect(() => x6(multisetDelta(exported.shots, reimported), exported.plan)).not.toThrow();
    expect(multisetDelta(exported.shots, reimported)).toEqual([]);
    expect(exported.plan).toBe('mixed-to-true');
    expect(exported.rotated).toBe(6);
  });

  it('is the HFI-8 false positive without the fold: every shot reads as lost and gained', () => {
    const unfolded = multisetDelta(before, reimported);
    // No key survives: all six shots lost, all six gained. (Counted by shot,
    // not by key: the 90 and the 270 legs key equal, heading and LRUD being
    // all the key holds - a leg and its reciprocal.)
    expect(unfolded.every((d) => d.before === 0 || d.after === 0)).toBe(true);
    expect(unfolded.reduce((n, d) => n + d.before, 0)).toBe(6);
    expect(unfolded.reduce((n, d) => n + d.after, 0)).toBe(6);
    expect(() => x6(unfolded)).toThrow(/X6 the shot set changed/);
  });

  it('still fails a bearing rotated by the WRONG section correction', () => {
    // b1 rotated by section A's 3.71 instead of its own 4.11.
    const wrong = reimported.map((s, index) => (index === 3 ? shot(13.71) : s));
    const exported = asExportedToTml(before, references);
    expect(() => x6(multisetDelta(exported.shots, wrong), exported.plan)).toThrow(/already rotated into true north/);
  });

  it('still fails any other change - an LRUD that moved', () => {
    const changed = reimported.map((s, index) => (index === 0 ? { ...s, lrud: [1, 1, 1, 2] as ShotIdentity['lrud'] } : s));
    const exported = asExportedToTml(before, references);
    expect(() => x6(multisetDelta(exported.shots, changed), exported.plan)).toThrow(/X6 the shot set changed/);
    // A length is X6c's (junctionDepthOracle.ts), which the junction rule may
    // change; the multiset no longer keys it.
    const longer = reimported.map((s, index) => (index === 0 ? { ...s, length: 5.5 } : s));
    expect(() => x6(multisetDelta(exported.shots, longer), exported.plan)).not.toThrow();
  });

  it('leaves the shots untouched on every plan that writes the readings as stored', () => {
    const cases: Array<[string, SectionAzimuthReference[], TmlAzimuthPlan]> = [
      ['one shared magnetic correction', [magnetic(3.71), magnetic(3.71)], 'magnetic'],
      ['nothing declared', [unknown, unknown], 'magnetic'],
      ['every section true north', [trueNorth, trueNorth], 'true-as-stored'],
      // An unknown section abstains: one declared magnetic correction beside it
      // is still a uniform magnetic document (the ordinary .tml + .dmp case).
      ['one magnetic correction plus a section with none on record', [magnetic(3.71), unknown], 'magnetic'],
      [
        'disagreeing corrections plus a section with none on record',
        [magnetic(3.71), magnetic(4.11), unknown],
        'magnetic-unresolved',
      ],
      ['true north plus a section with none on record', [trueNorth, unknown], 'magnetic-unresolved'],
      ['grid plus a section with none on record', [grid(1.5), unknown], 'magnetic-unresolved'],
    ];
    for (const [what, refs, plan] of cases) {
      const shots = refs.map((_ref, index) => shot(90 + index * 110));
      const exported = asExportedToTml(shots, refs);
      expect(exported.plan, what).toBe(plan);
      expect(exported.rotated, what).toBe(0);
      expect(exported.shots, what).toEqual(shots);
    }
  });

  it('rotates a uniform GRID project, and leaves a zero correction as the identity', () => {
    const gridded = asExportedToTml([shot(90), shot(359)], [grid(1.5), grid(1.5)]);
    expect(gridded.plan).toBe('mixed-to-true');
    expect(gridded.shots.map((s) => s.azimuthIn)).toEqual([91.5, 0.5]);

    // True north (correction 0) beside a magnetic 3.71: the document is
    // rotated, the true-north section's bearings go out exactly as stored.
    const mixed = asExportedToTml([shot(90), shot(90)], [trueNorth, magnetic(3.71)]);
    expect(mixed.plan).toBe('mixed-to-true');
    expect(mixed.rotated).toBe(1);
    expect(mixed.shots[0]).toEqual(shot(90));
    expect(mixed.shots[1]?.azimuthIn).toBeCloseTo(93.71, 9);
  });

  it('refuses a reference list that is not parallel to the shots', () => {
    expect(() => asExportedToTml([shot(90)], [])).toThrow(/1 shot\(s\) but 0 reference\(s\)/);
  });
});

describe('X8 option independence', () => {
  const options = (over: Partial<OptionSnapshot> = {}): OptionSnapshot => ({
    exportType: 'image',
    dataFormat: 'csv',
    imageFormat: 'png',
    dpi: 300,
    branchingThreshold: 2,
    ...over,
  });

  it('passes when the panel is untouched by the modal', () => {
    expect(() =>
      checkOptionIndependence(
        { panelBefore: options(), panelAfter: options(), modalChangedTo: options({ dpi: 600 }) },
        REPRO,
      ),
    ).not.toThrow();
  });

  it('fails when the modal wrote through to the panel', () => {
    expect(() =>
      checkOptionIndependence(
        {
          panelBefore: options(),
          panelAfter: options({ dpi: 600 }),
          modalChangedTo: options({ dpi: 600 }),
        },
        REPRO,
      ),
    ).toThrow(/changed the PANEL/);
  });

  it('fails when export options survive a project switch', () => {
    expect(() =>
      checkExportStoreReset(
        {
          dirty: options({ dataFormat: 'tml', dpi: 600 }),
          afterSwitch: options({ dataFormat: 'tml', dpi: 600 }),
          defaults: options(),
        },
        REPRO,
      ),
    ).toThrow(/survived a project switch/);
  });

  it('passes when they were reset', () => {
    expect(() =>
      checkExportStoreReset(
        { dirty: options({ dpi: 600 }), afterSwitch: options(), defaults: options() },
        REPRO,
      ),
    ).not.toThrow();
  });
});

describe('X9 export-mode invariants', () => {
  const survey: ModeSnapshot = {
    appMode: 'survey',
    geometryMode: 'measured',
    mapReadOnly: false,
    hasEligibleClosures: true,
  };
  const exporting: ModeSnapshot = {
    appMode: 'export',
    geometryMode: 'closureAdjusted',
    mapReadOnly: true,
    hasEligibleClosures: true,
  };

  it('passes the enter/exit round trip', () => {
    expect(() =>
      checkExportModeInvariants({ before: survey, during: exporting, after: survey }, REPRO),
    ).not.toThrow();
  });

  it('fails when export mode did not force closure-adjusted geometry', () => {
    expect(() =>
      checkExportModeInvariants(
        { before: survey, during: { ...exporting, geometryMode: 'measured' }, after: survey },
        REPRO,
      ),
    ).toThrow(/closure-adjusted/);
  });

  it("prints a mode only when it is one of the store's own words (#576 W51)", () => {
    const headline = (during: ModeSnapshot): string => {
      try {
        checkExportModeInvariants({ before: survey, during, after: survey }, REPRO);
      } catch (err) {
        return (err as Error).message.split('\n')[0] ?? '';
      }
      return '';
    };
    expect(headline({ ...exporting, appMode: 'plan' })).toBe('X9 entering Export mode left appMode at "plan"');
    expect(headline({ ...exporting, appMode: 'qzCaveA' })).toBe('X9 entering Export mode left appMode at "7 characters"');
    expect(headline({ ...exporting, geometryMode: 'qzCaveA' })).not.toContain('qzCaveA');
    expect(headline({ ...exporting, geometryMode: 'measured' })).toContain('(geometryMode="measured")');
  });

  it('EXEMPTS a project with no eligible closure from the geometry half', () => {
    // MapCanvas reverts closureAdjusted to measured there by design ("the
    // adjusted view is meaningless without an eligible closure"), so asserting
    // it would report the app's documented fallback as a defect.
    expect(() =>
      checkExportModeInvariants(
        {
          before: { ...survey, hasEligibleClosures: false },
          during: { ...exporting, geometryMode: 'measured', hasEligibleClosures: false },
          after: { ...survey, hasEligibleClosures: false },
        },
        REPRO,
      ),
    ).not.toThrow();
  });

  it('still requires READ-ONLY on a project with no eligible closure', () => {
    expect(() =>
      checkExportModeInvariants(
        {
          before: { ...survey, hasEligibleClosures: false },
          during: {
            ...exporting,
            geometryMode: 'measured',
            mapReadOnly: false,
            hasEligibleClosures: false,
          },
          after: { ...survey, hasEligibleClosures: false },
        },
        REPRO,
      ),
    ).toThrow(/still EDITABLE/);
  });

  it('fails when the map stayed editable', () => {
    expect(() =>
      checkExportModeInvariants(
        { before: survey, during: { ...exporting, mapReadOnly: false }, after: survey },
        REPRO,
      ),
    ).toThrow(/still EDITABLE/);
  });

  it('fails when the exit did not restore the geometry mode', () => {
    expect(() =>
      checkExportModeInvariants(
        { before: survey, during: exporting, after: { ...survey, geometryMode: 'closureAdjusted' } },
        REPRO,
      ),
    ).toThrow(/did not restore/);
  });
});
