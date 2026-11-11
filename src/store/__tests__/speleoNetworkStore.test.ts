import { beforeEach, describe, expect, it } from 'vitest';
import {
  selectSpeleoExclusiveBusy,
  selectSpeleoNetworkBusy,
  trackSpeleoNetworkCall,
  useSpeleoNetworkStore,
} from '../speleoNetworkStore';

const counts = () => {
  const s = useSpeleoNetworkStore.getState();
  return { inFlight: s.inFlight, exclusive: s.exclusiveInFlight };
};

/** A promise the test settles by hand. */
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

beforeEach(() => useSpeleoNetworkStore.getState().reset());

describe('speleoNetworkStore counters', () => {
  it('counts reads and exclusive calls separately and ends each once', () => {
    const endRead = useSpeleoNetworkStore.getState().begin('read');
    const endExclusive = useSpeleoNetworkStore.getState().begin('exclusive');
    expect(counts()).toEqual({ inFlight: 2, exclusive: 1 });
    expect(selectSpeleoNetworkBusy(useSpeleoNetworkStore.getState())).toBe(true);
    expect(selectSpeleoExclusiveBusy(useSpeleoNetworkStore.getState())).toBe(true);

    endExclusive();
    expect(counts()).toEqual({ inFlight: 1, exclusive: 0 });
    expect(selectSpeleoExclusiveBusy(useSpeleoNetworkStore.getState())).toBe(false);
    endRead();
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
    expect(selectSpeleoNetworkBusy(useSpeleoNetworkStore.getState())).toBe(false);
  });

  it('ignores a second end of the same call', () => {
    const endA = useSpeleoNetworkStore.getState().begin('exclusive');
    useSpeleoNetworkStore.getState().begin('exclusive');
    endA();
    endA();
    expect(counts()).toEqual({ inFlight: 1, exclusive: 1 });
  });

  it('keeps the indicator up until the last of overlapping calls ends', () => {
    const first = useSpeleoNetworkStore.getState().begin('read');
    const second = useSpeleoNetworkStore.getState().begin('read');
    first();
    expect(selectSpeleoNetworkBusy(useSpeleoNetworkStore.getState())).toBe(true);
    second();
    expect(selectSpeleoNetworkBusy(useSpeleoNetworkStore.getState())).toBe(false);
  });
});

describe('trackSpeleoNetworkCall', () => {
  it('counts from before the call starts until it resolves', async () => {
    const reply = deferred<string>();
    let seenAtStart = -1;
    const call = trackSpeleoNetworkCall('exclusive', () => {
      seenAtStart = useSpeleoNetworkStore.getState().exclusiveInFlight;
      return reply.promise;
    });
    expect(seenAtStart).toBe(1);
    expect(counts()).toEqual({ inFlight: 1, exclusive: 1 });
    reply.resolve('done');
    await expect(call).resolves.toBe('done');
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('ends the count when the call rejects', async () => {
    const reply = deferred<string>();
    const call = trackSpeleoNetworkCall('read', () => reply.promise);
    expect(counts().inFlight).toBe(1);
    reply.reject(new Error('offline'));
    await expect(call).rejects.toThrow('offline');
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('ends the count when the call throws before returning a promise', () => {
    expect(() =>
      trackSpeleoNetworkCall('exclusive', () => {
        throw new Error('IPC unavailable');
      }),
    ).toThrow('IPC unavailable');
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('keeps counting a call its caller abandoned until the reply arrives', async () => {
    // A caller that stops awaiting (a superseded flow, a closed dialog) does not
    // stop the backend call; the indicator must not claim it did.
    const reply = deferred<string>();
    void trackSpeleoNetworkCall('exclusive', () => reply.promise);
    expect(counts().exclusive).toBe(1);
    reply.resolve('late');
    await Promise.resolve();
    await Promise.resolve();
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });

  it('does not count a call that never reaches SpeleoDB', async () => {
    await trackSpeleoNetworkCall(null, () => Promise.resolve(1));
    expect(counts()).toEqual({ inFlight: 0, exclusive: 0 });
  });
});
