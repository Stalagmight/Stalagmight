/**
 * save-under-churn: repeatedly mutate then save to a real temp path, asserting
 * the atomic-write invariants from docs/saving.md hold every time: isDirty flips
 * false on success and no leftover *.stalx.tmp remains beside the file. Finally
 * reopens the saved file to prove it round-trips clean.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { Scenario } from './framework';
import { ensureProjectLoaded, makeSection } from './common';

export const saveUnderChurn: Scenario = {
  name: 'save-under-churn',
  description: 'Mutate + save in a loop; assert isDirty=false, no leftover .tmp, clean reopen.',
  defaultIterations: 15,
  async run(ctx, { iterations }) {
    await ensureProjectLoaded(ctx);

    const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'stalagmight-save-churn-'));
    const savePath = path.join(dir, 'churn.stalx');
    try {
      const first = (await ctx.callAction('project', 'saveProject', [savePath])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string };
      };
      if (!first.ok || first.result?.success === false) {
        throw new Error(`initial save failed: ${first.result?.error ?? 'unknown'}`);
      }

      for (let i = 0; i < iterations; i++) {
        const created = (await ctx.callAction('project', 'createSection', [makeSection(i)])) as {
          ok: boolean;
          result?: { success?: boolean; error?: string };
        };
        if (!created.ok || created.result?.success === false) {
          throw new Error(`createSection ${i} failed: ${created.result?.error ?? 'unknown'}`);
        }
        const saved = (await ctx.callAction('project', 'saveProject', [savePath])) as {
          ok: boolean;
          result?: { success?: boolean; error?: string };
        };
        if (!saved.ok || saved.result?.success === false) {
          throw new Error(`save ${i} failed: ${saved.result?.error ?? 'unknown'}`);
        }
        ctx.metric('saves');

        const dirty = await ctx.eval<boolean>('return window.__E2E_TEST__.projectStore.getState().isDirty;');
        if (dirty) throw new Error(`isDirty still true after save ${i}`);

        const leftovers = fs.readdirSync(dir).filter((n) => n.endsWith('.stalx.tmp'));
        // Counted, not named: a leftover temp file's name is the app's text
        // (docs/agent-harness.md, #576 W51).
        if (leftovers.length) throw new Error(`leftover temp file(s) after save ${i}: ${leftovers.length}`);
      }

      // Reopen to prove the on-disk file round-trips.
      await ctx.callAction('project', 'closeProject', []);
      const reopened = (await ctx.callAction('project', 'loadProject', [savePath])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string };
      };
      if (!reopened.ok || reopened.result?.success === false) {
        throw new Error(`reopen failed: ${reopened.result?.error ?? 'unknown'}`);
      }
      const ok = await ctx.eval<boolean>('return !!window.__E2E_TEST__.projectStore.getState().project;');
      if (!ok) throw new Error('project null after reopen');
      ctx.log(`${iterations} mutate+save cycles clean; reopened OK`);
    } finally {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // Best-effort cleanup.
      }
    }
  },
};
