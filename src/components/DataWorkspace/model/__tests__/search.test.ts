import { describe, expect, it } from 'vitest';
import {
  compileSearch,
  execAll,
  findInText,
  isWholeWord,
  orderColumns,
  searchColumnsInScope,
  type SearchPattern,
} from '../search';
import { makeColumns } from './fixtures';

const pat = (query: string, o: Partial<SearchPattern> = {}): SearchPattern => ({
  query,
  matchCase: false,
  wholeWord: false,
  regex: false,
  ...o,
});

/** All [start, end) pairs of `query` in `text`. */
function spans(text: string, p: SearchPattern): [number, number][] {
  const out: number[] = [];
  findInText(text, compileSearch(p), out);
  const pairs: [number, number][] = [];
  for (let i = 0; i < out.length; i += 2) pairs.push([out[i]!, out[i + 1]!]);
  return pairs;
}

describe('compileSearch', () => {
  it('an empty query searches nothing', () => {
    expect(compileSearch(pat('')).regex).toBeNull();
    expect(compileSearch(pat('')).error).toBeNull();
  });

  it('literal mode escapes regex syntax', () => {
    expect(spans('a.b axb a.b', pat('a.b'))).toEqual([
      [0, 3],
      [8, 11],
    ]);
    expect(spans('cost $5 (x)', pat('$5 (x)'))).toEqual([[5, 11]]);
  });

  it('is case-insensitive unless match case is on', () => {
    expect(spans('Sump sump SUMP', pat('sump'))).toHaveLength(3);
    expect(spans('Sump sump SUMP', pat('sump', { matchCase: true }))).toEqual([[5, 9]]);
    // Unicode case folding
    expect(spans('ÉCHO écho', pat('écho'))).toHaveLength(2);
  });

  it('an invalid regex reports an error and searches nothing', () => {
    const c = compileSearch(pat('a(b', { regex: true }));
    expect(c.regex).toBeNull();
    expect(c.error).toMatch(/./);
  });

  it('falls back to legacy syntax when only the u flag rejects a pattern', () => {
    // `\-` outside a class is an invalid identity escape under the u flag.
    const c = compileSearch(pat('A\\-3', { regex: true }));
    expect(c.error).toBeNull();
    expect(spans('A-3', pat('A\\-3', { regex: true }))).toEqual([[0, 3]]);
  });

  it('supports unicode property escapes in regex mode', () => {
    expect(spans('A1 Bé 22', pat('\\p{L}+', { regex: true }))).toEqual([
      [0, 1],
      [3, 5],
    ]);
  });
});

describe('findInText', () => {
  it('skips zero-length matches without looping', () => {
    expect(spans('abc', pat('^', { regex: true }))).toEqual([]);
    expect(spans('baab', pat('a*', { regex: true }))).toEqual([[1, 3]]);
    expect(spans('x😀y', pat('(?:)', { regex: true }))).toEqual([]);
    expect(spans('😀😀', pat('😀*', { regex: true }))).toEqual([[0, 4]]);
  });

  it('respects the limit', () => {
    const out: number[] = [];
    expect(findInText('aaaa', compileSearch(pat('a')), out, 2)).toBe(2);
    expect(out).toEqual([0, 1, 1, 2]);
    expect(findInText('aaaa', compileSearch(pat('a')), [], 0)).toBe(0);
  });

  it('finds nothing in empty text', () => {
    expect(spans('', pat('a'))).toEqual([]);
  });
});

describe('whole word', () => {
  it('rejects matches inside a word', () => {
    expect(spans('A1 A10 A1', pat('a1', { wholeWord: true }))).toEqual([
      [0, 2],
      [7, 9],
    ]);
  });

  it('uses unicode letters as word characters', () => {
    expect(spans('Höhle Höhlen', pat('höhle', { wholeWord: true }))).toEqual([[0, 5]]);
    expect(spans('grand洞 洞', pat('洞', { wholeWord: true }))).toEqual([[7, 8]]);
    expect(spans('café cafés', pat('café', { wholeWord: true }))).toEqual([[0, 4]]);
  });

  it('accepts a match that starts or ends with a non-word character', () => {
    expect(spans('A-3 B-30', pat('-3', { wholeWord: true }))).toEqual([[1, 3]]);
    expect(isWholeWord('A-3', 1, 3)).toBe(true);
    expect(isWholeWord('xA', 1, 2)).toBe(false);
  });

  it('retries the next alignment after a rejected match', () => {
    expect(spans('aaa aa', pat('aa', { wholeWord: true }))).toEqual([[4, 6]]);
  });

  it('applies to regex matches too', () => {
    expect(spans('A12 A1 B1x', pat('[A-Z]\\d', { regex: true, wholeWord: true }))).toEqual([[4, 6]]);
  });
});

describe('execAll', () => {
  it('returns exec results with groups keyed by start', () => {
    const all = execAll('A12 B3', compileSearch(pat('([A-Z])(\\d+)', { regex: true })));
    expect([...all.keys()]).toEqual([0, 4]);
    expect(all.get(4)![2]).toBe('3');
  });
});

describe('column order and scope', () => {
  const columns = makeColumns();

  it('orderColumns puts the persisted order first and keeps the rest in default order', () => {
    expect(orderColumns(columns, []).map((c) => c.key)).toEqual(columns.map((c) => c.key));
    expect(orderColumns(columns, ['comment', 'nope' as never, 'length', 'comment']).map((c) => c.key)).toEqual([
      'comment',
      'length',
      'type',
      'section',
      'name',
      'azimuth',
      'depth',
      'kind',
      'x',
    ]);
  });

  it('searchColumnsInScope: visible / all / custom, searchable only', () => {
    const cols = columns.map((c) => (c.key === 'x' ? { ...c, searchable: false } : c));
    const keysOf = (s: Parameters<typeof searchColumnsInScope>[1], hidden: string[]) =>
      searchColumnsInScope(cols, s, hidden as never).map((c) => c.key);
    expect(keysOf({ columnScope: 'visible', columns: [] }, ['comment', 'depth'])).toEqual([
      'type',
      'section',
      'name',
      'length',
      'azimuth',
      'kind',
    ]);
    expect(keysOf({ columnScope: 'all', columns: [] }, ['comment'])).toContain('comment');
    expect(keysOf({ columnScope: 'all', columns: [] }, [])).not.toContain('x');
    expect(keysOf({ columnScope: 'custom', columns: ['comment', 'x'] }, ['comment'])).toEqual(['comment']);
  });
});
