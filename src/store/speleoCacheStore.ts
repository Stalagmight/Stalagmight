import { create } from 'zustand';
import { invokeIpc } from '../utils/ipc';
import type { SpeleoCachedProject } from '../types/ipc.generated';

/**
 * The SpeleoDB copies cached on this machine, each classified by the backend
 * against the current session (`ownership`). The home screen lists the ones that
 * are not `other_account`; Settings' purge and the remove-copy prompt read the
 * dirty / unknown state from here.
 *
 * Purely local (no token, no network). Refreshed by whoever changes the answer:
 * the home screen on mount and when the session changes, the remove-copy prompt,
 * and the purge.
 */
interface SpeleoCacheState {
  copies: SpeleoCachedProject[];
  /** The first listing has settled (either way). */
  loaded: boolean;
  /** Re-read the listing. A reply older than the latest request is dropped. */
  refresh: () => Promise<SpeleoCachedProject[]>;
}

let requestSeq = 0;

export const useSpeleoCacheStore = create<SpeleoCacheState>((set, get) => ({
  copies: [],
  loaded: false,
  refresh: async () => {
    const request = ++requestSeq;
    try {
      const reply = await invokeIpc<SpeleoCachedProject[] | null>('speleodb_cached_projects', undefined, {
        purpose: 'list the SpeleoDB projects downloaded to this computer',
      });
      if (request !== requestSeq) return get().copies;
      const copies = reply ?? [];
      set({ copies, loaded: true });
      return copies;
    } catch {
      // Best effort: the home screen falls back to the server listing and recents.
      if (request === requestSeq) set({ loaded: true });
      return get().copies;
    }
  },
}));
