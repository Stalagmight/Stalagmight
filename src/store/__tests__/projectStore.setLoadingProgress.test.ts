/**
 * [#486] `setLoadingProgress` is the single write path shared by
 * `createProject`'s JSON `invoke()` result (result.loadingId), the binary load
 * result, `useLoadingProgress`'s event handler and the E2E
 * `simulateLoadingProgress` test hook (main.tsx). The ids used to be declared
 * `bigint` while every path delivered a plain JS number, so a store holding one
 * shape failed a staleness compare against the other (`2 !== 2n`) and dropped
 * live progress events. Since #576 C6 the generated types say `number`, which is
 * what the wire carries, so the store keeps the id as given and a numerically
 * equal id is the identical value.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import { useProjectStore } from '../projectStore';

beforeEach(() => {
  useProjectStore.setState((s) => ({
    loading: { ...s.loading, loadingId: 0, overallProgress: 0 },
  }));
});

describe('projectStore.setLoadingProgress - loadingId (#486, #576 C6)', () => {
  it('stores the plain-number loadingId the invoke() result delivers', () => {
    useProjectStore.getState().setLoadingProgress({ loadingId: 3 });

    expect(useProjectStore.getState().loading.loadingId).toBe(3);
    expect(typeof useProjectStore.getState().loading.loadingId).toBe('number');
  });

  it('a subsequent progress write with the same id is treated as matching, not stale', () => {
    // This is the exact #486 sequence: the result path writes first, then a
    // later progress write with the same id must read back as the identical
    // value, not fail an `===` compare a caller might run against it.
    useProjectStore.getState().setLoadingProgress({ loadingId: 3 });
    const afterResult = useProjectStore.getState().loading.loadingId;

    useProjectStore.getState().setLoadingProgress({ loadingId: 3, overallProgress: 55 });

    expect(useProjectStore.getState().loading.loadingId).toBe(afterResult);
    expect(useProjectStore.getState().loading.overallProgress).toBe(55);
  });
});
