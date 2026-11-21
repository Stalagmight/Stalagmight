/**
 * N-13: a list of lines names them, up to three, then counts the rest, as
 * the undo history does (`labels::listed`). Injection: returning the first
 * name and a count ("Upstream and 1 more") fails the two- and three-name
 * cases.
 */
import { describe, expect, it } from 'vitest';
import i18n from '../../i18n';
import { listNames } from '../listNames';

const t = i18n.t.bind(i18n);

describe('listNames', () => {
  it('names up to three, then counts the rest', () => {
    expect(listNames([], t)).toBe('');
    expect(listNames(['Upstream'], t)).toBe('Upstream');
    expect(listNames(['Upstream', 'Upstream Side'], t)).toBe('Upstream and Upstream Side');
    expect(listNames(['A', 'B', 'C'], t)).toBe('A, B and C');
    expect(listNames(['A', 'B', 'C', 'D'], t)).toBe('A, B, C and 1 more');
    expect(listNames(['A', 'B', 'C', 'D', 'E'], t)).toBe('A, B, C and 2 more');
  });
});
