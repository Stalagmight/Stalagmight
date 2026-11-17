/**
 * save-reopen-identity: the release golden flow for persistence (#17/#53 class).
 * Mutate, save to a temp path, close, reopen - the reopened project must be the
 * saved one. The comparison reuses the persistence oracle stack: canonical
 * snapshots, numeric tolerance, and the two DOCUMENTED families a save or a
 * reload may legitimately produce (`SAVE_HEALING_MUTATIONS`,
 * `RELOAD_RESOLVED_GEOMETRY`). Anything those families do not attribute is a
 * failure, and shot identity (by uuid) must survive exactly.
 */
import * as path from 'path';
import type { Journey } from './framework';
import { loadFixtureWithoutUndoHistory, disableAutoSave, makeLine, undoDepth } from '../scenarios/common';
import { canonicalProjectSnapshot } from '../lib/fuzzOracleCore';
import {
  classifyDiffs,
  diffSnapshots,
  describeDiffs,
  shotSetDelta,
  describeShotSetDelta,
  RELOAD_RESOLVED_GEOMETRY,
} from '../lib/projectSnapshot';

export const saveReopen: Journey = {
  name: 'save-reopen-identity',
  description: 'Mutate, save, close, reopen; the restored project must match the saved snapshot.',
  async run(ctx) {
    const { path: fixturePath } = await loadFixtureWithoutUndoHistory(ctx);
    await disableAutoSave(ctx);

    const created = (await ctx.callAction('project', 'createLine', [makeLine(466)])) as {
      ok: boolean;
      result?: { success?: boolean; error?: string | null };
    };
    if (!created.ok || created.result?.success === false) {
      throw new Error(`createLine failed: ${created.result?.error ?? 'unknown'}`);
    }
    const depthBefore = await undoDepth(ctx);

    const savePath = path.join(path.dirname(fixturePath), 'journey-save-reopen.stalx');
    const saved = (await ctx.callAction('project', 'saveProject', [savePath])) as {
      ok: boolean;
      result?: { success?: boolean; error?: string | null };
    };
    ctx.expect('save reported success', saved.ok && saved.result?.success === true,
      `error=${saved.result?.error ?? 'none'}`);

    // Snapshot AFTER the save: save-time heals are then already applied, so the
    // reopen comparison only has to absorb the reload's documented re-solve.
    const savedState = await ctx.ipc('get_project_state');
    const savedSnapshot = canonicalProjectSnapshot(savedState);
    await ctx.checkpoint('saved');

    const closed = (await ctx.callAction('project', 'closeProject', [])) as { ok: boolean };
    ctx.expect('project closed', closed.ok);

    const reopened = (await ctx.callAction('project', 'loadProject', [savePath])) as {
      ok: boolean;
      result?: { success?: boolean; error?: string | null };
    };
    ctx.expect('saved file reopened', reopened.ok && reopened.result?.success !== false,
      `error=${reopened.result?.error ?? 'none'}`);
    await ctx.checkpoint('reopened');

    const restoredState = await ctx.ipc('get_project_state');
    const restoredSnapshot = canonicalProjectSnapshot(restoredState);

    if (savedSnapshot !== restoredSnapshot) {
      const diffs = diffSnapshots(savedSnapshot, restoredSnapshot, 12);
      // ONLY the reload re-solve family is attributable here. The snapshot was
      // taken AFTER the save, so every save-time heal is already in the
      // expected state - allowing SAVE_HEALING_MUTATIONS across the reopen
      // would absorb exactly the divergence this comparison exists to catch
      // (e.g. a reload minting a uuid the saved file did not carry).
      const { unknown } = classifyDiffs(diffs, RELOAD_RESOLVED_GEOMETRY);
      ctx.expect('reopened state matches the saved snapshot (documented reload re-solve excepted)',
        unknown.length === 0, `\n${describeDiffs(unknown)}`);
    } else {
      ctx.expect('reopened state matches the saved snapshot (documented reload re-solve excepted)', true);
    }

    const identity = shotSetDelta(savedSnapshot, restoredSnapshot);
    ctx.expect('every shot survived the reopen (by uuid)', identity.length === 0,
      `\n${describeShotSetDelta(identity)}`);

    const depthAfter = await undoDepth(ctx);
    ctx.expect('undo history survived the reopen', depthAfter === depthBefore,
      `depth ${depthBefore} -> ${depthAfter}`);
  },
};
