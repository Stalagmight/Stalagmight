/**
 * [#576 FZ-INT] The frontend interleaving fuzzer (see `src/test/interleave/`): the
 * real stores, every IPC held until a seeded schedule applies and answers it
 * against a reference backend, and the invariants in `fuzzRun.ts` INVARIANTS
 * checked after every step and at quiescence.
 *
 * CI runs the pinned regression traces and the fixed seeds below (seconds), with
 * the shapes of the still-open findings left out (`KNOWN_OPEN_EXCLUDED`). Long
 * runs, on demand (docs/testing-recipes.md):
 *   FZ_INT_SEEDS=2000 FZ_INT_SEED_START=1000 pnpm exec vitest run src/store/__tests__/interleave.fuzz.test.ts
 *   FZ_INT_SECS=600 ...            random seeds for that long
 *   FZ_INT_SEED=1234 ...           one seed, printing its trace
 *   FZ_INT_SHAPES=all ...          include the open findings' shapes (or a comma list of RunOptions keys)
 *   FZ_INT_IGNORE=state,... ...    do not report these invariants
 * A failure prints the seed, the shrunk action list and its schedule trace; pin
 * the shrunk list in REGRESSION_TRACES once the bug is fixed.
 */
import { describe, expect, it, vi } from 'vitest';

import {
  ALL_SHAPES,
  describeAction,
  generate,
  KNOWN_OPEN_EXCLUDED,
  parseActions,
  runActions,
  shrink,
  type InvariantId,
  type RunOptions,
  type RunResult,
} from '../../test/interleave/fuzzRun';

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (command: string, args?: Record<string, unknown>) =>
    (globalThis as unknown as { __fzIntInvoke: (c: string, a?: Record<string, unknown>) => Promise<unknown> }).__fzIntInvoke(
      command,
      args,
    ),
}));

const env = (globalThis as unknown as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};

/** Commands the stores may send that the fuzzer answers without modelling. */
const KNOWN_UNMODELLED = new Set<string>([]);

/**
 * Every bug the fuzzer found and that is fixed, as its shrunk trace: each must now
 * hold every invariant. (`options` = the shapes the trace needs switched on.)
 */
const REGRESSION_TRACES: Array<{ id: string; found: InvariantId; options?: Partial<RunOptions>; actions: string }> = [
  {
    // An edit issued in CaveA while a switch to its copy was under way ran in the
    // copy: the identity guard compared created_at only, which copies share.
    id: 'FZ-FZ-INT-2',
    found: 'edit-target',
    options: { editAcrossSwitch: true },
    actions:
      'load("/caves/CaveA.stalx") run(0.000) run(0.000) run(0.000) run(0.000) edit(27) run(0.000) ' +
      'load("/caves/CaveA copy.stalx") edit(38) edit(39)',
  },
  {
    // A new project showed the previous project's Undo as available.
    id: 'FZ-FZ-INT-5',
    found: 'state',
    actions:
      'load("/caves/CaveA.stalx") run(0.000) run(0.000) run(0.000) undo run(0.000) run(0.000) panel(369) run(0.000) ' +
      'edit(36) run(0.000) run(0.000) run(0.000) create(65, false)',
  },
  {
    // An undo acknowledged while an autosave was in flight stayed unsaved, but the
    // autosave's answer marked the project clean (no prompt, no further autosave).
    id: 'FZ-FZ-INT-8',
    found: 'dirty-flag',
    actions:
      'load("/caves/CaveA.stalx") run(0.000) run(0.000) run(0.000) run(0.000) run(0.000) save run(0.000) ' +
      'reloadConfig edit(26) run(0.000) run(0.000) run(0.000) run(0.000) undo run(0.000) run(0.000) run(0.000) ' +
      'run(0.000) run(0.000) run(0.000) autosave run(0.584)',
  },
];


/**
 * The CI sweep. A seed stands for the list `generate` makes of it today, so a change
 * to the generator moves every seed: pin a found bug as its trace above, not as a seed.
 */
const FIXED_SEEDS = Array.from({ length: 55 }, (_, i) => 41 + i);

function shapes(): RunOptions {
  const wanted = env.FZ_INT_SHAPES;
  if (!wanted) return KNOWN_OPEN_EXCLUDED;
  if (wanted === 'all') return ALL_SHAPES;
  const on = new Set(wanted.split(','));
  return Object.fromEntries(Object.keys(KNOWN_OPEN_EXCLUDED).map((k) => [k, on.has(k)])) as unknown as RunOptions;
}

const OPTIONS = shapes();
const single = env.FZ_INT_SEED;
const count = Number(env.FZ_INT_SEEDS ?? 0);
const start = Number(env.FZ_INT_SEED_START ?? 1);
const secs = Number(env.FZ_INT_SECS ?? 0);
/** A long run gets the time it asked for, plus room to shrink what it found. */
const TIMEOUT_MS = secs > 0 ? (secs + 900) * 1000 : count > 0 ? count * 3000 + 900_000 : 120_000;

async function report(seed: number, result: RunResult, inv: InvariantId): Promise<string> {
  const shrunk = await shrink(generate(seed), inv, OPTIONS);
  const replay = await runActions(shrunk, OPTIONS);
  return [
    `seed ${seed}: ${result.violations.filter((v) => v.inv === inv).map((v) => v.msg).join('\n  ')}`,
    `shrunk to ${shrunk.length} actions: ${shrunk.map(describeAction).join(' ')}`,
    `replay: ${replay.violations.map((v) => `[${v.inv}] ${v.msg}`).join('; ') || 'PASSES (flaky?)'}`,
    'trace:',
    ...replay.trace.map((line) => `  ${line}`),
  ].join('\n');
}

describe('interleaving fuzzer (#576 FZ-INT)', () => {
  it.each(REGRESSION_TRACES)('regression trace $id ($found) holds every invariant', async ({ options, actions }) => {
    const result = await runActions(parseActions(actions), { ...KNOWN_OPEN_EXCLUDED, ...options });
    expect(result.violations.map((v) => `[${v.inv}] ${v.msg}`)).toEqual([]);
  });

  it('replays a seed exactly (the harness is deterministic)', async () => {
    const first = await runActions(generate(FIXED_SEEDS[0]!), OPTIONS);
    const again = await runActions(generate(FIXED_SEEDS[0]!), OPTIONS);
    expect(again.trace).toEqual(first.trace);
    expect(first.opsStarted).toBeGreaterThan(5);
  });

  it('holds every invariant on the fixed seeds', { timeout: TIMEOUT_MS }, async () => {
    const seeds = single
      ? [Number(single)]
      : count > 0
        ? Array.from({ length: count }, (_, i) => start + i)
        : FIXED_SEEDS;
    const deadline = secs > 0 ? Date.now() + secs * 1000 : 0;
    const ignored = new Set((env.FZ_INT_IGNORE ?? '').split(',').filter(Boolean));
    /** The first failing seed per invariant, and how many seeds failed it. */
    const byInvariant = new Map<InvariantId, { seed: number; result: RunResult; count: number }>();
    const unmodelled = new Set<string>();
    const failingSeeds: string[] = [];
    let runs = 0;
    for (let i = 0; deadline ? Date.now() < deadline : i < seeds.length; i += 1) {
      const seed = deadline ? Math.floor(Math.random() * 2 ** 31) : seeds[i]!;
      const result = await runActions(generate(seed), OPTIONS);
      runs += 1;
      result.unmodelled.forEach((c) => unmodelled.add(c));
      if (single) console.log(result.trace.join('\n'));
      if (result.violations.length > 0 && failingSeeds.length < 500) {
        failingSeeds.push(`${seed}:${[...new Set(result.violations.map((v) => v.inv))].join('+')}`);
      }
      for (const inv of new Set(result.violations.map((v) => v.inv))) {
        if (ignored.has(inv)) continue;
        const seen = byInvariant.get(inv);
        if (seen) seen.count += 1;
        else byInvariant.set(inv, { seed, result, count: 1 });
      }
    }
    const failures: string[] = [];
    for (const [inv, { seed, result, count: n }] of byInvariant) {
      failures.push(`${inv}: ${n} seed(s), first ${seed}\n${await report(seed, result, inv)}`);
    }
    const summary = [...byInvariant.entries()].map(([k, v]) => `${k} x${v.count}`).join(', ') || 'none';
    console.log(`[FZ-INT] ${runs} run(s), shapes ${JSON.stringify(OPTIONS)}; failing invariants: ${summary}`);
    if (failingSeeds.length) console.log(`[FZ-INT] failing seeds: ${failingSeeds.join(' ')}`);
    if (failures.length) console.log(failures.join('\n\n'));
    expect([...unmodelled].filter((c) => !KNOWN_UNMODELLED.has(c))).toEqual([]);
    expect(failures).toEqual([]);
  });
});
