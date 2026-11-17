/**
 * fractal-stress: build an L-system survey through the REAL store actions at
 * stress scale (#549). This absorbs the retired WDIO `fractal-data-generation`
 * tier: the same generator (`e2e/lib/fractal.ts`), driven through
 * `createLine` / `mergeStations` against the real backend, then verified
 * line-by-line and merge-by-merge.
 *
 * `iterations` here is the L-SYSTEM DEPTH, not a loop count: 2 is ~25 lines,
 * 3 is ~85, 4 is ~300+ (the old fractal-3/4 stress cases). Excluded from the
 * smoke gate - run it on request:
 *
 *   pnpm agent:scenario fractal-stress --iterations 4
 *
 * The run carries an explicit wall-clock budget
 * (`STALAGMIGHT_FRACTAL_BUDGET_MS`, default 180000) checked between phases and
 * during the line/merge loops, so a hung backend fails loudly with a phase
 * name instead of stalling the session.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { Scenario, ScenarioContext } from './framework';
import { hasProject } from './common';
import {
  buildActualMergeSet,
  buildExpectedMergeSet,
  executeLSystem,
  generateLSystem,
} from '../../lib/fractal';

const DEFAULT_BUDGET_MS = 180_000;

export const fractalStress: Scenario = {
  name: 'fractal-stress',
  description: 'L-system survey at stress scale through real store actions, budgeted.',
  // L-system depth 3 (~85 lines); pass --iterations 4 for the ~300-line tier.
  defaultIterations: 3,
  async run(ctx, { iterations }) {
    // Parse defensively: `Number("abc")` is NaN and every `elapsed > NaN` is
    // false, which would turn the fail-loudly budget into fail-open.
    const budgetEnv = Number(process.env.STALAGMIGHT_FRACTAL_BUDGET_MS);
    const budgetMs =
      Number.isFinite(budgetEnv) && budgetEnv > 0 ? budgetEnv : DEFAULT_BUDGET_MS;
    const start = Date.now();
    const checkBudget = (phase: string) => {
      const elapsed = Date.now() - start;
      if (elapsed > budgetMs) {
        throw new Error(
          `fractal-stress blew its ${budgetMs}ms budget during ${phase} (${elapsed}ms). ` +
            'Raise STALAGMIGHT_FRACTAL_BUDGET_MS only if the scale grew on purpose.',
        );
      }
    };

    const lSystem = generateLSystem(iterations);
    const { lines, merges } = executeLSystem(lSystem);
    const totalShots = lines.reduce((n, s) => n + s.shots.length, 0);
    ctx.log(
      `iter ${iterations}: ${lines.length} lines, ${totalShots} shots, ${merges.length} merges`,
    );

    if (await hasProject(ctx)) {
      await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
    }
    const created = (await ctx.callAction('project', 'createProject', [
      `Fractal Stress iter ${iterations}`,
    ])) as { ok: boolean; result?: { success?: boolean; error?: string | null } };
    if (!created.ok || created.result?.success === false) {
      throw new Error(`createProject failed: ${created.result?.error ?? 'unknown'}`);
    }
    checkBudget('project creation');

    for (const [index, line] of lines.entries()) {
      const res = (await ctx.callAction('project', 'createLine', [line])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string | null };
      };
      if (!res.ok || res.result?.success === false) {
        throw new Error(
          `createLine ${index} ("${line.name}") failed: ${res.result?.error ?? 'unknown'}`,
        );
      }
      if (index % 25 === 0) checkBudget(`line creation (${index}/${lines.length})`);
    }
    ctx.setMetric('lines', lines.length);
    ctx.setMetric('lineCreateMs', Date.now() - start);
    checkBudget('line creation');

    // Creation order is array order, so local fractal index -> backend id maps
    // off the project's line list - but assert the NAME at each position
    // matches before trusting it, or a mis-mapping would produce wrong merges
    // AND a matching expectation (the verify below uses this same map).
    const backendLines = await ctx.eval<{ id: number; name: string | null }[]>(
      'return window.__E2E_TEST__.projectStore.getState().project.lines.map((s) => ({ id: s.id, name: s.name }));',
    );
    if (backendLines.length !== lines.length) {
      throw new Error(
        `expected ${lines.length} lines in the store, found ${backendLines.length}`,
      );
    }
    const lineIdMap = new Map<number, number>();
    backendLines.forEach((backend, index) => {
      const expectedName = lines[index]?.name;
      if (backend.name !== expectedName) {
        throw new Error(
          `line order diverged at index ${index}: store has "${backend.name}", ` +
            `the generator made "${expectedName}" - the index->id map would lie`,
        );
      }
      lineIdMap.set(index, backend.id);
    });

    for (const [index, merge] of merges.entries()) {
      const a = {
        line_id: lineIdMap.get(merge.stationA.line_id),
        station_index: merge.stationA.station_index,
      };
      const b = {
        line_id: lineIdMap.get(merge.stationB.line_id),
        station_index: merge.stationB.station_index,
      };
      const res = (await ctx.callAction('project', 'mergeStations', [a, b])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string | null };
      };
      if (!res.ok || res.result?.success === false) {
        throw new Error(`mergeStations ${index} failed: ${res.result?.error ?? 'unknown'}`);
      }
      if (index % 25 === 0) checkBudget(`merges (${index}/${merges.length})`);
    }
    ctx.setMetric('merges', merges.length);
    checkBudget('merges');

    await verifyAgainstBackend(ctx, lines.length, merges, lineIdMap);
    checkBudget('backend verification');

    // The retired WDIO tier also SAVED the generated survey - serialization of
    // a large merge graph is its own failure surface. Write to a temp path,
    // require the file, and clean up (close first: deleting a live SQLite
    // database under the backend's open handle is the documented SIGBUS
    // hazard).
    const saveDir = fs.mkdtempSync(path.join(os.tmpdir(), 'fractal-stress-'));
    const savePath = path.join(saveDir, `fractal-iter${iterations}.stalx`);
    try {
      const saved = (await ctx.callAction('project', 'saveProject', [savePath])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string | null };
      };
      if (!saved.ok || saved.result?.success !== true) {
        throw new Error(`saveProject failed: ${saved.result?.error ?? 'unknown'}`);
      }
      if (!fs.existsSync(savePath)) {
        throw new Error(`save reported success but nothing landed at ${savePath}`);
      }
      ctx.setMetric('savedBytes', fs.statSync(savePath).size);
    } finally {
      await ctx.callAction('project', 'closeProject', []).catch(() => undefined);
      fs.rmSync(saveDir, { recursive: true, force: true });
    }
    checkBudget('save');
    ctx.setMetric('totalMs', Date.now() - start);
  },
};

async function verifyAgainstBackend(
  ctx: ScenarioContext,
  expectedLines: number,
  merges: ReturnType<typeof executeLSystem>['merges'],
  lineIdMap: Map<number, number>,
): Promise<void> {
  // The payload nests the project (`project.lines`, see fuzzOracleCore's
  // canonical key paths).
  const state = (await ctx.ipc('get_project_state')) as {
    project?: {
      lines?: unknown[];
      station_merges?: Array<{
        station_a: { line_id: number; station_index: number };
        station_b: { line_id: number; station_index: number };
      }>;
    };
  };
  const gotLines = state.project?.lines?.length ?? 0;
  if (gotLines !== expectedLines) {
    throw new Error(`backend holds ${gotLines} lines, expected ${expectedLines}`);
  }

  const expected = buildExpectedMergeSet(merges, lineIdMap);
  const actual = buildActualMergeSet(state.project?.station_merges ?? []);
  const missing = [...expected].filter((k) => !actual.has(k));
  const extra = [...actual].filter((k) => !expected.has(k));
  if (missing.length > 0 || extra.length > 0) {
    throw new Error(
      `merge sets diverge: ${missing.length} missing (${missing.slice(0, 3).join('; ')}) / ` +
        `${extra.length} unexpected (${extra.slice(0, 3).join('; ')})`,
    );
  }
}
