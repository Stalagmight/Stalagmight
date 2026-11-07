/**
 * [#576 TEL-2] A failed flush keeps its counts under the project they were
 * recorded in.
 *
 * `flushToRust` restored a failed batch into the live counter map, and the
 * project sequence is only read when a batch is sent. So counts that failed to
 * go out during a project open were sent on the next flush under the NEXT
 * project's sequence - attributed to a project they never happened in.
 *
 * Each case records its flush timeline (`t=<s> seq=<n> <events> <outcome>`) so
 * a regression diff shows the mis-tagged batch itself, not just a count mismatch.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

type TrackEventsArgs = { events: Record<string, number>; projectSeq: number };

let flushes: string[] = [];
/** How many upcoming `track_events` calls reject. */
let failNext = 0;

beforeEach(() => {
  vi.useFakeTimers({ now: 0 });
  vi.resetModules();
  flushes = [];
  failNext = 0;
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: TrackEventsArgs) => {
    if (cmd === 'is_telemetry_available') return true;
    if (cmd === 'track_events' && args) {
      const at = (Date.now() / 1000).toFixed(1);
      const events = Object.entries(args.events)
        .map(([k, v]) => `${k}=${v}`)
        .sort()
        .join(',');
      const failing = failNext > 0;
      flushes.push(`t=${at}s seq=${args.projectSeq} ${events} ${failing ? 'FAILED' : 'ok'}`);
      if (failing) {
        failNext--;
        throw new Error('IPC unavailable');
      }
    }
    return undefined;
  });
});

afterEach(() => {
  vi.useRealTimers();
});

/** A fresh telemetry module (its state is module-level), enabled at t=0, so the
 *  60 s dwell interval ticks at t=60 s, 120 s, ... */
async function startTelemetry() {
  const telemetry = await import('../telemetry');
  await telemetry.initTelemetry(true);
  return telemetry;
}

/** Advance fake time to the absolute time `ms`, running due timers and microtasks. */
async function advanceTo(ms: number) {
  await vi.advanceTimersByTimeAsync(ms - Date.now());
}

describe('a failed telemetry flush', () => {
  it('re-sends its counts under the sequence they were recorded in, not the next project\'s', async () => {
    const t = await startTelemetry();

    await advanceTo(1_000);
    t.trackAction('old-project', 'toolbar');
    await advanceTo(2_000);
    failNext = 1;
    await t.advanceProjectSeq(); // project 1 opens; the seq-0 flush fails

    await advanceTo(30_000);
    t.trackAction('new-project', 'toolbar'); // project 1's own event
    await advanceTo(60_500); // the t=60 s interval tick retries

    expect(flushes, 'old-project was recorded under seq 0 and must be re-sent as seq 0').toEqual([
      't=2.0s seq=0 action.old-project:toolbar=1 FAILED',
      't=60.0s seq=0 action.old-project:toolbar=1 ok',
      't=60.0s seq=1 action.new-project:toolbar=1 ok',
    ]);
  });

  it('merges a failed batch with later counts of the same sequence and sends it once', async () => {
    const t = await startTelemetry();

    await advanceTo(30_000);
    t.trackAction('probe', 'toolbar');
    failNext = 1;
    await advanceTo(60_500); // interval tick: fails
    t.trackAction('probe', 'toolbar'); // same project, same event
    await advanceTo(120_500); // next tick: one batch, both counts

    expect(flushes).toEqual([
      't=60.0s seq=0 action.probe:toolbar=1 FAILED',
      't=120.0s seq=0 action.probe:toolbar=2 ok',
    ]);
  });

  it('keeps the failed counts across two project switches until they go out', async () => {
    const t = await startTelemetry();

    await advanceTo(1_000);
    t.trackAction('first', 'toolbar');
    failNext = 2;
    await t.advanceProjectSeq(); // seq 0 -> 1; fails
    await t.advanceProjectSeq(); // seq 1 -> 2; the retry of seq 0 fails again
    await advanceTo(60_500);

    expect(flushes).toEqual([
      't=1.0s seq=0 action.first:toolbar=1 FAILED',
      't=1.0s seq=0 action.first:toolbar=1 FAILED',
      't=60.0s seq=0 action.first:toolbar=1 ok',
    ]);
  });

  it('drops the held counts when telemetry is switched off', async () => {
    const t = await startTelemetry();

    await advanceTo(1_000);
    t.trackAction('before-opt-out', 'toolbar');
    failNext = 1;
    await t.advanceProjectSeq(); // fails; counts held for retry
    t.setTelemetryEnabled(false);
    t.setTelemetryEnabled(true);
    await advanceTo(130_000);

    expect(flushes).toEqual([
      't=1.0s seq=0 action.before-opt-out:toolbar=1 FAILED',
    ]);
  });
});
