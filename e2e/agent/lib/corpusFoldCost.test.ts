/**
 * #576 RV18-W23-1: folding a line costs time in proportion to the line.
 *
 * The fold runs on every tool result, log line and action-log line a corpus
 * run records. Its escape-tolerant pattern tried a run of backslashes from each
 * of its backslashes and read the rest of the run every time, and two runs side
 * by side (a backslash of the name before an escaped character) split it every
 * way there is - 40k backslashes took seconds and a 1 MB backslash line did not
 * finish. Each case below is a line built against one name's pattern, with the
 * name's ASCII anchor present so the pattern really runs, and the name's own
 * escaped form at the end so the fold is shown still to find it.
 *
 * Everything here is synthetic: token names, no file on disk.
 */
import { describe, expect, it } from 'vitest';

import { redactCorpusText, registerCorpusRedactions } from './fixtureCorpus';

/** Far over the 2 MB rung's fold time once linear, far under the quadratic fold's. */
const BOUND_MS = 1000;
const MB = 1 << 20;
const B = '\\';

/** JSON as an ASCII-only encoder writes it, inside the quotes. */
const asciiJson = (value: string): string =>
  JSON.stringify(value)
    .slice(1, -1)
    .replace(/[\u007f-\uffff]/g, (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);

/**
 * Fold ever longer lines, doubling from 1 KB to 2 MB, and stop at the first
 * rung over the bound: a fold that is not linear fails here in seconds rather
 * than running for hours on the 1 MB rung.
 */
function ladder(line: (size: number) => string): { size: number; ms: number; folded: string }[] {
  const rungs: { size: number; ms: number; folded: string }[] = [];
  for (let size = 1024; size <= 2 * MB; size *= 2) {
    const text = line(size);
    const start = performance.now();
    const folded = redactCorpusText(text);
    const ms = performance.now() - start;
    rungs.push({ size, ms: Math.round(ms), folded });
    if (ms > BOUND_MS) break;
  }
  return rungs;
}

describe('the corpus fold is linear in the line it folds (#576 RV18-W23-1)', () => {
  it.each([
    // `body(size)` is the line's adversarial part, which holds no whole name.
    {
      what: 'a name with no ASCII anchor, after a backslash run',
      id: 'cost1',
      name: '\uff23\uff41\uff56\uff45\uff21',
      body: (n: number) => B.repeat(n),
    },
    {
      what: 'a name whose ASCII anchor is in the line (".mak" style)',
      id: 'cost2',
      name: '\u015aCaveB-na.mak',
      body: (n: number) => `${B.repeat(n)} CaveB-na.mak`,
    },
    {
      what: 'a name that starts with a quote',
      id: 'cost3',
      name: '"CaveC" south.dmp',
      body: (n: number) => `${B.repeat(n)} south.dmp`,
    },
    {
      what: 'a backslash of the name before an escaped character',
      id: 'cost4',
      name: `CaveD${B}\u00e9 east.dmp`,
      body: (n: number) => `CaveD${B.repeat(n)} east.dmp`,
    },
    {
      what: 'a name that starts with two backslashes',
      id: 'cost5',
      name: `${B}${B}\u00e9CaveE.dmp`,
      body: (n: number) => `${B.repeat(n)} CaveE.dmp`,
    },
    {
      what: 'a name that starts with U+FFFD, after a run of \\xHH bytes',
      id: 'cost6',
      name: '\ufffdCaveF.dmp',
      body: (n: number) => `${`${B}x41`.repeat(n / 4)} CaveF.dmp`,
    },
    {
      // #576 RV18-W23-5: every character but a letter, a digit and `-._~` may be `%HH` too.
      what: 'a name that starts with a non-ASCII character, after a run of %HH bytes',
      id: 'cost7',
      name: '\u00e9 CaveG.dmp',
      body: (n: number) => `${'%C3'.repeat(Math.floor(n / 3))}%20CaveG.dmp`,
    },
  ])('folds $what in linear time, and still folds the name', ({ id, name, body }) => {
    registerCorpusRedactions(id, '', [name]);
    // The name as an ASCII-only JSON encoder writes it, inside a `{:?}` path.
    const tail = ` path="${asciiJson(name)}"`;
    const rungs = ladder((size) => `${body(size)}${tail}`);
    const timings = rungs.map(({ size, ms }) => `${size / 1024}K=${ms}ms`).join(' ');
    // Every rung under the bound, 1 MB and 2 MB included.
    expect({ overBound: rungs.filter((r) => r.ms > BOUND_MS).length, timings }).toEqual({ overBound: 0, timings });
    expect(rungs.map((r) => r.size)).toContain(MB);
    expect(rungs.at(-1)?.size).toBe(2 * MB);
    // The body as it was, and the name folded to the entry's tag.
    expect(rungs.at(-1)?.folded === `${body(2 * MB)} path="<corpus #${id}>/file"`).toBe(true);
  }, 120_000);
});
