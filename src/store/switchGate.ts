/**
 * [#576 FZ-FZ-INT-4, FZ-FZ-INT-7] The frontend gate between project switches and the
 * saves and edits around them.
 *
 * A switch (load, create, close, delete - `utils/projectTransition.ts`) waits for the
 * saves already sent (`savesSettled`) and the edits already queued
 * (`mutationsSettled`) before it sends its own IPC, so each lands in the project it
 * was made for. A save or an edit REQUESTED once a switch has started is refused
 * (`switchUnderWay`) until every switch requested so far has settled, or committed
 * its project in the store (`SwitchTurn.committed`): before that, the store cannot
 * know which project such a request would reach.
 * The backend backs both up: every save and edit names the project it was made for
 * (`expectedProject`) and is refused with PROJECT_CHANGED when another is open.
 *
 * Plain module state, importing no store: read by the store's save and edit
 * chokepoints and by the switch coordinator without an import cycle.
 */
import { PROJECT_CHANGED_REFUSAL, refusedLocally, type CommandRefused } from './commandResult';

let switchesUnsettled = 0;

/** Whether a project switch has been requested and has not settled yet. */
export function switchUnderWay(): boolean {
  return switchesUnsettled > 0;
}

/**
 * The refusal of a save or an edit REQUESTED while a switch is under way (the
 * store's PROJECT_CHANGED text, no code), or null when none is.
 */
export function refuseDuringSwitch(): CommandRefused | null {
  return switchUnderWay() ? refusedLocally(PROJECT_CHANGED_REFUSAL) : null;
}

/** Called when a switch is requested; the returned function is called once when it settles (or is dropped). */
export function noteSwitchRequested(): () => void {
  switchesUnsettled += 1;
  let released = false;
  return () => {
    if (released) return;
    released = true;
    switchesUnsettled -= 1;
  };
}

const savesInFlight = new Set<Promise<unknown>>();

/** Track a save (its IPC and its aftermath) until it settles; returns it unchanged. */
export function trackSave<T>(save: Promise<T>): Promise<T> {
  savesInFlight.add(save);
  const forget = () => {
    savesInFlight.delete(save);
  };
  save.then(forget, forget);
  return save;
}

/**
 * A save as the store issues it: refused while a switch is under way
 * ({@link refuseDuringSwitch}), else started and tracked until it settles.
 */
export function issueSave<T>(save: () => Promise<T>): Promise<T | CommandRefused> {
  const refused = refuseDuringSwitch();
  return refused ? Promise.resolve(refused) : trackSave(save());
}

/**
 * Settles once every save tracked SO FAR has settled, whether it succeeded or not;
 * never rejects. Saves tracked later are not waited for.
 */
export function savesSettled(): Promise<void> {
  return Promise.allSettled([...savesInFlight]).then(() => undefined);
}
