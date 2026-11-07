import { describe, expect, it, vi } from 'vitest';
import { HoverFrame } from '../hoverFrame';

/** A manual frame clock: frames run only when the test says so. */
function frames() {
  const queue = new Map<number, () => void>();
  let next = 1;
  return {
    schedule: (cb: () => void) => {
      const id = next++;
      queue.set(id, cb);
      return id;
    },
    unschedule: (id: number) => queue.delete(id),
    pending: () => queue.size,
    flush: () => {
      const cbs = [...queue.values()];
      queue.clear();
      for (const cb of cbs) cb();
    },
  };
}

describe('HoverFrame', () => {
  it('runs once per frame with the latest event', () => {
    const f = frames();
    const run = vi.fn();
    const h = new HoverFrame<string>(run, f.schedule, f.unschedule);
    h.push('a');
    h.push('b');
    h.push('c');
    expect(f.pending()).toBe(1);
    f.flush();
    expect(run).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith('c');
  });

  it('cancel drops the queued event and its frame', () => {
    const f = frames();
    const run = vi.fn();
    const h = new HoverFrame<string>(run, f.schedule, f.unschedule);
    h.push('a');
    h.cancel();
    expect(f.pending()).toBe(0);
    f.flush();
    expect(run).not.toHaveBeenCalled();
  });

  it('works again after a cancel', () => {
    const f = frames();
    const run = vi.fn();
    const h = new HoverFrame<string>(run, f.schedule, f.unschedule);
    h.push('a');
    h.cancel();
    h.push('b');
    f.flush();
    expect(run).toHaveBeenCalledTimes(1);
    expect(run).toHaveBeenCalledWith('b');
  });

  it('cancel with nothing queued is a no-op', () => {
    const f = frames();
    const run = vi.fn();
    const h = new HoverFrame<string>(run, f.schedule, f.unschedule);
    expect(() => h.cancel()).not.toThrow();
    expect(f.pending()).toBe(0);
  });
});
