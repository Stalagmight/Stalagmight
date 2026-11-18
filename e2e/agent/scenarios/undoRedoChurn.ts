/**
 * undo-redo-churn: build N lines (undoable mutations), then undo all N and
 * redo all N, asserting the project state signature returns to the pre-churn
 * baseline after undo and to the post-mutation state after redo. Catches
 * undo/redo asymmetry and state corruption in the command stack.
 */
import type { Scenario } from './framework';
import { ensureProjectLoaded, makeLine, stateSignature } from './common';

export const undoRedoChurn: Scenario = {
  name: 'undo-redo-churn',
  description: 'Create N lines, then undo N + redo N; assert snapshot equality.',
  defaultIterations: 12,
  async run(ctx, { iterations }) {
    await ensureProjectLoaded(ctx);
    const base = await stateSignature(ctx);

    let created = 0;
    for (let i = 0; i < iterations; i++) {
      const res = (await ctx.callAction('project', 'createLine', [makeLine(i)])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string };
      };
      if (!res.ok || res.result?.success === false) {
        throw new Error(`createLine ${i} failed: ${res.result?.error ?? 'unknown'}`);
      }
      created++;
      ctx.metric('created');
    }
    const mutated = await stateSignature(ctx);
    if (mutated === base) throw new Error('creating lines did not change the state signature');

    for (let i = 0; i < created; i++) {
      await ctx.callAction('project', 'undo', []);
      ctx.metric('undos');
    }
    const afterUndo = await stateSignature(ctx);
    if (afterUndo !== base) {
      throw new Error(`state after ${created} undos does not match baseline\n  base=${base}\n  got =${afterUndo}`);
    }

    for (let i = 0; i < created; i++) {
      await ctx.callAction('project', 'redo', []);
      ctx.metric('redos');
    }
    const afterRedo = await stateSignature(ctx);
    if (afterRedo !== mutated) {
      throw new Error(`state after ${created} redos does not match post-mutation state\n  want=${mutated}\n  got =${afterRedo}`);
    }
    ctx.log(`churned ${created} create/undo/redo cycles with stable snapshots`);
  },
};
