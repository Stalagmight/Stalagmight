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
import { valueShape } from '../lib/shareableFailure';

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
    // The name is not one of ours: the caller's text (an MCP argument, a replay
    // spec's recorded call), so it prints as its shape (#576 W51).
    throw new Error(`unknown scenario ${valueShape(name)}; known: ${listScenarios().join(', ') || '(none)'}`);
  }
  // Numbers whatever the caller read them from (#576 W51): `fuzz-shrink` takes
  // them out of a trace and a replay spec out of an actions.jsonl, which a
  // forged copy can fill with text, and both print in the progress lines.
  const seed = Number(opts.seed ?? 1);
  const iterations = Number(opts.iterations ?? scenario.defaultIterations);
  const ctx = makeContext(target, seed, opts.log);
  ctx.log(`scenario ${name} start (seed=${seed}, iterations=${iterations})`);
  const start = Date.now();
  await scenario.run(ctx, { iterations });
  const durationMs = Date.now() - start;
  ctx.log(`scenario ${name} done in ${durationMs}ms`);
  return { scenario: name, seed, iterations, durationMs, metrics: ctx.metrics };
}
