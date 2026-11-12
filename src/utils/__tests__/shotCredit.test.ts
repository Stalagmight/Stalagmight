import { describe, expect, it } from 'vitest';
import { creditOverride, effectiveCredit } from '../shotCredit';

const section = { surveyor: 'Bob', date: '2026-03-02' };

describe('effectiveCredit', () => {
  it('inherits both values when the shot has no credit', () => {
    expect(effectiveCredit({}, section)).toEqual(section);
  });

  it('inherits only the field the shot leaves absent', () => {
    expect(effectiveCredit({ credit: { surveyor: 'Ann' } }, section)).toEqual({
      surveyor: 'Ann',
      date: '2026-03-02',
    });
  });

  it('treats null as explicitly unknown, never falling back to the section', () => {
    expect(effectiveCredit({ credit: { surveyor: null, date: null } }, section)).toEqual({
      surveyor: null,
      date: null,
    });
  });
});

describe('creditOverride', () => {
  it('is null when the shot credits exactly what its section does', () => {
    expect(creditOverride({}, section)).toBeNull();
    expect(creditOverride({ credit: { surveyor: 'Bob' } }, section)).toBeNull();
  });

  it('returns the effective credit when it differs', () => {
    expect(creditOverride({ credit: { surveyor: 'Ann', date: '2026-04-11' } }, section)).toEqual({
      surveyor: 'Ann',
      date: '2026-04-11',
    });
  });
});
