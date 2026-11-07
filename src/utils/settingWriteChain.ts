/**
 * [#576 SETTINGS-ORDER] Writes of one setting land in the order they were made.
 *
 * The config and display-setting commands run on the backend's blocking pool, so
 * two IPCs in flight at once are not applied in the order they were sent. A
 * setting the frontend persists without waiting - a quick double click on a grid
 * spacing, a geometry-mode toggle, a panel divider dragged twice, a column resize
 * flushed on unmount - could have its EARLIER value land last and be what the
 * next launch reads. Each write here waits for the previous write of the same key
 * to settle (succeeded or failed) before it starts, so the last one made is the
 * last one applied. Different keys stay independent: the config writers apply
 * each write to its own fields under one lock (`modify_config`), and a display
 * setting is one row of the open project's database, written under the command
 * lock (`save_display_setting`, not `modify_config`; per project - see
 * `chainProjectSettingWrite`), so only same-key order matters.
 *
 * [#576 RV6b-3] A config RE-READ after a write belongs in the config chain
 * (`reloadConfigInOrder`), not in the writer's own: two re-reads in flight answer
 * in either order, and one issued before a settings save landed could otherwise
 * answer after that save's re-read and put a config without it back.
 *
 * The gas-planning fields are not routed through here: `utils/gasConfigDisk.ts`
 * is their one writer and already serializes (and coalesces) its own writes.
 */

import { openProjectIdentity, projectSessionEpoch } from '../store/projectSession';
import { switchesSoFarDecided } from '../store/switchGate';
import type { DisplaySettingTarget } from '../types/ipc.generated';

const tails = new Map<string, Promise<void>>();

/** Every project-setting write queued so far, numbered, and the newest one a timed-out flush dropped. */
let projectWritesQueued = 0;
let droppedUpTo = 0;
/**
 * [#576 W50-NF2] Set from the start of a switch's flush until the switches requested
 * by then have decided ({@link switchesSoFarDecided}): it settles at that moment.
 */
let flushedSwitchesDecided: Promise<void> | null = null;
/** The chain keys `chainProjectSettingWrite` has used. */
const projectSettingKeys = new Set<string>();
/** Per chain key: the number of the newest write made, and of the newest one sent. */
const lastQueued = new Map<string, number>();
const lastSent = new Map<string, number>();
/**
 * [#576 Q-W22S-1-D1, Q-W44-1] Per chain key: the writes a flush gave up on (the
 * key's tail when its bound ran out), until they settle, and the newest write a
 * flush dropped there. A later flush does not wait for such a key again: it drops
 * the writes queued behind it at once, as the bound would.
 */
const stuck = new Map<string, Promise<void>>();
const droppedUpToByKey = new Map<string, number>();

/**
 * Run `write` after every earlier write chained under `key` has settled, and
 * return its result. A failed write does not block the ones queued after it.
 */
export function chainSettingWrite<T>(key: string, write: () => Promise<T>): Promise<T> {
  const previous = tails.get(key) ?? Promise.resolve();
  const result = previous.then(write);
  const tail = result.then(
    () => undefined,
    () => undefined,
  );
  tails.set(key, tail);
  void tail.then(() => {
    if (tails.get(key) === tail) tails.delete(key);
  });
  return result;
}

/**
 * [#576 RV6b-2] {@link chainSettingWrite} for a setting stored IN the open project
 * (grid spacing, the geometry mode, the display toggles), which the backend writes
 * into whichever project is open when the command runs. A write queued behind
 * another used to reach the backend after the user opened another project, and
 * land there. Now `write` gets the identity of the project open when the write was
 * MADE (its `created_at` and the file it is open from, #576 NF-W8FE-3), to send for
 * the backend to check (`save_display_setting` refuses another project's - or
 * another copy's - with PROJECT_CHANGED), and a write whose turn comes after the
 * project was replaced by another one (`store/projectSession.ts`) is dropped
 * unsent: it resolves `undefined`.
 *
 * [#576 RV8b-3] Replaced by ANOTHER one: the rule is "a write lands in the project
 * it was made in", the backend's own. A reload of the same file (a dialog's
 * reopen, a SpeleoDB pull that reloads the copy) starts a new session but opens
 * the project the write was made in, so the write is still sent - as one already
 * in flight would land. Only a write made with no project open falls back to the
 * session, because "no project" names nothing to compare.
 *
 * [#576 W50-NF2] A write MADE once a load's or a close's flush has started, before
 * that switch decided (committed its project, or settled without), is not one the
 * flush covers: queued behind a write the flush gave up on, or sent while the load
 * runs, it could land after a reload of the same file and in the reopened session.
 * It waits for the switch to decide, as the store's switch gate holds saves and
 * edits (`store/switchGate.ts`), and is dropped unsent when a new session started
 * meanwhile; when none did (the switch failed or was dropped), it is sent.
 */
export function chainProjectSettingWrite<T>(
  key: string,
  write: (madeFor: DisplaySettingTarget | null) => Promise<T>,
): Promise<T | undefined> {
  const session = projectSessionEpoch();
  const madeFor = openProjectIdentity();
  const madeDuringSwitch = flushedSwitchesDecided;
  projectWritesQueued += 1;
  const queued = projectWritesQueued;
  projectSettingKeys.add(key);
  lastQueued.set(key, queued);
  return chainSettingWrite(key, async () => {
    if (madeDuringSwitch) await madeDuringSwitch;
    if (queued <= droppedUpTo || queued <= (droppedUpToByKey.get(key) ?? 0)) return undefined;
    if (madeDuringSwitch && session !== projectSessionEpoch()) return undefined;
    if (!stillTheProjectItWasMadeIn(madeFor, session)) return undefined;
    lastSent.set(key, queued);
    return write(madeFor);
  });
}

/**
 * [#576 FZ-FZ-INT-6] How long a project switch (a load or a close) waits for the
 * project-setting writes queued before it ({@link flushProjectSettingWrites}). A
 * write normally lands in milliseconds; the bound only keeps a wedged command from
 * stalling the switch.
 */
export const PROJECT_SETTING_FLUSH_TIMEOUT_MS = 2000;

/** What {@link flushProjectSettingWrites} left behind. */
export interface SettingFlushOutcome {
  /**
   * [#576 Q-W22S-1-D1] The chain keys it dropped a write of (one still queued, unsent,
   * when it gave up): their value on screen may not be what is stored. Empty when
   * every write queued before it settled in time.
   */
  dropped: string[];
  /** Settles once the writes it gave up on have settled (at once when there are none). */
  givenUpSettled: Promise<void>;
}

/**
 * [#576 FZ-FZ-INT-6] Settle the project-setting writes queued so far, before a load
 * or a close replaces the project they were made in. Without it a write queued
 * behind another of its key could be sent after a close and a reload of the SAME
 * file - the same `created_at` and file, so the identity rule above took it as the
 * project it was made in, and the backend stored a value the reopened session did
 * not show. Each write now lands in its own session.
 *
 * Bounded by {@link PROJECT_SETTING_FLUSH_TIMEOUT_MS}: a write still QUEUED (not yet
 * sent) when the bound runs out - one made during the wait included - is dropped
 * unsent; one already sent lands as it would have. With nothing queued it returns at once and arms no timer.
 *
 * [#576 Q-W22S-1-D1, Q-W44-1] The writes it gave up on are cleared from later
 * flushes: one made later still queues behind them (same-key order holds), but the
 * next flush drops it at once instead of waiting the full bound for them again.
 */
export function flushProjectSettingWrites(timeoutMs = PROJECT_SETTING_FLUSH_TIMEOUT_MS): Promise<SettingFlushOutcome> {
  // [#576 W50-NF2] From here until the switch decides, a write made waits for it.
  const decided = switchesSoFarDecided();
  flushedSwitchesDecided = decided;
  void decided.then(() => {
    if (flushedSwitchesDecided === decided) flushedSwitchesDecided = null;
  });
  const pending = [...projectSettingKeys].flatMap((key) => {
    const tail = tails.get(key);
    return tail ? [{ key, tail }] : [];
  });
  // A key a flush already gave up on: drop what is queued behind it now, without waiting.
  const stillStuck = pending.filter(({ key }) => stuck.has(key));
  const dropped = stillStuck.filter(({ key }) => hasUnsentWrite(key)).map(({ key }) => key);
  for (const { key } of stillStuck) droppedUpToByKey.set(key, lastQueued.get(key) ?? 0);
  const waitFor = pending.filter(({ key }) => !stuck.has(key));
  const stuckSettled = Promise.all(stillStuck.map(({ key }) => stuck.get(key))).then(() => undefined);
  if (waitFor.length === 0) return Promise.resolve({ dropped, givenUpSettled: stuckSettled });
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timedOut = new Promise<boolean>((resolve) => {
    timer = setTimeout(() => resolve(true), timeoutMs);
  });
  const flushed = Promise.all(waitFor.map(({ tail }) => tail)).then(() => false);
  return Promise.race([flushed, timedOut]).then((outOfTime) => {
    clearTimeout(timer);
    if (!outOfTime) return { dropped, givenUpSettled: stuckSettled };
    // [#576 RVF-c-A2-3] Every write queued AT the timeout, those made during the wait
    // included: one made then queues behind the wedged write too, and would be sent
    // after the switch committed (a reload of the same file takes it).
    droppedUpTo = Math.max(droppedUpTo, projectWritesQueued);
    const givenUp: Promise<void>[] = [];
    for (const key of projectSettingKeys) {
      const tail = tails.get(key);
      if (!tail || stuck.has(key)) continue;
      if (hasUnsentWrite(key)) dropped.push(key);
      stuck.set(key, tail);
      givenUp.push(tail);
      void tail.then(() => {
        if (stuck.get(key) === tail) stuck.delete(key);
      });
    }
    return { dropped, givenUpSettled: Promise.all([stuckSettled, ...givenUp]).then(() => undefined) };
  });
}

/** Whether a write of `key` was made after the newest one sent: queued, or dropped unsent. */
function hasUnsentWrite(key: string): boolean {
  return (lastQueued.get(key) ?? 0) > (lastSent.get(key) ?? 0);
}

/**
 * [#576 Q-W22S-1-D1] Whether the newest write of `key` made so far was dropped by a
 * flush, unsent: the value on screen is that write's, and not what is stored.
 */
export function droppedWriteShown(key: string): boolean {
  const newest = lastQueued.get(key) ?? 0;
  return newest > (lastSent.get(key) ?? 0) && (newest <= droppedUpTo || newest <= (droppedUpToByKey.get(key) ?? 0));
}

/**
 * Whether the project open now is the one a write was made in (see above): the
 * same `created_at`, and either the same file or the same session. The session
 * covers a Save As and a never-saved project's first save, which move the open
 * project to another file without a new open (#576 RV9a-2) - the backend still
 * takes such a write (`opened_from`). A switch to another copy is a new session
 * on another file, so it is still dropped.
 */
function stillTheProjectItWasMadeIn(madeFor: DisplaySettingTarget | null, session: number): boolean {
  const open = openProjectIdentity();
  const sameSession = session === projectSessionEpoch();
  if (madeFor === null || open === null) return madeFor === open && sameSession;
  return open.createdAt === madeFor.createdAt && (open.filePath === madeFor.filePath || sameSession);
}

/** Keys for the writers that share a command but not a setting. */
export const SETTING_KEYS = {
  displaySetting: (key: string) => `display-setting:${key}`,
  panelSize: (field: string) => `panel-size:${field}`,
  panelLayout: 'panel-layout',
  /** Every `patch_config` / `update_config` save, Settings dialog included. */
  config: 'config',
} as const;
