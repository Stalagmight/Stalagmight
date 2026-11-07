/**
 * #576 RV19b-P4, P5: which short names the fold takes, and two more spellings.
 *
 * P4: a name under three characters is never folded, since a folder named `1`
 * or `A2` (a Walls `.PATH` folder, a Compass survey folder) would fold every
 * `1` or `A2` in every line. That reason holds for ASCII only. The minimum
 * counted UTF-16 units, so a two-character name in another script - a cave
 * name of two CJK characters - was never folded, though nothing ASCII the
 * harness writes can contain it. Such a name now folds; ASCII ones under three
 * characters still do not, nor does one character written decomposed.
 *
 * P5: a form encoder writes a space as `+`, and an HTML writer a character as
 * a numeric reference (`&#233;`, `&#xE9;`) or as `&amp;`, `&lt;`, `&gt;`,
 * `&quot;` or `&#39;`. Both now fold.
 *
 * Everything here is synthetic: token names and a copy directory that is only a
 * string.
 */
import { describe, expect, it } from 'vitest';

import { redactCorpusText, registerCorpusRedactions } from './fixtureCorpus';

/** Two CJK characters (U+6D1E U+7A9F): a whole cave name. */
const CJK = '\u6d1e\u7a9f';
/** One character, which macOS writes decomposed: two code units, one letter. */
const ONE_ACCENTED = 'e\u0301';
const SHORT_ASCII = ['1', 'A2'];
const SPACED = 'Qzcafe Nord & Sud.srv';
const ACCENTED = 'Qzcaf\u00e9 <Nord>.srv';
const QUOTED = 'Qzcave "Nord\'s".srv';

registerCorpusRedactions('shrt1', '', [CJK, ONE_ACCENTED, ...SHORT_ASCII]);
registerCorpusRedactions('form5', '', [SPACED, ACCENTED, QUOTED]);

describe('a two-character name in another script folds (#576 RV19b-P4)', () => {
  it('folds a two-character CJK name, as itself and escaped', () => {
    expect(redactCorpusText(`opened ${CJK}/a.srv`)).toBe('opened <corpus #shrt1>/file/a.srv');
    expect(redactCorpusText('opened \\u6d1e\\u7a9f/a.srv')).toBe('opened <corpus #shrt1>/file/a.srv');
  });

  it('leaves ASCII names under three characters, and one decomposed letter, alone', () => {
    const line = `step 1 of A2: caf${ONE_ACCENTED} 12`;
    expect(redactCorpusText(line)).toBe(line);
  });
});

describe('the fold finds a name form-encoded or as HTML writes it (#576 RV19b-P5)', () => {
  it('folds `+` for a space', () => {
    expect(redactCorpusText('q=Qzcafe+Nord+%26+Sud.srv&x=1')).toBe('q=<corpus #form5>/file&x=1');
  });

  it('folds numeric character references and the named ones for & < > " \'', () => {
    expect(redactCorpusText('<p>Qzcafe Nord &amp; Sud.srv</p>')).toBe('<p><corpus #form5>/file</p>');
    expect(redactCorpusText('Qzcaf&#233; &lt;Nord&gt;.srv')).toBe('<corpus #form5>/file');
    expect(redactCorpusText('Qzcaf&#xE9; &#60;Nord&#x3e;.srv')).toBe('<corpus #form5>/file');
    expect(redactCorpusText('Qzcave &quot;Nord&#39;s&quot;.srv')).toBe('<corpus #form5>/file');
    expect(redactCorpusText('Qzcave &#34;Nord&apos;s&#x22;.srv')).toBe('<corpus #form5>/file');
  });
});
