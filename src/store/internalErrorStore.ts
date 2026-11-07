import { create } from 'zustand';

/**
 * [#576 A11] Whether the backend has reported an internal error this session.
 *
 * The backend raises `internal-error` once, the first time a crash leaves one of its
 * locks poisoned: it recovered the data and carried on, and the user must be told
 * to save and restart (docs/notifications.md forbids the silent alternative). Set
 * once, never cleared - the condition lasts until the app restarts, which resets
 * this store with everything else.
 */
interface InternalErrorStore {
  /** True once the backend has reported an internal error. */
  raised: boolean;
  /** The lock the backend named (for the log; never shown to the user). */
  cell: string | null;
  raise: (cell: string) => void;
}

export const useInternalErrorStore = create<InternalErrorStore>((set) => ({
  raised: false,
  cell: null,
  raise: (cell) => set((state) => (state.raised ? state : { raised: true, cell })),
}));
