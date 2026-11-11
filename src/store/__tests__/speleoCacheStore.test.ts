import { beforeEach, describe, expect, it, vi } from 'vitest';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc, getErrorMessage: String }));

import { useSpeleoCacheStore } from '../speleoCacheStore';
import type { SpeleoCachedProject } from '../../types/ipc.generated';

const copy = (id: string): SpeleoCachedProject => ({
  project_id: id, project_name: id, local_path: `/cache/${id}/project.stalx`, ownership: 'mine',
  has_unpushed_edits: false, state_unknown: false, is_open: false,
});

beforeEach(() => {
  ipc.mockReset();
  useSpeleoCacheStore.setState({ copies: [], loaded: false });
});

describe('speleoCacheStore', () => {
  it('stores the listing', async () => {
    ipc.mockResolvedValue([copy('a')]);
    expect(await useSpeleoCacheStore.getState().refresh()).toEqual([copy('a')]);
    expect(useSpeleoCacheStore.getState()).toMatchObject({ copies: [copy('a')], loaded: true });
  });

  it('drops a reply older than the latest request', async () => {
    let answerFirst: (v: SpeleoCachedProject[]) => void = () => {};
    ipc.mockImplementationOnce(() => new Promise((resolve) => { answerFirst = resolve; }));
    ipc.mockResolvedValueOnce([copy('new')]);
    const first = useSpeleoCacheStore.getState().refresh();
    await useSpeleoCacheStore.getState().refresh();
    answerFirst([copy('old')]);
    await first;
    expect(useSpeleoCacheStore.getState().copies).toEqual([copy('new')]);
  });

  it('keeps the last listing when the call fails, and an older backend answering null lists nothing', async () => {
    useSpeleoCacheStore.setState({ copies: [copy('kept')] });
    ipc.mockRejectedValueOnce(new Error('synthetic'));
    await useSpeleoCacheStore.getState().refresh();
    expect(useSpeleoCacheStore.getState()).toMatchObject({ copies: [copy('kept')], loaded: true });
    ipc.mockResolvedValueOnce(null);
    await useSpeleoCacheStore.getState().refresh();
    expect(useSpeleoCacheStore.getState().copies).toEqual([]);
  });
});
