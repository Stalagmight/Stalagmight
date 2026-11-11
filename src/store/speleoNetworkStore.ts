import { create } from 'zustand';
import type { SpeleoNetworkKind } from '../utils/speleoNetworkCommands';

/**
 * In-flight SpeleoDB network calls, counted at the IPC boundary (`invokeIpc`).
 *
 * Every call registers with {@link SpeleoNetworkStore.begin} before the IPC is
 * sent and ends when its promise settles, whether it resolved, rejected or was
 * abandoned by a caller that stopped awaiting it. The backend call keeps running
 * until it replies, so "abandoned" still counts as in flight until then.
 *
 * Counters, not booleans: overlapping calls (a status refresh during an upload,
 * two list refreshes) must not let the first one to finish clear the indicator
 * while the other is still talking to the server.
 */
export interface SpeleoNetworkStore {
  /** Every SpeleoDB network call awaiting its reply. Drives the busy indicator. */
  inFlight: number;
  /** The `exclusive` subset: lock, upload, download and account calls. Drives the
      UI gating that keeps a second such operation from being started. */
  exclusiveInFlight: number;
  /** Register one call. Returns its `end`, which is idempotent: a second call is a
      no-op, so a `finally` that runs twice cannot drive a counter below the
      calls that are really outstanding. */
  begin: (kind: SpeleoNetworkKind) => () => void;
  /** Test-only: forget every outstanding call. */
  reset: () => void;
}

export const useSpeleoNetworkStore = create<SpeleoNetworkStore>((set) => ({
  inFlight: 0,
  exclusiveInFlight: 0,
  begin: (kind) => {
    const exclusive = kind === 'exclusive' ? 1 : 0;
    set((s) => ({ inFlight: s.inFlight + 1, exclusiveInFlight: s.exclusiveInFlight + exclusive }));
    let ended = false;
    return () => {
      if (ended) return;
      ended = true;
      set((s) => ({
        inFlight: Math.max(0, s.inFlight - 1),
        exclusiveInFlight: Math.max(0, s.exclusiveInFlight - exclusive),
      }));
    };
  },
  reset: () => set({ inFlight: 0, exclusiveInFlight: 0 }),
}));

/** Any SpeleoDB network call is in flight. */
export const selectSpeleoNetworkBusy = (s: SpeleoNetworkStore): boolean => s.inFlight > 0;

/** A lock, upload, download or account call is in flight. */
export const selectSpeleoExclusiveBusy = (s: SpeleoNetworkStore): boolean => s.exclusiveInFlight > 0;

/**
 * Track one IPC promise when `kind` says it reaches SpeleoDB. The registration
 * happens synchronously, before the caller can observe the promise, so there is
 * no frame in which the call is running but not counted.
 */
export function trackSpeleoNetworkCall<T>(kind: SpeleoNetworkKind | null, call: () => Promise<T>): Promise<T> {
  if (kind === null) return call();
  const end = useSpeleoNetworkStore.getState().begin(kind);
  let pending: Promise<T>;
  try {
    pending = call();
  } catch (error) {
    end();
    throw error;
  }
  return pending.finally(end);
}
