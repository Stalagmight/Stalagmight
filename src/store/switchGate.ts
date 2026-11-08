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
 * [#576 W84 x FZ-FZ-INT-1] Before an exit that leaves the project (Close, New Project,
 * opening a `.stalx`, the import-as-new hand-over) requests its switch, it waits for the
 * queued edits so its guard reads a settled `isDirty` (`settleEditsBeforeHandBack` in
 * `utils/quit.ts`). Two such exits can then be waiting at once, and both resume on the
 * same settled queue, oldest first. Latest wins there too: each exit takes a ticket
 * ({@link noteProjectExitRequested}) and one that a newer exit or a switch request has
 * overtaken ({@link projectExitSuperseded}) does nothing.
 *
 * Plain module state, importing no store: read by the store's save and edit
 * chokepoints and by the switch coordinator without an import cycle.
 */
import { PROJECT_CHANGED_REFUSAL, refusedLocally, type CommandRefused } from './commandResult';

/** Advanced by every project exit intent and every switch request: see {@link noteProjectExitRequested}. */
let projectExitIntents = 0;

/**
 * [#576 W84 x FZ-FZ-INT-1] Note that the user asked to leave the project (an exit that
 * waits before it requests its switch); the returned ticket names this request.
 */
export function noteProjectExitRequested(): number {
  projectExitIntents += 1;
  return projectExitIntents;
}

/** Whether a newer exit intent or a switch request was made since `ticket` was taken. */
export function projectExitSuperseded(ticket: number): boolean {
  return projectExitIntents !== ticket;
}

let switchesUnsettled = 0;
/** One per switch requested and not yet released (committed, settled or dropped). */
const undecided = new Set<Promise<void>>();

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

/**
 * Called when a switch is requested; the returned function is called once when it settles (or is dropped).
 * A switch request also overtakes every exit still waiting to request its own ({@link projectExitSuperseded}).
 */
export function noteSwitchRequested(): () => void {
  projectExitIntents += 1;
  switchesUnsettled += 1;
  let decide!: () => void;
  const decided = new Promise<void>((resolve) => {
    decide = resolve;
  });
  undecided.add(decided);
  let released = false;
  return () => {
    if (released) return;
    released = true;
    switchesUnsettled -= 1;
    undecided.delete(decided);
    decide();
  };
}

/**
 * [#576 W50-NF2] Settles once every switch requested SO FAR has committed its project
 * or settled (the moment the gate lets saves and edits through for it); never rejects.
 * Switches requested later are not waited for.
 */
export function switchesSoFarDecided(): Promise<void> {
  return Promise.all([...undecided]).then(() => undefined);
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
