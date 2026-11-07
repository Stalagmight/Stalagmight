/**
 * #576 W33-NF4: folding a folded line changes nothing.
 *
 * A line is folded more than once on its way out - a tool result is folded, cut
 * and folded again into the action log - and the fold looked for each name in
 * everything, its own tags included. A corpus name that occurs inside a tag
 * (`corpus`, `file`, a piece of `<corpus #id>/file`) garbled the tag on the
 * second fold, and on the first when an earlier name's tag held it. Nothing
 * leaked, but the line no longer said which entry it was. And a name that ends
 * in a backslash, written twice in a row, folded once: its backslash read as an
 * escape of the second one's quote, so the second one printed until the next
 * fold.
 *
 * Everything here is synthetic: token names and a copy directory that is only a
 * string.
 */
import { describe, expect, it } from 'vitest';

import { redactCorpusText, registerCorpusRedactions } from './fixtureCorpus';

const B = '\\';
const DIR = '/tmp/qzidem/sessions/run-1/fuzz-files/corpus-1';
const T1 = '<corpus #idem1>';
const F1 = `${T1}/file`;

const NAMED: { id: string; dir: string; names: string[] }[] = [
  // The finding's two: words of the tag itself.
  { id: 'idem1', dir: DIR, names: ['corpus', 'file'] },
  // A piece of its own tag, and a piece of another entry's.
  { id: 'idem2', dir: '', names: ['#idem2>'] },
  { id: 'idem3', dir: '', names: ['orpus #idem1>/fi'] },
  // `<`, `>` and `#`, which a tag is made of.
  { id: 'idem4', dir: '', names: ['qz<a>#b.dmp'] },
  // Opens with a quote and ends in a backslash.
  { id: 'idem5', dir: '', names: [`"qzq${B}`] },
];

/** A seeded generator, so a failure names an input that can be run again. */
function rng(seed: number): () => number {
  let state = seed;
  return () => {
    state = (state * 1103515245 + 12345) & 0x7fffffff;
    return state / 0x7fffffff;
  };
}
const rnd = rng(576);
const pick = <T>(items: readonly T[]): T => items[Math.floor(rnd() * items.length)]!;

/** Names drawn from what a tag is made of, plus the characters the escapes turn on. */
const NAME_CHARS = [...'corpusfile<>#/ 1dm', B, '"', 'é', 'q', 'z', '%', '�'];
const RANDOM: { id: string; dir: string; names: string[] }[] = Array.from({ length: 12 }, (_, k) => ({
  id: `idr${String(k).padStart(2, '0')}`,
  dir: k % 4 === 0 ? `/tmp/qzidem/r${k}` : '',
  names: [Array.from({ length: 3 + Math.floor(rnd() * 6) }, () => pick(NAME_CHARS)).join('')],
}));

for (const entry of [...NAMED, ...RANDOM]) registerCorpusRedactions(entry.id, entry.dir, entry.names);

const NAMES = [...NAMED, ...RANDOM].flatMap((entry) => (entry.dir === '' ? entry.names : [entry.dir, ...entry.names]));
const TAGS = [...NAMED, ...RANDOM].flatMap((entry) => [`<corpus #${entry.id}>`, `<corpus #${entry.id}>/file`]);

/** Rust's `{:?}` of a string, inside the quotes. */
function debugEscape(text: string): string {
  let out = '';
  for (const c of text) {
    const code = c.codePointAt(0)!;
    if (c === '"' || c === B) out += `${B}${c}`;
    else if (code < 0x20 || code === 0x301) out += `${B}u{${code.toString(16)}}`;
    else out += c;
  }
  return out;
}
/** A name in one of the forms the fold finds it in. */
function encode(name: string): string {
  const forms = [
    () => name,
    () => JSON.stringify(name).slice(1, -1),
    () => JSON.stringify(name).slice(1, -1).replace(/[\u007f-￿]/g, (c) => `${B}u${c.charCodeAt(0).toString(16).padStart(4, '0')}`),
    () => debugEscape(name),
    () => encodeURIComponent(name),
    () => name.normalize('NFD'),
  ];
  return pick(forms)();
}
const NOISE = [B, B, '"', '<', '>', '#', '/', 'a', 'x', 'u', '{', '}', '0', '4', '1', 'e', ' ', '%', 'é'];
/** One piece of a line: a name, a tag, a piece of a tag, something tag-shaped, or noise. */
function piece(): string {
  const r = rnd();
  if (r < 0.3) return encode(pick(NAMES));
  if (r < 0.45) return pick(TAGS);
  if (r < 0.6) {
    const tag = pick(TAGS);
    const start = Math.floor(rnd() * tag.length);
    return tag.slice(start, start + 1 + Math.floor(rnd() * (tag.length - start)));
  }
  if (r < 0.65) return pick(['<corpus #zzzzz>', '<corpus #idem1', '<corpus #<corpus #idem1>', '<corpus #idem1>/fil']);
  return Array.from({ length: 1 + Math.floor(rnd() * 4) }, () => pick(NOISE)).join('');
}

describe('folding a folded line changes nothing (#576 W33-NF4)', () => {
  it('folds a path through names `corpus` and `file` to whole tags, and leaves a folded line as it is', () => {
    const once = redactCorpusText(`could not read ${DIR}/file and corpus`);
    expect(once).toBe(`could not read ${T1}/${F1} and ${F1}`);
    expect(redactCorpusText(once)).toBe(once);
    // The line the finding refolded.
    const folded = `could not read ${T1}/sub/${F1}`;
    expect(redactCorpusText(folded)).toBe(folded);
  });

  it.each([
    { what: 'a piece of its own tag', text: 'missing #idem2> here', folded: 'missing <corpus #idem2>/file here' },
    { what: "a piece of another entry's tag", text: 'xorpus #idem1>/fiy', folded: 'x<corpus #idem3>/filey' },
    { what: 'a name holding <, > and #', text: 'open qz<a>#b.dmp failed', folded: 'open <corpus #idem4>/file failed' },
    {
      what: 'a name ending in a backslash, written twice in a row',
      text: `x "qzq${B}"qzq${B} y`,
      folded: 'x <corpus #idem5>/file<corpus #idem5>/file y',
    },
  ])('folds $what on the first fold, and a second changes nothing', ({ text, folded }) => {
    expect(redactCorpusText(text)).toBe(folded);
    expect(redactCorpusText(folded)).toBe(folded);
  });

  it('still folds a name in text that only looks like a tag', () => {
    // `<corpus #qz<a>` is no tag this process wrote, so it is text like any other.
    const once = redactCorpusText('open <corpus #qz<a>#b.dmp> failed');
    expect(once).not.toContain('qz<a>#b.dmp');
    expect(redactCorpusText(once)).toBe(once);
  });

  it('a second fold of any line changes nothing (3000 seeded random lines)', () => {
    const examples: { text: string; once: string; twice: string }[] = [];
    let unstable = 0;
    let folded = 0;
    for (let i = 0; i < 3000; i++) {
      const text = Array.from({ length: 1 + Math.floor(rnd() * 8) }, piece).join('');
      const once = redactCorpusText(text);
      const twice = redactCorpusText(once);
      if (once !== text) folded++;
      if (twice === once) continue;
      unstable++;
      if (examples.length < 3) examples.push({ text, once, twice });
    }
    expect({ unstable, examples }).toEqual({ unstable: 0, examples: [] });
    // The lines exercise the fold: most of them hold something it folds.
    expect(folded).toBeGreaterThan(1500);
  });
});
