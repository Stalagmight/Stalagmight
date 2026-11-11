import { describe, expect, it } from 'vitest';
import { raisedSectionOf } from '../raisedSection';

describe('raisedSectionOf', () => {
  it('is the selected section, or the section of the selected station or shot', () => {
    expect(raisedSectionOf('7', null, null)).toBe(7);
    expect(raisedSectionOf(null, '4:12', null)).toBe(4);
    expect(raisedSectionOf(null, null, '9:3')).toBe(9);
    expect(raisedSectionOf('0', null, null)).toBe(0); // section 0 exists
  });

  it('is null with nothing selected or a malformed key', () => {
    expect(raisedSectionOf(null, null, null)).toBeNull();
    expect(raisedSectionOf(null, ':1', null)).toBeNull();
    expect(raisedSectionOf(null, null, 'x:1')).toBeNull();
  });

  it('takes the section, then the station, then the shot', () => {
    expect(raisedSectionOf('1', '2:0', '3:0')).toBe(1);
    expect(raisedSectionOf(null, '2:0', '3:0')).toBe(2);
  });
});
