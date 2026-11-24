/**
 * RS-PROSE: the parts for sentences that word a name their own way. A
 * station's own name in a sentence that states its Station ID draws no
 * superscript ID and no line context; a reworded token keeps its drawing and
 * reads the sentence's words. Injection: copying `station_id` into
 * `ownStationPart` fails its first case.
 */
import { describe, expect, it } from 'vitest';
import { ownStationPart, reworded } from '../proseNameParts';
import { proseParts, proseToPlain, proseToken, type ProsePart } from '../prose';

describe('prose name parts', () => {
  it("draws a station's own name bare, in its home line's colour or neutral", () => {
    expect(ownStationPart('0', 2)).toEqual({ kind: 'station', text: '0', name: '0', line_id: 2 });
    expect(ownStationPart('21')).toEqual({ kind: 'station', text: '21', name: '21' });
  });

  it('rewords a token and keeps what its pill draws', () => {
    const part: ProsePart = { kind: 'station', text: '3a (Main)', name: '3a', line_id: 2, home_line: 1, station_id: 11 };
    const token = reworded(proseToken(part), '3a');
    expect(proseToPlain(token)).toBe('3a');
    expect(proseParts(token)).toEqual([{ ...part, text: '3a' }]);
    expect(reworded('words only', 'plain')).toBe('plain');
  });
});
