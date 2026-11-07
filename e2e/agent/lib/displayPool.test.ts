/**
 * Pinning the pool is what makes concurrent agent sessions safe, and it spends
 * every retry the default walk would have had - so a value that silently
 * shrinks the pool, or an entry that is not a display at all, has to be
 * rejected rather than handed to `startXvfb` as a shorter list than the caller
 * believes it passed. On a developer box the consequence of getting this wrong
 * is the fuzzer driving a VISIBLE app window on the desktop for the whole run.
 */
import { describe, expect, it } from 'vitest';

import { DEFAULT_XVFB_DISPLAYS, parseDisplayPool } from './displayPool';

describe('parseDisplayPool', () => {
  it('falls back to the five-display default when unset', () => {
    expect(parseDisplayPool(undefined)).toEqual(DEFAULT_XVFB_DISPLAYS);
    expect(DEFAULT_XVFB_DISPLAYS).toHaveLength(5);
  });

  it('takes a pinned pool, trimming whitespace', () => {
    expect(parseDisplayPool(':87')).toEqual([':87']);
    expect(parseDisplayPool(' :89 , :88 ')).toEqual([':89', ':88']);
  });

  it('rejects a value that filters down to nothing', () => {
    expect(() => parseDisplayPool('')).toThrow(/set but empty/);
    expect(() => parseDisplayPool(',, ')).toThrow(/set but empty/);
  });

  it('rejects anything that is not a :N display', () => {
    expect(() => parseDisplayPool('89')).toThrow(/malformed/);
    expect(() => parseDisplayPool(':89,localhost:0')).toThrow(/malformed/);
    expect(() => parseDisplayPool(':89,:8a')).toThrow(/malformed/);
  });

  it("counts the malformed entries on its first line and names them only below it (#576 W51)", () => {
    expect(() => parseDisplayPool(':89,qzCaveA,:8a')).toThrow(
      new Error('STALAGMIGHT_AGENT_DISPLAYS has 2 malformed display(s); entries must look like ":87"\n  malformed: ["qzCaveA",":8a"]'),
    );
  });
});
