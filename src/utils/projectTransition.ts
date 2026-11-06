/**
 * Coordinate whole-project switches with in-place SpeleoDB tree replacements.
 * Existing switches finish before a replacement starts; switches requested during
 * the replacement wait until its frontend reconciliation is complete.
 *
 * [#576 D23] A switch also waits for the edits queued BEFORE it
 * (`mutationsSettled`): edits run on their own queue (`runSerialized`), and one
 * still waiting there when the user opened another project used to reach the
 * backend after the switch and run against the new project, where the same small
 * section and shot ids mostly resolve. Edits issued DURING a switch are refused at
 * the store's chokepoint (`store/switchGate.ts`, [#576 FZ-FZ-INT-7]).
 *
 * [#576 FZ-FZ-INT-4] It also waits for the saves already sent (`savesSettled`).
 */
import { mutationsSettled } from '../store/mutationQueue';
import { noteSwitchRequested, savesSettled } from '../store/switchGate';

let replacementTail: Promise<void> = Promise.resolve();
let switchTail: Promise<void> = Promise.resolve();

/** What a running switch's work can tell the coordinator. */
export interface SwitchTurn {
  /**
   * The switch's project is committed in the store (shown, editable): saves and edits
   * made from here are made for it, so the gate lets them through while the rest of
   * the work (the recent list, the undo state) runs. The next switch still waits for
   * all of it.
   */
  committed(): void;
}

export function runProjectSwitch<T>(work: (turn: SwitchTurn) => Promise<T>): Promise<T> {
  // The gate (`switchGate.ts`) is released when the work says its project is committed,
  // or at the latest BEFORE the switch's promise resolves, so a caller that awaits the
  // switch and then saves or edits is not refused.
  const release = noteSwitchRequested();
  const before = Promise.all([replacementTail, mutationsSettled(), savesSettled()]);
  const result = before.then(async () => {
    try {
      return await work({ committed: release });
    } finally {
      release();
    }
  });
  switchTail = Promise.allSettled([switchTail, result]).then(() => undefined);
  return result;
}

/**
 * [#576 D23-SWAP] Invoke a backend verb that swaps the open survey tree IN PLACE
 * (SpeleoDB Update, start editing's fast-forward, discard, take remote, view or
 * restore a revision, an open or adopt that lands on the project already on
 * screen) only after the edits queued before it have finished - the wait a
 * project switch makes above. `runTreeReplacement` cannot make it: it wraps the
 * frontend reconciliation, which runs AFTER the backend has swapped, so an edit
 * still queued at that point reaches the backend after the swap and runs against
 * the new tree.
 *
 * The edits are the ones queued when this is CALLED, so call it where the action
 * is issued, before the action's first await. Edits queued later are not waited
 * for (a swap must never wait on work that may itself wait on it), and a failed
 * edit does not hold the swap: `mutationsSettled` never rejects.
 */
export function afterQueuedEdits<T>(verb: () => Promise<T>): Promise<T> {
  return mutationsSettled().then(verb);
}

export function runTreeReplacement<T>(work: () => Promise<T>): Promise<T> {
  const previousReplacement = replacementTail;
  const switchesAlreadyIssued = switchTail;
  let releaseReplacement!: () => void;
  const replacementFinished = new Promise<void>((resolve) => {
    releaseReplacement = resolve;
  });

  replacementTail = previousReplacement.then(() => replacementFinished);
  return (async () => {
    await previousReplacement;
    await switchesAlreadyIssued;
    try {
      return await work();
    } finally {
      releaseReplacement();
    }
  })();
}
