import { describe, expect, it } from 'vitest';
import { compileSearch, execAll } from '../search';
import { expandReplacement, planReplace, replaceInText, type ReplaceSpec } from '../replace';
import { buildDataView } from '../view';
import { EMPTY_SEARCH } from '../../store';
import type { DataColumnKey, DataRow, SearchMatch } from '../../types';
import { makeColumns, makeCtx, makeData, makeRows, viewState, type SectionSeed } from './fixtures';

const spec = (query: string, replacement: string, o: Partial<ReplaceSpec> = {}): ReplaceSpec => ({
  query,
  replacement,
  matchCase: false,
  wholeWord: false,
  regex: false,
  ...o,
});

/** Replace every match of `s` in `text`. */
function replaceAll(text: string, s: ReplaceSpec): string {
  const ranges = [...execAll(text, compileSearch(s)).values()].map((m) => ({ start: m.index, end: m.index + m[0].length }));
  return replaceInText(text, ranges, s).text;
}

describe('expandReplacement', () => {
  const exec = (re: RegExp, text: string) => re.exec(text)!;

  it('expands $1, $&, $`, $\', $$ and named groups like String.replace', () => {
    const text = 'xx A12 yy';
    const m = exec(/(?<letter>[A-Z])(\d+)/, text);
    expect(expandReplacement('$2-$1', m, text)).toBe('12-A');
    expect(expandReplacement('[$&]', m, text)).toBe('[A12]');
    expect(expandReplacement('$`|$\'', m, text)).toBe('xx | yy');
    expect(expandReplacement('$$1', m, text)).toBe('$1');
    expect(expandReplacement('$<letter>!', m, text)).toBe('A!');
    expect(expandReplacement('$<nope>', m, text)).toBe('');
  });

  it('leaves unknown references literal', () => {
    const text = 'A12';
    const m = exec(/([A-Z])/, text);
    expect(expandReplacement('$0 $2 $<x> $', m, text)).toBe('$0 $2 $<x> $');
    // $10 with one group = group 1 then "0"
    expect(expandReplacement('$10', m, text)).toBe('A0');
    // matches the engine on the same inputs
    expect('A12'.replace(/([A-Z])/, '$0 $2 $<x> $ $10')).toBe(`${expandReplacement('$0 $2 $<x> $ $10', m, text)}12`);
  });
});

describe('replaceInText', () => {
  it('literal mode inserts the replacement as typed ($ is not special)', () => {
    expect(replaceAll('sump Sump SUMP', spec('sump', '$1 pool'))).toBe('$1 pool $1 pool $1 pool');
  });

  it('match case limits what is replaced', () => {
    expect(replaceAll('sump Sump', spec('Sump', 'X', { matchCase: true }))).toBe('sump X');
  });

  it('regex mode uses groups', () => {
    expect(replaceAll('A1 B22', spec('([A-Z])(\\d+)', '$2$1', { regex: true }))).toBe('1A 22B');
    expect(replaceAll('A1', spec('(?<l>[A-Z])', '<$<l>>', { regex: true }))).toBe('<A>1');
  });

  it('whole word replaces only whole words', () => {
    expect(replaceAll('A1 A10 A1', spec('a1', 'Z', { wholeWord: true }))).toBe('Z A10 Z');
  });

  it('replaces only the given ranges, and ignores ranges that are no longer matches', () => {
    const s = spec('a', 'X');
    expect(replaceInText('a b a b a', [{ start: 4, end: 5 }], s)).toEqual({ text: 'a b X b a', replaced: 1 });
    expect(replaceInText('a b a', [{ start: 1, end: 2 }], s)).toEqual({ text: 'a b a', replaced: 0 });
  });

  it('a zero-length pattern replaces nothing', () => {
    expect(replaceAll('abc', spec('x*', 'Y', { regex: true }))).toBe('abc');
  });
});

describe('planReplace', () => {
  const SECTIONS: SectionSeed[] = [
    {
      id: 1,
      name: 'Main',
      shots: [{ length: 10, comment: 'sump sump' }, { length: 11, comment: 'dry sump' }, { length: 12 }],
      stationComments: { 1: 'Sump pool' },
    },
  ];

  function setup(ctxOpts: Parameters<typeof makeCtx>[1] = {}) {
    const data = makeData(SECTIONS);
    const rows = makeRows(data);
    const ctx = makeCtx(data, ctxOpts);
    const columns = makeColumns();
    const model = {
      rowByKey: new Map<string, DataRow>(rows.map((r) => [r.key, r])),
      columnByKey: new Map<DataColumnKey, (typeof columns)[number]>(columns.map((c) => [c.key, c])),
    };
    const matchesFor = (query: string, o: Partial<typeof EMPTY_SEARCH> = {}) =>
      buildDataView(rows, columns, ctx, viewState({ search: { ...EMPTY_SEARCH, open: true, query, ...o } })).matches;
    return { rows, ctx, columns, model, matchesFor };
  }

  it('groups matches by cell: one edit per cell with all its matches replaced', () => {
    const { ctx, model, matchesFor } = setup();
    const matches = matchesFor('sump');
    expect(matches).toHaveLength(4);
    const plan = planReplace(matches, model, ctx, spec('sump', 'pool'));
    expect(plan.edits.map((e) => [e.row.key, e.colKey, e.value])).toEqual([
      ['sh:1:100001', 'comment', 'pool pool'],
      ['st:1:1', 'comment', 'pool pool'],
      ['sh:1:100002', 'comment', 'dry pool'],
    ]);
    expect(plan.editMatches).toEqual([2, 1, 1]);
    expect(plan.replacedMatches).toBe(4);
    expect(plan.lockedMatches).toBe(0);
  });

  it('replaces only the current match when given one match', () => {
    const { ctx, model, matchesFor } = setup();
    const second = matchesFor('sump')[1]!;
    const plan = planReplace([second], model, ctx, spec('sump', 'pool'));
    expect(plan.edits.map((e) => e.value)).toEqual(['sump pool']);
  });

  it('skips and counts matches in locked cells', () => {
    const { ctx, model, matchesFor } = setup({ frozen: ['1:0'] });
    // length 10 / 11 / 12: the frozen shot's length is locked
    const matches = matchesFor('1', { columnScope: 'custom', columns: ['length'] });
    const plan = planReplace(matches, model, ctx, spec('1', '2'));
    expect(plan.lockedMatches).toBe(1);
    expect(plan.lockedCells).toBe(1);
    expect(plan.edits.map((e) => [e.row.key, e.value])).toEqual([
      ['sh:1:100002', 22],
      ['sh:1:100003', 22],
    ]);
  });

  it('skips every match in a read-only workspace', () => {
    const { ctx, model, matchesFor } = setup({ editingBlocked: true });
    const plan = planReplace(matchesFor('sump'), model, ctx, spec('sump', 'x'));
    expect(plan.edits).toEqual([]);
    expect(plan.lockedMatches).toBe(4);
    expect(plan.lockedCells).toBe(3);
  });

  it('reports a replacement the column cannot parse', () => {
    const { ctx, model, matchesFor } = setup();
    const matches = matchesFor('10', { columnScope: 'custom', columns: ['length'] });
    const plan = planReplace(matches, model, ctx, spec('10', 'ten'));
    expect(plan.edits).toEqual([]);
    expect(plan.invalid).toEqual([{ rowKey: 'sh:1:100001', colKey: 'length', text: 'ten.0', matches: 1 }]);
  });

  it('leaves a cell alone when the text would not change', () => {
    const { ctx, model, matchesFor } = setup();
    const plan = planReplace(matchesFor('sump'), model, ctx, spec('sump', 'sump'));
    // "Sump pool" matched case-insensitively becomes "sump pool": a real change.
    expect(plan.edits.map((e) => [e.row.key, e.value])).toEqual([['st:1:1', 'sump pool']]);
    expect(plan.unchangedCells).toBe(2);
  });

  it('ignores matches for rows or columns that no longer exist', () => {
    const { ctx, model } = setup();
    const ghost: SearchMatch = { rowKey: 'sh:9:1', colKey: 'comment', start: 0, end: 1, lockReason: null };
    expect(planReplace([ghost], model, ctx, spec('s', 'x')).edits).toEqual([]);
  });
});
