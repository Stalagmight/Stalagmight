import { create } from 'zustand';
import { useProjectStore } from '../store/projectStore';

interface PendingReloadConfirm {
  /** The project the question is about; the dialog withdraws it if that project leaves. */
  path: string;
  resolve: (discard: boolean) => void;
}

/**
 * The SpeleoDB "Reload project" discard question, asked through Stalagmight's
 * own dialog ({@link SpeleoReloadConfirmDialog}) rather than the native confirm.
 *
 * Kept outside uiStore's single `activeDialog` slot on purpose: the question is
 * a step INSIDE the reload verb, which awaits the answer, so it must not evict
 * (or be evicted by) whatever dialog owns that slot - the same reason the
 * read-only notice lives beside it.
 */
export const useSpeleoReloadConfirm = create<{ pending: PendingReloadConfirm | null }>(() => ({
  pending: null,
}));

/**
 * Ask whether to discard local work and reload. Resolves `true` only on an
 * explicit "Discard changes"; every other ending (Cancel, Escape, the close
 * button, a superseding request, the project closing) resolves `false`, so the
 * caller keeps the work.
 */
export function confirmSpeleoReloadDiscard(): Promise<boolean> {
  const path = useProjectStore.getState().project?.file_path;
  if (!path) return Promise.resolve(false);
  useSpeleoReloadConfirm.getState().pending?.resolve(false);
  return new Promise<boolean>((resolve) => {
    useSpeleoReloadConfirm.setState({ pending: { path, resolve } });
  });
}

/** Settle the open question (no-op when none is open). */
export function answerSpeleoReloadConfirm(discard: boolean): void {
  const pending = useSpeleoReloadConfirm.getState().pending;
  if (!pending) return;
  useSpeleoReloadConfirm.setState({ pending: null });
  pending.resolve(discard);
}
