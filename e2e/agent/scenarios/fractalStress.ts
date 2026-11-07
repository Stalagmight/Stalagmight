/**
 * fractal-stress: build an L-system survey through the REAL store actions at
 * stress scale (#549). This absorbs the retired WDIO `fractal-data-generation`
 * tier: the same generator (`e2e/lib/fractal.ts`), driven through
 * `createSection` / `mergeStations` against the real backend, then verified
 * section-by-section and merge-by-merge.
 *
 * `iterations` here is the L-SYSTEM DEPTH, not a loop count: 2 is ~25 sections,
 * 3 is ~85, 4 is ~300+ (the old fractal-3/4 stress cases). Excluded from the
 * smoke gate - run it on request:
 *
 *   pnpm agent:scenario fractal-stress --iterations 4
 *
 * The run carries an explicit wall-clock budget
 * (`STALAGMIGHT_FRACTAL_BUDGET_MS`, default 180000) checked between phases and
 * during the section/merge loops, so a hung backend fails loudly with a phase
 * name instead of stalling the session.
 */
import * as fs from 'fs';
import * as os from 'os';
import * as path from 'path';
import type { Scenario, ScenarioContext } from './framework';
import { hasProject } from './common';
import { withheldAppText } from '../lib/shareableFailure';
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
  // L-system depth 3 (~85 sections); pass --iterations 4 for the ~300-section tier.
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
    const { sections, merges } = executeLSystem(lSystem);
    const totalShots = sections.reduce((n, s) => n + s.shots.length, 0);
    ctx.log(
      `iter ${iterations}: ${sections.length} sections, ${totalShots} shots, ${merges.length} merges`,
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

    for (const [index, section] of sections.entries()) {
      const res = (await ctx.callAction('project', 'createSection', [section])) as {
        ok: boolean;
        result?: { success?: boolean; error?: string | null };
      };
      if (!res.ok || res.result?.success === false) {
        throw new Error(
          `createSection ${index} ("${section.name}") failed: ${res.result?.error ?? 'unknown'}`,
        );
      }
      if (index % 25 === 0) checkBudget(`section creation (${index}/${sections.length})`);
    }
    ctx.setMetric('sections', sections.length);
    ctx.setMetric('sectionCreateMs', Date.now() - start);
    checkBudget('section creation');

    // Creation order is array order, so local fractal index -> backend id maps
    // off the project's section list - but assert the NAME at each position
    // matches before trusting it, or a mis-mapping would produce wrong merges
    // AND a matching expectation (the verify below uses this same map).
    const backendSections = await ctx.eval<{ id: number; name: string | null }[]>(
      'return window.__E2E_TEST__.projectStore.getState().project.sections.map((s) => ({ id: s.id, name: s.name }));',
    );
    if (backendSections.length !== sections.length) {
      throw new Error(
        `expected ${sections.length} sections in the store, found ${backendSections.length}`,
      );
    }
    const sectionIdMap = new Map<number, number>();
    backendSections.forEach((backend, index) => {
      const expectedName = sections[index]?.name;
      if (backend.name !== expectedName) {
        // The store's name is the app's text, so it prints as its length (#576 W51).
        throw new Error(
          `section order diverged at index ${index}: store has ${backend.name === null ? 'no name' : withheldAppText(backend.name)}, ` +
            `the generator made "${expectedName}" - the index->id map would lie`,
        );
      }
      sectionIdMap.set(index, backend.id);
    });

    for (const [index, merge] of merges.entries()) {
      const a = {
        section_id: sectionIdMap.get(merge.stationA.section_id),
        station_index: merge.stationA.station_index,
      };
      const b = {
        section_id: sectionIdMap.get(merge.stationB.section_id),
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

    await verifyAgainstBackend(ctx, sections.length, merges, sectionIdMap);
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
  expectedSections: number,
  merges: ReturnType<typeof executeLSystem>['merges'],
  sectionIdMap: Map<number, number>,
): Promise<void> {
  // The payload nests the project (`project.sections`, see fuzzOracleCore's
  // canonical key paths).
  const state = (await ctx.ipc('get_project_state')) as {
    project?: {
      sections?: unknown[];
      station_merges?: Array<{
        station_a: { section_id: number; station_index: number };
        station_b: { section_id: number; station_index: number };
      }>;
    };
  };
  const gotSections = state.project?.sections?.length ?? 0;
  if (gotSections !== expectedSections) {
    throw new Error(`backend holds ${gotSections} sections, expected ${expectedSections}`);
  }

  const expected = buildExpectedMergeSet(merges, sectionIdMap);
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
