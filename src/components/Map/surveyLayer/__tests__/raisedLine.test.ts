import { describe, expect, it } from 'vitest';
import { raisedLineOf } from '../raisedLine';

describe('raisedLineOf', () => {
  it('is the selected line, or the line of the selected station or shot', () => {
    expect(raisedLineOf('7', null, null)).toBe(7);
    expect(raisedLineOf(null, '4:12', null)).toBe(4);
    expect(raisedLineOf(null, null, '9:3')).toBe(9);
    expect(raisedLineOf('0', null, null)).toBe(0); // line 0 exists
  });

  it('is null with nothing selected or a malformed key', () => {
    expect(raisedLineOf(null, null, null)).toBeNull();
    expect(raisedLineOf(null, ':1', null)).toBeNull();
    expect(raisedLineOf(null, null, 'x:1')).toBeNull();
  });

  it('takes the line, then the station, then the shot', () => {
    expect(raisedLineOf('1', '2:0', '3:0')).toBe(1);
    expect(raisedLineOf(null, '2:0', '3:0')).toBe(2);
  });
});
