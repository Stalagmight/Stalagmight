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
 * [#576 FZ-FZ-INT-1] Switches run ONE AT A TIME: a load, create or close waits for
 * the previous switch's IPC to answer (its whole work, aftermath included) before it
 * sends its own, so the backend applies them in the order the user made them. Two in
 * flight at once could be applied in either order: a new load cancelled the previous
 * one's ANSWER, not its backend effect, and the store ended on one project while the
 * backend had the other open. Latest wins: a switch still waiting for its turn when
 * a newer one is requested is dropped (it resolves its `dropped` outcome), and the
 * one running is told (`onSuperseded`: a load cancels its backend load at once). A
 * switch with no `dropped` outcome (a delete, which the user confirmed) is never
 * dropped; it runs in its turn.
 *
 * [#576 FZ-FZ-INT-4] It also waits for the saves already sent (`savesSettled`).
 */
import { mutationsSettled } from '../store/mutationQueue';
import { noteSwitchRequested, savesSettled } from '../store/switchGate';

/** What a running switch's work can ask about its turn. */
export interface SwitchTurn {
  /**
   * True once a newer switch was requested while this one runs. Its committed answer
   * still applies (the backend has that project open: dropping it is what left the
   * store and the backend on different projects), but none of the rest of it should
   * speak: a failure, a prompt or a dialog is about a project nobody is opening now.
   */
  readonly superseded: boolean;
  /**
   * The switch's project is committed in the store (shown, editable): saves and edits
   * made from here are made for it, so the gate lets them through while the rest of
   * the work (the recent list, the undo state) runs. The next switch still waits for
   * all of it.
   */
  committed(): void;
}

export interface SwitchOptions<T> {
  /** What this switch resolves when it is dropped while waiting (latest wins). Absent: never dropped. */
  dropped?: () => T;
  /** Called once if a newer switch is requested while this one runs. */
  onSuperseded?: () => void;
  /**
   * Share the newest switch requested, while it has not settled, when it has the same
   * key: a load of the same path (a double click, a StrictMode re-mount) runs once.
   */
  dedupKey?: string;
}

interface QueuedSwitch {
  key: string | undefined;
  promise: Promise<unknown>;
  started: boolean;
  /** Drop it if it has not started and may be dropped; whether it was. */
  drop: () => boolean;
  supersede: () => void;
}

let replacementTail: Promise<void> = Promise.resolve();
let switchTail: Promise<void> = Promise.resolve();
/** Every switch requested and not yet settled, oldest first. */
const unsettled: QueuedSwitch[] = [];

export function runProjectSwitch<T>(
  work: (turn: SwitchTurn) => Promise<T>,
  options: SwitchOptions<T> = {},
): Promise<T> {
  const newest = unsettled[unsettled.length - 1];
  if (options.dedupKey !== undefined && newest?.key === options.dedupKey) return newest.promise as Promise<T>;

  const release = noteSwitchRequested();
  for (const earlier of [...unsettled]) if (!earlier.drop()) earlier.supersede();
  const before = Promise.all([replacementTail, mutationsSettled(), savesSettled(), switchTail]);

  let superseded = false;
  let dropped = false;
  let onSuperseded = options.onSuperseded;
  const turn: SwitchTurn = {
    get superseded() {
      return superseded;
    },
    committed: () => release(),
  };
  // Settled: out of the queue, and the gate released. Run BEFORE the switch's promise
  // resolves, so a caller that awaits the switch and then saves or edits is not refused.
  const settle = () => {
    const at = unsettled.indexOf(entry);
    if (at >= 0) unsettled.splice(at, 1);
    release();
  };
  let settleDropped!: (value: T) => void;
  const droppedOutcome = new Promise<T>((resolve) => {
    settleDropped = resolve;
  });
  const ran = before
    .then(async () => {
      if (dropped) return droppedOutcome;
      entry.started = true;
      try {
        return await work(turn);
      } finally {
        settle();
      }
    });
  const result = Promise.race([ran, droppedOutcome]);
  const entry: QueuedSwitch = {
    key: options.dedupKey,
    promise: result,
    started: false,
    drop: () => {
      if (entry.started || !options.dropped) return false;
      dropped = true;
      settle();
      settleDropped(options.dropped());
      return true;
    },
    supersede: () => {
      superseded = true;
      const notify = entry.started ? onSuperseded : undefined;
      if (notify) {
        onSuperseded = undefined;
        notify();
      }
    },
  };
  unsettled.push(entry);
  // The next switch waits for this one to settle: run, or dropped once it was its turn.
  switchTail = Promise.allSettled([switchTail, ran]).then(() => undefined);
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
