/**
 * Serialises frontend mutations, so their aftermath applies in ISSUE order (#24).
 *
 * `executeCommand` and `executeUndoRedo` are otherwise independent async chains: each awaits
 * its own IPC call and then its own `refreshProjectState`, so two overlapping operations can
 * finish in the opposite order to the one they were issued in. Their reverse aftermaths do NOT
 * commute when the counts differ - reverse at N, a shot-count edit, reverse at N+1 - so applying
 * them out of order walks the gas-plan pins and the distance-gradient origin onto a different
 * station, in range and silent.
 *
 * The backend already serialises on `command_lock`, so this costs no real concurrency: it makes
 * the frontend's completion order match the order the backend applied things in.
 *
 * The workspace mode rides the same queue (#576 RV2-FE-G1, `workspaceModeSync`): the backend
 * refuses survey edits while it is told Plan or Export, so an edit issued in Survey that is still
 * waiting here must reach it before a switch the user made after issuing it.
 *
 * Each entry runs whether or not its predecessor succeeded - a failed command must not wedge
 * every later one - and the chain deliberately holds no result, so a rejected promise cannot
 * escape into the next link.
 *
 * Its own module (not `projectStore.ts`) so the mode sync can share it without importing the
 * store.
 */
let mutationChain: Promise<void> = Promise.resolve();

export function runSerialized<T>(work: () => Promise<T>): Promise<T> {
  const result = mutationChain.then(work, work);
  mutationChain = result.then(
    () => undefined,
    () => undefined
  );
  return result;
}

/**
 * [#576 D23] Settles once every mutation queued SO FAR has finished, whether it
 * succeeded or not; never rejects. A project switch awaits it before it starts
 * (`runProjectSwitch`), so an edit the user issued on the old project lands there
 * instead of on the project the switch opens. Mutations queued after this call are
 * not waited for - a switch must never wait on work that may itself wait on it.
 */
export function mutationsSettled(): Promise<void> {
  return mutationChain;
}
