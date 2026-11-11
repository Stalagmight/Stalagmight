import { useSpeleoStore } from '../store/speleoStore';
import { selectSpeleoExclusiveBusy, useSpeleoNetworkStore } from '../store/speleoNetworkStore';

/**
 * True while a SpeleoDB operation that must not overlap another is running: a
 * hand-off verb holding the shared `busy` claim (start editing, reload, push,
 * done, take remote, discard, publish, switch to read-only), or any lock,
 * upload, download or account call counted at the IPC boundary - including the
 * ones that never claim `busy` (sign-in, sign-out, opening from SpeleoDB,
 * restoring a revision, releasing a held lock from the resume prompt).
 *
 * Controls that would start a second such operation, or change the SpeleoDB
 * endpoint or account under one, are disabled while this is true. Everything
 * else (map, panels, settings, editing) stays available.
 */
export function useSpeleoOperationBusy(): boolean {
  const verbBusy = useSpeleoStore((s) => s.busy);
  const networkBusy = useSpeleoNetworkStore(selectSpeleoExclusiveBusy);
  return verbBusy || networkBusy;
}

/** Non-hook read of {@link useSpeleoOperationBusy}, for guards inside callbacks. */
export function isSpeleoOperationBusy(): boolean {
  return useSpeleoStore.getState().busy || selectSpeleoExclusiveBusy(useSpeleoNetworkStore.getState());
}
