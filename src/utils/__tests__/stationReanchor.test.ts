/**
 * The index maths that keeps a committed station pick on the same physical
 * station when the shots under it change.
 *
 * Both hosts (zero-datum config, the MNemo wizard) drive placement off the pick,
 * so an off-by-one here lands a GPS anchor or a segment merge on the wrong
 * station - silently, because the number itself stays in range.
 */
import { describe, it, expect } from 'vitest';
import { reanchorStation, reanchorAfterRunDelete } from '../stationReanchor';

describe('reanchorStation', () => {
  it('leaves a pick at or before the edit alone', () => {
    expect(reanchorStation(0, 2, 1, 6)).toBe(0);
    expect(reanchorStation(2, 2, 1, 6)).toBe(2);
    expect(reanchorStation(2, 2, -1, 4)).toBe(2);
  });

  it('shifts a pick past the edit by the whole delta', () => {
    expect(reanchorStation(3, 2, 1, 6)).toBe(4);
    expect(reanchorStation(5, 1, 3, 8)).toBe(8);
    expect(reanchorStation(4, 1, -1, 4)).toBe(3);
  });

  it('clamps to the surviving station range', () => {
    // Deleting the last shot pulls a pick on the final station back onto the new
    // final station rather than off the end.
    expect(reanchorStation(5, 4, -1, 4)).toBe(4);
    expect(reanchorStation(1, 0, -3, 0)).toBe(0);
    // Never negative, whatever the arithmetic says.
    expect(reanchorStation(0, 0, -5, 0)).toBe(0);
  });

  it('treats a zero delta as a no-op apart from the clamp', () => {
    expect(reanchorStation(3, 1, 0, 6)).toBe(3);
    expect(reanchorStation(9, 1, 0, 6)).toBe(6);
  });
});

describe('reanchorAfterRunDelete', () => {
  it('collapses a pick inside the removed run onto the run start', () => {
    // Stations 2..7 deleted with 5 picked: the pick has to land on 2, the
    // boundary the span closed up to. Plain reanchorStation would compute
    // 5 - 6 = -1 and clamp to the head of the section.
    expect(reanchorAfterRunDelete(5, 2, 6, 4)).toBe(2);
    expect(reanchorAfterRunDelete(3, 2, 6, 4)).toBe(2);
    // The last station OF the run is still inside it.
    expect(reanchorAfterRunDelete(8, 2, 6, 4)).toBe(2);
  });

  it('shifts a pick past the run by the run length', () => {
    expect(reanchorAfterRunDelete(9, 2, 6, 4)).toBe(3);
  });

  it('leaves a pick at or before the run start alone', () => {
    expect(reanchorAfterRunDelete(2, 2, 6, 4)).toBe(2);
    expect(reanchorAfterRunDelete(1, 2, 6, 4)).toBe(1);
  });

  it('agrees with reanchorStation for a single-row delete', () => {
    for (let pick = 0; pick <= 5; pick++) {
      expect(reanchorAfterRunDelete(pick, 2, 1, 4)).toBe(reanchorStation(pick, 2, -1, 4));
    }
  });
});
