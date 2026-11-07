/**
 * [#576 D23] A project switch waits for the edits queued before it.
 *
 * Edits run on one queue (`runSerialized`, src/store/mutationQueue.ts) and project
 * switches on another (`runProjectSwitch`). They did not wait for each other, so an
 * edit still queued (behind the previous edit's refresh, say) when the user opened
 * another project reached the backend AFTER the switch and ran against the new
 * project, where the same small section and shot ids mostly resolve. Edits issued
 * DURING a switch keep relying on the identity guards; this pins the other half.
 */
import { describe, expect, it } from 'vitest';
import { runSerialized } from '../store/mutationQueue';
import { switchUnderWay } from '../store/switchGate';
import { runProjectSwitch } from './projectTransition';
import { deferred, microtasks } from '../test/factories/async';

describe('runProjectSwitch', () => {
  it('starts only after the edits queued before it have finished', async () => {
    const trace: string[] = [];
    const editInFlight = deferred();
    const edit = runSerialized(async () => {
      trace.push('edit starts on the old project');
      await editInFlight.promise;
      trace.push('edit finishes on the old project');
    });
    const queuedEdit = runSerialized(async () => {
      trace.push('queued edit runs on the old project');
    });

    const projectSwitch = runProjectSwitch(async () => {
      trace.push('switch starts');
    });

    try {
      await microtasks(10);
      // The switch must still be waiting: an edit ahead of it is mid-flight.
      expect(trace).toEqual(['edit starts on the old project']);
    } finally {
      // Released whatever the verdict: the queue is module state, and a wedged
      // edit would stall every later test in this file.
      editInFlight.resolve();
    }
    await Promise.all([edit, queuedEdit, projectSwitch]);
    expect(trace).toEqual([
      'edit starts on the old project',
      'edit finishes on the old project',
      'queued edit runs on the old project',
      'switch starts',
    ]);
  });

  it('does not make an edit issued during the switch wait for it', async () => {
    const trace: string[] = [];
    const switchInFlight = deferred();
    const projectSwitch = runProjectSwitch(async () => {
      trace.push('switch starts');
      await switchInFlight.promise;
      trace.push('switch finishes');
    });
    await microtasks(10);

    // An edit issued while the switch runs goes straight through its own queue -
    // the identity guards, not this ordering, keep it off the new project.
    await runSerialized(async () => {
      trace.push('edit during the switch');
    });
    expect(trace).toEqual(['switch starts', 'edit during the switch']);

    switchInFlight.resolve();
    await projectSwitch;
    expect(trace).toEqual(['switch starts', 'edit during the switch', 'switch finishes']);
  });

  it('still runs when a queued edit failed', async () => {
    const failing = runSerialized(async () => {
      throw new Error('edit refused');
    });
    await expect(failing).rejects.toThrow('edit refused');
    await expect(runProjectSwitch(async () => 'switched')).resolves.toBe('switched');
  });

  it('releases the switch gate when superseding the running switch throws (#576 RVF-c-A1-3)', async () => {
    const running = deferred();
    const first = runProjectSwitch(
      async () => {
        await running.promise;
        return 'first';
      },
      {
        onSuperseded: () => {
          throw new Error('onSuperseded failed');
        },
      },
    );
    await microtasks(10); // the first switch is running (started)

    let thrown: unknown = null;
    try {
      await runProjectSwitch(async () => 'second');
    } catch (e) {
      thrown = e;
    }
    running.resolve();
    await first;
    await microtasks(10);

    expect(String(thrown)).toContain('onSuperseded failed');
    // Every later save and edit is refused while a switch is "under way"; a throw
    // must not leave that state behind.
    expect(switchUnderWay(), 'the gate is released').toBe(false);
  });

  it('starts the next switch only after the superseded one\'s cancel has answered (#576 RVF-c-C3)', async () => {
    // `cancel_loading` cancels whatever load is active when it RUNS. Sent when a load
    // is superseded, it must not still be on its way when the next switch starts its
    // own load, or it cancels that one instead.
    const trace: string[] = [];
    const running = deferred();
    const cancelAnswered = deferred();
    const first = runProjectSwitch(
      async () => {
        await running.promise;
        trace.push('first load answers');
      },
      {
        onSuperseded: () => {
          trace.push('cancel sent');
          return cancelAnswered.promise.then(() => {
            trace.push('cancel answered');
          });
        },
      },
    );
    await microtasks(10);
    const second = runProjectSwitch(async () => {
      trace.push('second load starts');
    });
    running.resolve(); // the superseded load finished on its own
    await first;
    await microtasks(10);
    const startedBeforeTheCancelAnswered = trace.includes('second load starts');
    cancelAnswered.resolve();
    await second;

    expect(startedBeforeTheCancelAnswered, trace.join(' > ')).toBe(false);
    expect(trace).toEqual(['cancel sent', 'first load answers', 'cancel answered', 'second load starts']);
  });
});
