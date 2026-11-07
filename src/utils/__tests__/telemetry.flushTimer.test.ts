/**
 * The pending flush timeout must die with any flush that pre-empts it.
 *
 * `flushToRust` used to null `flushTimer` without `clearTimeout`, so every flush
 * that did not come from that timeout itself - `advanceProjectSeq` on each project
 * open, and the 60 s dwell interval - left it running. The orphan still fired at
 * its old deadline and flushed whatever had been tracked since, ahead of that
 * batch's own window. It also nulled `flushTimer` in turn, orphaning the timeout
 * scheduled after it, so the one-pending-timeout invariant never came back. If the
 * orphan's deadline fell inside the `track_events` round-trip of a project open,
 * the new project's first events went out under the previous project's sequence.
 *
 * Each case prints its flush timeline (`t=<s> seq=<n> <events>`) so a regression
 * shows the stray flush itself, not just a count mismatch.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

type TrackEventsArgs = { events: Record<string, number>; projectSeq: number };

let flushes: string[] = [];
let trackEventsLatencyMs = 0;

beforeEach(() => {
  vi.useFakeTimers({ now: 0 });
  vi.resetModules();
  flushes = [];
  trackEventsLatencyMs = 0;
  invokeMock.mockReset();
  invokeMock.mockImplementation(async (cmd: string, args?: TrackEventsArgs) => {
    if (cmd === 'is_telemetry_available') return true;
    if (cmd === 'track_events' && args) {
      const at = (Date.now() / 1000).toFixed(1);
      flushes.push(`t=${at}s seq=${args.projectSeq} ${Object.keys(args.events).sort().join(',')}`);
      if (trackEventsLatencyMs > 0) {
        await new Promise((resolve) => setTimeout(resolve, trackEventsLatencyMs));
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

describe('telemetry flush timeout', () => {
  it('a project-open flush cancels the pending timeout, so later events wait for their own batch', async () => {
    const t = await startTelemetry();

    await advanceTo(1_000);
    t.trackAction('probe-a', 'toolbar'); // schedules the flush timeout for t=61 s
    await advanceTo(2_000);
    await t.advanceProjectSeq(); // project open: probe-a goes out now, under seq 0

    await advanceTo(60_500); // the t=60 s interval tick had nothing to send
    t.trackAction('probe-b', 'toolbar'); // its own timeout is due at t=120.5 s
    await advanceTo(61_500); // past probe-a's old t=61 s deadline

    expect(flushes, 'probe-b must not ride probe-a\'s cancelled 61 s deadline').toEqual([
      't=2.0s seq=0 action.probe-a:toolbar',
    ]);

    await advanceTo(120_500);
    expect(flushes).toEqual([
      't=2.0s seq=0 action.probe-a:toolbar',
      't=120.0s seq=1 action.probe-b:toolbar',
    ]);
  });

  it('an interval flush cancels the pending timeout too', async () => {
    const t = await startTelemetry();

    await advanceTo(30_000);
    t.trackAction('probe-a', 'toolbar'); // timeout due at t=90 s
    await advanceTo(60_000); // the interval tick sends probe-a

    await advanceTo(89_500);
    t.trackAction('probe-b', 'toolbar'); // its own timeout is due at t=149.5 s
    await advanceTo(120_500);

    expect(flushes, 'probe-b must not ride probe-a\'s cancelled 90 s deadline').toEqual([
      't=60.0s seq=0 action.probe-a:toolbar',
      't=120.0s seq=0 action.probe-b:toolbar',
    ]);
  });

  it('no stray flush lands inside a project open and tags the new project\'s events with the old sequence', async () => {
    const t = await startTelemetry();

    await advanceTo(1_000);
    t.trackAction('probe-a', 'toolbar'); // timeout due at t=61 s
    await advanceTo(2_000);
    await t.advanceProjectSeq(); // project 1 opens: seq 0 -> 1

    trackEventsLatencyMs = 1_000; // the next track_events round-trip spans t=61 s
    await advanceTo(60_500);
    t.trackAction('probe-b', 'toolbar'); // project 1's event
    const opening = t.advanceProjectSeq(); // project 2 opens: sends probe-b, bumps to seq 2 at t=61.5 s
    t.trackAction('probe-c', 'toolbar'); // project 2's first event, tracked during that round-trip
    await advanceTo(62_000);
    await opening;

    await advanceTo(120_500);
    expect(flushes, 'probe-c belongs to project 2 (seq 2)').toEqual([
      't=2.0s seq=0 action.probe-a:toolbar',
      't=60.5s seq=1 action.probe-b:toolbar',
      't=120.0s seq=2 action.probe-c:toolbar',
    ]);
  });
});

describe('advanceProjectSeq', () => {
  // The sequence used to move on only once the project-open flush had come back,
  // so the 60 s dwell interval could fire inside that round trip and send the new
  // project's first events under the previous project's sequence.
  it('tags an event tracked during the project-open flush with the new sequence, even when the interval fires inside it', async () => {
    const t = await startTelemetry();

    await advanceTo(59_500);
    t.trackAction('old-project', 'toolbar');
    trackEventsLatencyMs = 1_000; // the project-open round trip spans the t=60 s interval tick
    const opening = t.advanceProjectSeq(); // sends old-project under seq 0
    t.trackAction('new-project', 'toolbar'); // the new project's first event
    await advanceTo(61_000);
    await opening;

    trackEventsLatencyMs = 0;
    await advanceTo(130_000);
    expect(flushes, 'new-project belongs to the new project (seq 1)').toEqual([
      't=59.5s seq=0 action.old-project:toolbar',
      't=60.0s seq=1 action.new-project:toolbar',
    ]);
  });
});
