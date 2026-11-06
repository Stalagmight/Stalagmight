import { useProjectStore } from '../store/projectStore';
import type { GasConfig } from '../types/ipc.generated';

/**
 * The one path Plan-mode controls persist gas fields through (#317 review round).
 *
 * Two things went wrong with per-caller writers, and this module exists to make both
 * structurally impossible:
 *
 * - `PositiveNumberField` commits on every valid CHANGE, not on blur - so an un-debounced
 *   writer turns "type 100" into three `update_config` IPCs, three config reloads, and three
 *   LSQ-solve cache drops. Every queued write here is trailing-debounced.
 * - Two independent writers clobber each other across the async round trip: `updateConfig`
 *   only refreshes `config` after its IPC + reload complete, so a second writer that reads
 *   `getState().config` inside that window merges against the pre-write snapshot and silently
 *   reverts the first writer's field. Here every write joins one promise chain and reads the
 *   saved config at EXECUTION time, after the previous write has fully landed, and patches
 *   queued while a write is pending are merged into a single later write. (The write now
 *   carries only the queued fields - see below - so nothing is read back into it.)
 *
 * The chaining above only ever solved that for gas-vs-gas. Against a writer in ANOTHER
 * module it did nothing, because the payload was a whole `Config` spread from this store's
 * snapshot: a data-table column resize (500 ms debounce) landing inside a gas field's 450 ms
 * debounce was overwritten by this module's stale copy of `panels`, and silently - the open
 * table keeps rendering its own state and only reverts on the next launch. So the write now
 * goes through `updateGasConfig`, which posts ONLY the `[gas]` block (#357 item 2). Ordering
 * could never have fixed that; not sending the other fields is what does.
 *
 * [#576 FZ-INT-3-D1] The same held one level down: the whole `[gas]` block, spread from this
 * store's copy, put back a `[gas]` field Settings had just saved (its patch landed, its
 * re-read had not, or landed after this write was built). So the write now carries only the
 * fields queued here, each as its own `[gas, field]` patch - the way Settings saves - and a
 * field this module never touched cannot be reverted by it.
 *
 * Fire-and-forget by design (a failed write logs inside the store action and keeps the working
 * copy authoritative for the session) - same contract the swim-speed writer always had.
 */
const WRITE_DEBOUNCE_MS = 450;

let pending: Partial<GasConfig> | null = null;
let timer: ReturnType<typeof setTimeout> | null = null;
let chain: Promise<void> = Promise.resolve();

function fire(): void {
  const patch = pending;
  pending = null;
  timer = null;
  if (!patch) return;
  chain = chain.then(async () => {
    const store = useProjectStore.getState();
    if (!store.config) return;
    await store.updateGasConfig(patch);
  });
}

/** Queue a gas-field patch for the saved config; newer patches to the same field win. */
export function queueGasConfigWrite(patch: Partial<GasConfig>): void {
  pending = { ...pending, ...patch };
  if (timer) clearTimeout(timer);
  timer = setTimeout(fire, WRITE_DEBOUNCE_MS);
}

/**
 * Write anything still queued immediately (component unmount - a value committed within the
 * debounce window right before leaving Plan mode must still reach disk). Returns a promise
 * that resolves once every write issued so far has fully landed, so tests can await it.
 */
export function flushGasConfigWrite(): Promise<void> {
  if (timer) clearTimeout(timer);
  fire();
  return chain;
}
