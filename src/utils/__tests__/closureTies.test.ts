import { describe, expect, it } from 'vitest';
import type { ClosureRef } from '../../types/ipc/ClosureRef';
import type { StationRef } from '../../types/project';
import { alignedClosureTargets, isClosureChainEntry } from '../closureTies';

const ref = (line_id: number, station_index: number): StationRef => ({ line_id, station_index });
const entry = (
  closure_id: bigint,
  target: StationRef,
  via: StationRef | null,
): ClosureRef => ({ closure_id, station_ref: target, via_station: via, is_transitive: via !== null });

/**
 * The entries are the wire's, as `survey/wire/stations.rs` builds them (pinned
 * there by `closure_references_follow_todays_rule_with_a_station_as_the_merge_group`
 * and `closure_chains_let_two_partners_of_one_slot_see_each_other`).
 *
 * X = 1:1, Y = 2:1, Z = 3:1. Closure 7 joins X and Y (unaligned), closure 8
 * joins X and Z (aligned). X holds both directly, so Y sees Z "via X" under 8
 * and Z sees Y "via X" under 7: the closure-chain entries.
 */
const X = ref(1, 1);
const Y = ref(2, 1);
const Z = ref(3, 1);
const onX = [entry(7n, Y, null), entry(8n, Z, null)];
const onY = [entry(7n, X, null), entry(8n, Z, X)];
const onZ = [entry(8n, X, null), entry(7n, Y, X)];

describe('isClosureChainEntry', () => {
  it('a direct entry ties its holder', () => {
    expect(isClosureChainEntry(X, [], onX, onX[1]!)).toBe(false);
    expect(isClosureChainEntry(Y, [], onY, onY[0]!)).toBe(false);
  });

  it('an entry that reaches a third station through a partner is a chain entry', () => {
    expect(isClosureChainEntry(Y, [], onY, onY[1]!)).toBe(true);
    expect(isClosureChainEntry(Z, [], onZ, onZ[1]!)).toBe(true);
  });

  it("a junction's other slot is tied through the end's own slot, even when that slot is hidden", () => {
    // J is a junction with slots 4:0 (the closure end, say on a hidden line)
    // and 5:0. Closure 9 joins J and W = 6:2. On 5:0 the entry reaches W via
    // 4:0, which only `merged_with` says is the same station.
    const J = ref(4, 0);
    const J2 = ref(5, 0);
    const W = ref(6, 2);
    const onJ2 = [entry(9n, W, J)];
    expect(isClosureChainEntry(J2, [J], onJ2, onJ2[0]!)).toBe(false);
    // Without the junction it would be a third station: a chain entry.
    expect(isClosureChainEntry(J2, [], onJ2, onJ2[0]!)).toBe(true);
  });

  it("the far station's other slots are tied through the far end", () => {
    // Closure 9 joins A = 1:3 and W = 6:2, where W is a junction with 7:0.
    // A lists W directly and 7:0 via W; a slot of A's own junction (8:0) lists
    // W via A and 7:0 via W.
    const A = ref(1, 3);
    const A2 = ref(8, 0);
    const W = ref(6, 2);
    const W2 = ref(7, 0);
    const onA = [entry(9n, W, null), entry(9n, W2, W)];
    expect(isClosureChainEntry(A, [A2], onA, onA[1]!)).toBe(false);
    const onA2 = [entry(9n, W, A), entry(9n, W2, W)];
    expect(isClosureChainEntry(A2, [A], onA2, onA2[0]!)).toBe(false);
    expect(isClosureChainEntry(A2, [A], onA2, onA2[1]!)).toBe(false);
  });
});

describe('alignedClosureTargets', () => {
  const aligned = new Set([8n]);

  it('follows an aligned closure the holder is an end of', () => {
    expect(alignedClosureTargets(X, [], onX, aligned)).toEqual([Z]);
    expect(alignedClosureTargets(Z, [], onZ, aligned)).toEqual([X]);
  });

  it("does not follow the aligned closure of a partner's other tie (plan D15)", () => {
    // Y is tied to X only by the unaligned 7; 8 ties X and Z, not Y.
    expect(alignedClosureTargets(Y, [], onY, aligned)).toEqual([]);
  });

  it('follows nothing when no closure is aligned or there are no entries', () => {
    expect(alignedClosureTargets(X, [], onX, new Set())).toEqual([]);
    expect(alignedClosureTargets(X, [], undefined, aligned)).toEqual([]);
  });
});
