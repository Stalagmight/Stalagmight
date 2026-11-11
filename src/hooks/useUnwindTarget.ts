import { useCallback, useEffect, useRef, useState } from 'react';
import { invokeIpc } from '../utils/ipc';
import { useProjectStore } from '../store/projectStore';
import { useSpeleoStore } from '../store/speleoStore';
import type { SpeleoRewindTarget, SpeleoUnwindTarget } from '../types/ipc.generated';

/**
 * [#303] Which revision a chain step would load - backward (`'back'`, the Unwind) or
 * forward (`'forward'`, "Redo into next SpeleoDB version").
 *
 * One hook for both directions because the fetch policy is the interesting part and it
 * is identical: fetched ON DEMAND (`enabled` / `resolve`), not on the status refresh, since
 * resolving either costs a `GET /revisions/` and the only thing that needs the answer is
 * the dropdown. `null` means there is nothing to reach in that direction -
 * backward: an empty repo, the root of the history, or a project whose undo history is
 * not pushed at all; forward: not mid-walk (which is how an EDIT removes the direction,
 * since a forward command drops the chain pointer), or already back at BASE. Either way
 * the caller renders no affordance rather than one that fails when pressed.
 *
 * Errors resolve to `null` deliberately: the target is an affordance, so being
 * offline or unauthenticated should HIDE the entry, not raise anything. A user who
 * presses the entry gets the real error from the action itself.
 */
export type ChainDirection = 'back' | 'forward';

/**
 * [#303] The resolved step, tagged with the direction it was asked for.
 *
 * A UNION, not an intersection. The two commands answer with different shapes -
 * `speleodb_unwind_target` has `is_current`/`discards_unpushed`, `speleodb_rewind_target`
 * has `is_latest` - and the old `SpeleoUnwindTarget & SpeleoRewindTarget` described a
 * value neither of them ever returns: it typed the other direction's field as a `boolean`
 * that was in fact `undefined` at runtime. Discriminating on the direction the hook was
 * asked for makes the reads narrow, so a caller cannot silently consult a field the
 * command it called never sent.
 */
export type ChainTarget =
  | ({ direction: 'back' } & SpeleoUnwindTarget)
  | ({ direction: 'forward' } & SpeleoRewindTarget);

/**
 * How long a resolved target stays good enough to reuse, in ms.
 *
 * [#303] The hover path resolves the target BEFORE opening the panel (it is the only way
 * to know whether there is anything to show), which puts a `GET /revisions/` behind a
 * 300ms hover of a disabled button - cheap per press, not cheap when a cursor crosses the
 * toolbar. The TTL is the agreed mitigation and is here from the start, not bolted on:
 * repeated hovers inside the window answer from the last fetch.
 */
const TARGET_TTL_MS = 5000;

/**
 * Stable identity for the open project, in the same shape `useMapTools` uses for its
 * along-line replies (#26): a distinct file - even a duplicate sharing created_at /
 * modified_at - and any edit yield a different key, so a reply issued for one project
 * cannot be stored against another.
 */
function projectIdentityKey(): string {
  const projectState = useProjectStore.getState();
  const speleoState = useSpeleoStore.getState();
  const project = projectState.project;
  return [
    projectState.projectTreeGeneration,
    project?.created_at ?? '',
    project?.modified_at ?? '',
    project?.file_path ?? '',
    speleoState.remoteProjectId ?? '',
    speleoState.sessionGeneration,
    speleoState.chainTargetGeneration,
  ].join('|');
}

export function useUnwindTarget(
  enabled: boolean,
  direction: ChainDirection = 'back',
): {
  target: ChainTarget | null;
  refresh: () => Promise<void>;
  resolve: () => Promise<ChainTarget | null>;
} {
  const [target, setTarget] = useState<ChainTarget | null>(null);
  // Only a remote project has revisions to walk; asking otherwise earns a refusal.
  const isRemote = useSpeleoStore((s) => s.projectLinkKind === 'remote');
  const remoteProjectId = useSpeleoStore((s) => s.remoteProjectId);
  const sessionGeneration = useSpeleoStore((s) => s.sessionGeneration);
  const chainTargetGeneration = useSpeleoStore((s) => s.chainTargetGeneration);
  const projectTreeGeneration = useProjectStore((s) => s.projectTreeGeneration);

  // The memo carries the inputs it was fetched for, so a change of direction, of
  // remote-ness, or OF THE PROJECT ITSELF invalidates it by not matching rather than by
  // needing to be cleared - no window in which a stale answer can be read back.
  //
  // The project identity is load-bearing and was missing: the two guards below sit on the
  // fetch, and `resolve` returns from this memo BEFORE reaching them. So for the whole TTL
  // after anything that moves the copy - a chain step, a push, a revision swap, none of
  // which unmount the dropdown because the project stays open - a hover answered with the
  // pre-swap hexsha, and `UndoHistoryDropdown` puts that straight into the confirmation.
  // Folding the identity in here means the memo cannot outlive what it describes, which is
  // better than repeating the guards at a second site that has to be kept in step.
  // Built on DEMAND, not at render: this hook subscribes to the speleo store, not the
  // project one, so a swap that moves the project does not re-render it - a key captured
  // during render would still name the old identity at the moment it matters most.
  const makeCacheKey = useCallback(
    () => `${direction}:${String(isRemote)}:${projectIdentityKey()}`,
    [
      direction,
      isRemote,
      projectTreeGeneration,
      remoteProjectId,
      sessionGeneration,
      chainTargetGeneration,
    ],
  );
  const cacheRef = useRef<{ at: number; key: string; value: ChainTarget | null } | null>(null);
  // Monotonic stamp for the in-flight fetch. `resolve` (hover) and `refresh` (panel open)
  // are two entry points into the same request, so a cursor that crosses the toolbar and
  // then opens the panel has BOTH running - and whichever the server answers last used to
  // win, target and memo alike. Only the newest request may store.
  const requestIdRef = useRef(0);

  const fetchTarget = useCallback(async (): Promise<ChainTarget | null> => {
    if (!isRemote) return null;
    try {
      if (direction === 'forward') {
        const raw = await invokeIpc<SpeleoRewindTarget | null>('speleodb_rewind_target');
        return raw ? { direction: 'forward', ...raw } : null;
      }
      const raw = await invokeIpc<SpeleoUnwindTarget | null>('speleodb_unwind_target');
      return raw ? { direction: 'back', ...raw } : null;
    } catch {
      // Offline / logged out / no permission: nothing to offer, and nothing to say.
      return null;
    }
  }, [isRemote, direction]);

  const remember = useCallback(
    (value: ChainTarget | null): ChainTarget | null => {
      cacheRef.current = { at: Date.now(), key: makeCacheKey(), value };
      setTarget(value);
      return value;
    },
    [makeCacheKey],
  );

  // One fetch, stamped and bound to the project it was issued for, storing only if it is
  // still the answer anyone asked for. Both guards cover the memo as well as `target`: a
  // stale value cached here outlives its own reply for the whole TTL, so guarding only
  // the setState would trade one wrong render for five seconds of them.
  const fetchNewest = useCallback(async (): Promise<ChainTarget | null> => {
    requestIdRef.current += 1;
    const requestId = requestIdRef.current;
    // Captured at request time: the backend binds the resulting action to whatever link is
    // open when it is PRESSED, so a hexsha resolved against a project the user has since
    // left describes a step that no longer exists.
    const requestProjectKey = projectIdentityKey();
    const value = await fetchTarget();
    if (requestId !== requestIdRef.current) return null;
    if (projectIdentityKey() !== requestProjectKey) return null;
    return remember(value);
  }, [fetchTarget, remember]);

  // Explicit refresh path for a panel that is already open and wants a current answer.
  const refresh = useCallback(async () => {
    await fetchNewest();
  }, [fetchNewest]);

  // The before-opening path: answer from the memo when it is fresh, so a cursor crossing
  // the toolbar cannot turn into one `GET /revisions/` per pass. Stores either way, so an
  // open panel renders what the caller decided to open on.
  const resolve = useCallback(async (): Promise<ChainTarget | null> => {
    const cached = cacheRef.current;
    if (cached && cached.key === makeCacheKey() && Date.now() - cached.at < TARGET_TTL_MS) {
      setTarget(cached.value);
      return cached.value;
    }
    // A discarded reply answers `null` rather than the value it fetched: the caller opens
    // the panel on this answer, and a step resolved for a project the user has left (or
    // superseded by a newer ask) is not something to open a panel on.
    return fetchNewest();
  }, [makeCacheKey, fetchNewest]);

  useEffect(() => {
    if (!enabled) {
      // Drop the stale answer on close so a re-open re-asks rather than rendering a
      // target that a push or lock in between may have moved.
      setTarget(null);
      return;
    }
    // Opening is normally preceded immediately by `resolve()` so the caller knows
    // whether there is anything to show. Reuse that project-bound TTL entry instead of
    // downloading and parsing the same revision a second time.
    void resolve();
  }, [enabled, resolve]);

  return { target, refresh, resolve };
}
