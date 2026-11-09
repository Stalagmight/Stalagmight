import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { defaultConfig, type Config } from '../../types/config';
import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

const initialProjectState = useProjectStore.getState();
const initialSpeleoState = useSpeleoStore.getState();
const endpointA = 'https://a.example.org';
const endpointB = 'https://b.example.org';
const configFor = (endpoint: string): Config => ({
  ...defaultConfig,
  speleodb: { ...defaultConfig.speleodb, endpoint },
});
const signIn = (endpoint: string) => useSpeleoStore.getState().setSessionFromLogin({
  endpoint,
  user_email: 'caver@example.org',
});

beforeEach(() => {
  invokeIpcMock.mockReset();
  useProjectStore.setState({ config: configFor(endpointA) });
  useSpeleoStore.setState({ session: null, sessionGeneration: 0 });
  invokeIpcMock.mockImplementation(async (command: string) => {
    if (command === 'get_config') return configFor(endpointB);
    return undefined;
  });
});

afterEach(() => {
  useProjectStore.setState(initialProjectState, true);
  useSpeleoStore.setState(initialSpeleoState, true);
});

describe('settings save after SpeleoDB authentication', () => {
  it('preserves an inline login when the cached settings still name the old server', async () => {
    signIn(endpointB);
    const session = useSpeleoStore.getState().session;
    const generation = useSpeleoStore.getState().sessionGeneration;

    expect((await useProjectStore.getState().updateConfig(configFor(endpointB))).success).toBe(true);

    expect(useSpeleoStore.getState().session).toEqual(session);
    expect(useSpeleoStore.getState().sessionGeneration).toBe(generation);
  });

  it('clears the old session when settings move its server', async () => {
    signIn(endpointA);

    expect((await useProjectStore.getState().updateConfig(configFor(endpointB))).success).toBe(true);

    expect(useSpeleoStore.getState().session).toBeNull();
  });

  it('compares the live session even when cached settings already match the requested server', async () => {
    useProjectStore.setState({ config: configFor(endpointB) });
    signIn(endpointA);

    await useProjectStore.getState().updateConfig(configFor(endpointB));

    expect(useSpeleoStore.getState().session).toBeNull();
  });

  it('preserves equivalent spellings of the authenticated server', async () => {
    signIn('https://B.example.org:443/');

    await useProjectStore.getState().updateConfig(configFor(endpointB));

    expect(useSpeleoStore.getState().session?.loggedIn).toBe(true);
  });

  it('does not clear a newer login when an older settings request completes', async () => {
    signIn(endpointA);
    let finishSave!: () => void;
    invokeIpcMock.mockImplementation(async (command: string) => {
      if (command === 'update_config') return new Promise<void>((resolve) => { finishSave = resolve; });
      if (command === 'get_config') return configFor(endpointA);
      return undefined;
    });

    const saving = useProjectStore.getState().updateConfig(configFor(endpointB));
    signIn(endpointA);
    const generation = useSpeleoStore.getState().sessionGeneration;
    finishSave();
    await saving;

    expect(useSpeleoStore.getState().session?.endpoint).toBe(endpointA);
    expect(useSpeleoStore.getState().sessionGeneration).toBe(generation);
  });

  it('keeps authentication when persisting the endpoint fails', async () => {
    signIn(endpointA);
    invokeIpcMock.mockRejectedValueOnce(new Error('disk full'));

    expect((await useProjectStore.getState().updateConfig(configFor(endpointB))).success).toBe(false);

    expect(useSpeleoStore.getState().session?.endpoint).toBe(endpointA);
  });
});
