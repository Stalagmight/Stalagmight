/**
 * Determinism mode, the pairing: run the SAME seed twice, each replay under its
 * own probe (`fuzzDeterminismProbe.ts`), and compare the two record streams
 * (`fuzzDeterminism.ts`).
 *
 * `STALAGMIGHT_FUZZ_DETERMINISM=1` runs both replays in ONE app session -
 * `runScenario` does the pairing, and each replay opens its own fresh copy of
 * the fixture exactly as a single run does. `=2sessions` runs each replay in a
 * session of its own, which only `pnpm agent:scenario` can do (it owns the
 * session); see `scenario-run.ts`.
 *
 * Both replays always run, even when the first one fails: whether a failure
 * reproduces on the same seed is itself the question this mode answers.
 */
import type { ScenarioMetrics } from '../scenarios/framework';
import { compareRecordStreams, formatDivergence, Masker, recordBody, type DetRecord } from './fuzzDeterminism';
import { DetProbe, detProbe, setDetProbe } from './fuzzDeterminismProbe';
import type { FuzzDeterminismMode } from './fuzzKnobs';
import { valueShape } from './shareableFailure';

export interface DeterminismPairMeta {
  scenario: string;
  seed: number;
  iterations: number;
  fixture: string;
  mode: Exclude<FuzzDeterminismMode, 'off'>;
  /** `STALAGMIGHT_FUZZ_DETERMINISM_PLANT`: replay B perturbs its Nth state record. */
  plantAt: number | null;
  log?: ((message: string) => void) | undefined;
}

interface ReplayOutcome {
  probe: DetProbe;
  metrics: ScenarioMetrics | null;
  error: Error | null;
}

/** The first line of a failure, with temp paths and uuids masked, for comparing two verdicts. */
function headline(error: Error): string {
  return new Masker(null).text(error.message.split('\n')[0] ?? '');
}

function lastActionBefore(records: readonly DetRecord[], index: number): string | null {
  for (let at = Math.min(index, records.length - 1); at >= 0; at--) {
    const record = records[at] as DetRecord;
    if (record.kind !== 'action') continue;
    const text = JSON.stringify(recordBody(record));
    return text.length > 300 ? `${text.slice(0, 300)}...` : text;
  }
  return null;
}

/**
 * Run `runReplay` twice under fresh probes and compare. Returns replay A's
 * metrics (plus the record count) when both agree and both passed; throws the
 * divergence - seed, trace step, first differing path, reproduce line -
 * otherwise. When both replays failed the same way, A's failure is rethrown,
 * marked as reproduced.
 */
export async function runDeterminismPair(
  meta: DeterminismPairMeta,
  runReplay: (replay: 'A' | 'B') => Promise<ScenarioMetrics>,
): Promise<ScenarioMetrics> {
  if (detProbe() !== null) throw new Error('determinism replays cannot nest');
  const outcomes: ReplayOutcome[] = [];
  for (const replay of ['A', 'B'] as const) {
    const probe = new DetProbe(replay, meta.plantAt);
    setDetProbe(probe);
    let metrics: ScenarioMetrics | null = null;
    let error: Error | null = null;
    try {
      metrics = await runReplay(replay);
    } catch (err) {
      // Not an Error: its shape heads the message and its text goes on a later
      // line, which the runners withhold (#576 W51).
      error =
        err instanceof Error ? err : new Error(`a replay threw ${valueShape(err)}, not an Error\n  ${String(err)}`);
    } finally {
      setDetProbe(null);
    }
    meta.log?.(
      `determinism replay ${replay}: ${probe.records.length} record(s), ${error === null ? 'passed' : 'FAILED'}`,
    );
    outcomes.push({ probe, metrics, error });
  }
  const [a, b] = outcomes as [ReplayOutcome, ReplayOutcome];
  const divergence = compareRecordStreams(a.probe.records, b.probe.records);
  if (divergence !== null) {
    // Lines after the divergence's own, which the runners withhold (#576 W51).
    const verdicts =
      a.error === null && b.error === null
        ? []
        : [
            `  replay A ${a.error === null ? 'passed' : `failed: ${headline(a.error)}`}`,
            `  replay B ${b.error === null ? 'passed' : `failed: ${headline(b.error)}`}`,
          ];
    throw new Error(
      [
        formatDivergence(
          {
            scenario: meta.scenario,
            seed: meta.seed,
            iterations: meta.iterations,
            fixture: meta.fixture,
            knob: meta.mode === 'one-session' ? '1' : '2sessions',
            lastAction: lastActionBefore(a.probe.records, divergence.index),
          },
          divergence,
        ),
        ...verdicts,
      ].join('\n'),
    );
  }
  if (a.error !== null || b.error !== null) {
    if (a.error === null || b.error === null) {
      const failed = (a.error ?? b.error) as Error;
      throw new Error(
        `determinism: replay ${a.error === null ? 'B' : 'A'} of seed ${meta.seed} failed and replay ` +
          `${a.error === null ? 'A' : 'B'} passed, over identical record streams ` +
          `(${a.probe.records.length} records)\n${failed.message}`,
      );
    }
    const same = headline(a.error) === headline(b.error);
    a.error.message =
      `${a.error.message}\n  determinism: replay B ${same ? 'reproduced this failure' : `failed DIFFERENTLY: ${headline(b.error)}`} ` +
      `over identical record streams (${a.probe.records.length} records)`;
    throw a.error;
  }
  const metrics = a.metrics as ScenarioMetrics;
  metrics.metrics['determinism:records'] = a.probe.records.length;
  return metrics;
}
