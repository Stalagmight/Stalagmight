/**
 * #576 W39-NF5: a corpus name that ends in a backslash, right before a longer
 * name that opens with a quote, folds too.
 *
 * Names fold longest first, and a name's first character takes every backslash
 * in front of it as its escape (`\"` is how JSON writes a quote, `\u00e9` how an
 * ASCII-only encoder writes `é`). So in `qzab\"qzlonger.dmp` the longer name
 * took the shorter one's backslash, and the shorter one, left without it,
 * printed: `qzab<corpus #id>/file`. A second fold did not help.
 *
 * Everything here is synthetic: token names, no copy directory.
 */
import { describe, expect, it } from 'vitest';

import { redactCorpusText, registerCorpusRedactions } from './fixtureCorpus';

const B = '\\';
/** Ends in a backslash. */
const SHORT = `qzab${B}`;
/** Longer, and opens with a quote. */
const QUOTED = '"qzlonger.dmp';
/** Longer, and opens with a character an ASCII-only encoder escapes. */
const ACCENTED = '\u00e9qzaccent.dmp';

registerCorpusRedactions('adj1', '', [SHORT]);
registerCorpusRedactions('adj2', '', [QUOTED]);
registerCorpusRedactions('adj3', '', [ACCENTED]);

const T1 = '<corpus #adj1>/file';
const T2 = '<corpus #adj2>/file';
const T3 = '<corpus #adj3>/file';

/** The longer name is longer, so it folds first: the order the finding needs. */
it('the inputs are what they say', () => {
  expect(QUOTED.length).toBeGreaterThan(SHORT.length);
  expect(ACCENTED.length).toBeGreaterThan(SHORT.length);
});

describe('a name ending in a backslash before a name its backslash can escape folds (#576 W39-NF5)', () => {
  it.each([
    { what: 'the finding: both names as they are', text: `x ${SHORT}${QUOTED} y`, folded: `x ${T1}${T2} y` },
    {
      what: 'both names in a JSON string',
      text: `{"path":${JSON.stringify(`${SHORT}${QUOTED}`)}}`,
      folded: `{"path":"${T1}${T2}"}`,
    },
    {
      what: 'the accented name escaped by an ASCII-only encoder',
      text: `x ${SHORT}${B}u00e9qzaccent.dmp y`,
      folded: `x ${T1}${T3} y`,
    },
    { what: 'the accented name as it is', text: `x ${SHORT}${ACCENTED} y`, folded: `x ${T1}${T3} y` },
  ])('folds $what, and a second fold changes nothing', ({ text, folded }) => {
    expect(redactCorpusText(text)).toBe(folded);
    expect(redactCorpusText(folded)).toBe(folded);
  });

  it('folds the quoted name escaped on its own as before: the tag keeps the backslash its quote took', () => {
    const once = redactCorpusText(`open ${JSON.stringify(`a ${QUOTED} b`)} failed`);
    expect(once).toBe(`open "a ${T2} b" failed`);
    expect(redactCorpusText(once)).toBe(once);
    expect(redactCorpusText(`x ${B}${B}u00e9qzaccent.dmp y`)).toBe(`x ${T3} y`);
  });
});
