/**
 * #576 W58 NF2: fuzz-shrink prints a trace step's test id, trigger, path, export
 * format, oracle name and corpus id shaped, never as the trace holds them:
 * `readTrace` checks those fields only as text. Synthetic names only (`qz...`,
 * which no harness or app source contains).
 */
import * as os from 'os';
import * as path from 'path';
import { describe, expect, it } from 'vitest';

import { shareableTestId } from '../../../scripts/lib/shareable-selector.mjs';
import { shortId } from './crc32';
import { DEFAULT_FIXTURE } from './fixtures';
import { describeSteps, type TraceStep } from './fuzzTrace';
import { foldLocalPaths } from './shareableFailure';

const lines = (steps: TraceStep[]): string[] => describeSteps(steps).split('\n');

describe("fuzz-shrink's minimal step list prints each field shaped (#576 W58 NF2)", () => {
  it('prints a test id in the test-id form', () => {
    const out = lines([
      { kind: 'ui', testid: 'continue-button', label: 'continue' },
      { kind: 'ui', testid: 'qzCaveA-row', double: true, label: 'continue' },
      { kind: 'input', testid: 'qzStationB', value: 'x', mode: 'type', label: 'continue' },
    ]);
    expect(out).toEqual([
      '  1. click continue-button - continue',
      `  2. dblclick ${shareableTestId('qzCaveA-row')} - continue`,
      `  3. type ${shareableTestId('qzStationB')} - continue`,
    ]);
    expect(out.join('\n')).not.toMatch(/qz/);
  });

  it('prints a trigger only when it is one the harness records, else its length', () => {
    const out = lines([
      { kind: 'fileDialog', trigger: 'toolbar-import-button', dialog: 'open', rel: null, label: 'continue' },
      { kind: 'fileDialog', trigger: 'key:Ctrl+O', dialog: 'open', rel: ['a', 'b'], label: 'continue' },
      { kind: 'fileDialog', trigger: 'qzCaveA-button', dialog: 'open', rel: null, label: 'continue' },
    ]);
    expect(out).toEqual([
      '  1. open-dialog via toolbar-import-button -> cancel - continue',
      '  2. open-dialog via key:Ctrl+O -> 2 file(s) - continue',
      '  3. open-dialog via a string of 14 character(s) -> cancel - continue',
    ]);
  });

  it('prints every path folded', () => {
    const home = path.join(os.homedir(), 'fold-probe', 'a.dmp');
    const folded = foldLocalPaths(home);
    expect(folded).not.toContain(os.homedir());
    const out = lines([
      { kind: 'fileDialog', trigger: 'toolbar-import-button', dialog: 'open', rel: home, label: 'continue' },
      { kind: 'genFile', spec: {}, rel: home, label: 'continue' },
      { kind: 'corpusFile', corpusId: 'abc12', rel: home, label: 'continue' },
      { kind: 'route', rel: home, label: 'continue' },
      { kind: 'export', format: 'csv', options: {}, rel: home, label: 'continue' },
    ]);
    expect(out).toEqual([
      `  1. open-dialog via toolbar-import-button -> ${folded} - continue`,
      `  2. genFile ${folded} - continue`,
      `  3. corpusFile #abc12 -> ${folded} - continue`,
      `  4. route ${folded} - continue`,
      `  5. export csv -> ${folded} - continue`,
    ]);
  });

  it('prints a relative path as itself only when the repo tracks it, else as its tag (#576 W74-NF2)', () => {
    // A relative path is all readTrace lets through; folding replaces only the local roots, so it printed as written.
    const tag = (rel: string): string => `<private-path#${shortId(rel)}>`;
    const out = lines([
      { kind: 'genFile', spec: {}, rel: 'qzCaveA/x.dmp', label: 'continue' },
      { kind: 'fileDialog', trigger: 'toolbar-import-button', dialog: 'open', rel: 'qzCaveA/x.dmp', label: 'continue' },
      { kind: 'corpusFile', corpusId: 'abc12', rel: 'fuzz-files/qzCaveB.tml', label: 'continue' },
      { kind: 'route', rel: 'qzStationC.svx', label: 'continue' },
      { kind: 'export', format: 'csv', options: {}, rel: 'out/qzCaveD.csv', label: 'continue' },
      // What the repo tracks: a path git lists, and a session's copy of a tracked fixture.
      { kind: 'route', rel: 'e2e/agent/lib/fuzzTrace.ts', label: 'continue' },
      { kind: 'route', rel: `fixtures/${DEFAULT_FIXTURE}`, label: 'continue' },
      // A tracked fixture's name under another directory is not its copy: the directory could be a name.
      { kind: 'route', rel: `qzCaveE/${DEFAULT_FIXTURE}`, label: 'continue' },
    ]);
    expect(out).toEqual([
      `  1. genFile ${tag('qzCaveA/x.dmp')} - continue`,
      `  2. open-dialog via toolbar-import-button -> ${tag('qzCaveA/x.dmp')} - continue`,
      `  3. corpusFile #abc12 -> ${tag('fuzz-files/qzCaveB.tml')} - continue`,
      `  4. route ${tag('qzStationC.svx')} - continue`,
      `  5. export csv -> ${tag('out/qzCaveD.csv')} - continue`,
      '  6. route e2e/agent/lib/fuzzTrace.ts - continue',
      `  7. route fixtures/${DEFAULT_FIXTURE} - continue`,
      `  8. route ${tag(`qzCaveE/${DEFAULT_FIXTURE}`)} - continue`,
    ]);
    expect(out.join('\n')).not.toMatch(/qz/);
  });

  it('prints an export format only when it is one the harness exports, else its length', () => {
    const out = lines([
      { kind: 'export', format: 'tmlu', options: {}, rel: 'out/a.tmlu', label: 'continue' },
      { kind: 'export', format: 'qzCaveA', options: {}, rel: 'out/a', label: 'continue' },
    ]);
    expect(out).toEqual([
      `  1. export tmlu -> <private-path#${shortId('out/a.tmlu')}> - continue`,
      `  2. export a string of 7 character(s) -> <private-path#${shortId('out/a')}> - continue`,
    ]);
  });

  it('prints an oracle name only when it is one the harness runs, else its length', () => {
    const out = lines([
      { kind: 'oracle', oracle: 'x4-byte-identity', inputs: {}, label: 'continue' },
      { kind: 'oracle', oracle: 'qzStationB', inputs: {}, label: 'continue' },
    ]);
    expect(out).toEqual(['  1. oracle x4-byte-identity - continue', '  2. oracle a string of 10 character(s) - continue']);
  });

  it('prints a corpus id only in its id form, else its length', () => {
    const out = lines([
      { kind: 'corpusFile', corpusId: '0f3a9', rel: 'fuzz-files/corpus-1', label: 'continue' },
      { kind: 'corpusFile', corpusId: 'qzCaveA/x.dmp', rel: 'fuzz-files/corpus-2', label: 'continue' },
    ]);
    expect(out).toEqual([
      `  1. corpusFile #0f3a9 -> <private-path#${shortId('fuzz-files/corpus-1')}> - continue`,
      `  2. corpusFile a string of 13 character(s) -> <private-path#${shortId('fuzz-files/corpus-2')}> - continue`,
    ]);
  });
});
