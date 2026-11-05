/**
 * Scenario registry + runner. `runScenario` builds a seeded ScenarioContext
 * bound to the live session and runs the named scenario, returning its metrics.
 * The caller (the run_scenario MCP tool) does a full oracle sweep afterwards.
 */
import { makeContext, type Scenario, type ScenarioMetrics, type ScenarioTarget } from './framework';
import { seedLargeProject } from './seedLargeProject';
import { undoRedoChurn } from './undoRedoChurn';
import { undoRedoFuzz } from './undoRedoFuzz';
import { undoPersistenceFuzz } from './undoPersistenceFuzz';
import { dialogCycling } from './dialogCycling';
import { toolSwitchSpam } from './toolSwitchSpam';
import { mapStorm } from './mapStorm';
import { saveUnderChurn } from './saveUnderChurn';
import { importRepeatedly } from './importRepeatedly';
import { importDialogFuzz } from './importDialogFuzz';
import { exportRoundtripFuzz } from './exportRoundtripFuzz';
import { rapidProjectCycle } from './rapidProjectCycle';
import { fractalStress } from './fractalStress';
import { fuzzDeterminism, fuzzDeterminismPlant, fuzzFixture } from '../lib/fuzzKnobs';
import { detProbe } from '../lib/fuzzDeterminismProbe';
import { runDeterminismPair } from '../lib/fuzzDeterminismRun';

const SCENARIOS: Scenario[] = [
  seedLargeProject,
  undoRedoChurn,
  undoRedoFuzz,
  undoPersistenceFuzz,
  dialogCycling,
  toolSwitchSpam,
  mapStorm,
  saveUnderChurn,
  importRepeatedly,
  importDialogFuzz,
  exportRoundtripFuzz,
  rapidProjectCycle,
  fractalStress,
];

const REGISTRY: Record<string, Scenario> = Object.fromEntries(SCENARIOS.map((s) => [s.name, s]));

export function listScenarios(): string[] {
  return Object.keys(REGISTRY).sort();
}

export function getScenario(name: string): Scenario | undefined {
  return REGISTRY[name];
}

/** The scenarios `STALAGMIGHT_FUZZ_DETERMINISM` replays twice (they carry the probe hooks). */
export const DETERMINISM_SCENARIOS: ReadonlySet<string> = new Set([
  undoRedoFuzz.name,
  undoPersistenceFuzz.name,
  exportRoundtripFuzz.name,
]);

export interface RunScenarioOpts {
  seed?: number | undefined;
  iterations?: number | undefined;
  log?: (message: string) => void;
}

export async function runScenario(
  name: string,
  target: ScenarioTarget,
  opts: RunScenarioOpts = {},
): Promise<ScenarioMetrics> {
  const scenario = REGISTRY[name];
  if (!scenario) {
    throw new Error(`unknown scenario "${name}"; known: ${listScenarios().join(', ') || '(none)'}`);
  }
  const seed = opts.seed ?? 1;
  const iterations = opts.iterations ?? scenario.defaultIterations;
  // Determinism mode (`lib/fuzzDeterminismRun.ts`): the same seed twice, in
  // this session, unless a caller already installed a replay's probe.
  const mode = fuzzDeterminism();
  if (mode !== 'off' && DETERMINISM_SCENARIOS.has(name) && detProbe() === null) {
    if (mode === 'two-sessions') {
      throw new Error(
        'STALAGMIGHT_FUZZ_DETERMINISM=2sessions needs a session per replay; only `pnpm agent:scenario` runs it',
      );
    }
    return runDeterminismPair(
      { scenario: name, seed, iterations, fixture: fuzzFixture(), mode, plantAt: fuzzDeterminismPlant(), log: opts.log },
      () => runScenario(name, target, opts),
    );
  }
  const ctx = makeContext(target, seed, opts.log);
  detProbe()?.addRoot(ctx.artifactsDir);
  ctx.log(`scenario ${name} start (seed=${seed}, iterations=${iterations})`);
  const start = Date.now();
  await scenario.run(ctx, { iterations });
  const durationMs = Date.now() - start;
  ctx.log(`scenario ${name} done in ${durationMs}ms`);
  return { scenario: name, seed, iterations, durationMs, metrics: ctx.metrics };
}
