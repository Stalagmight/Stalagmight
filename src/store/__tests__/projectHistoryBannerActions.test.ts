import { beforeEach, expect, it, vi } from 'vitest';
import { createProjectHistoryBannerActions } from '../projectHistoryBannerActions';
import { useProjectStore } from '../projectStore';
const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: ipc }));
const { checkHistoryDiscarded } = createProjectHistoryBannerActions(
  useProjectStore.setState,
  useProjectStore.getState,
);
beforeEach(() => {
  ipc.mockReset();
  vi.spyOn(console, 'error').mockImplementation(() => {});
  useProjectStore.setState({
    project: { file_path: '/a.stalx' } as never,
    historyDiscarded: false,
    historyDiscardedEarlierVersion: false,
    historyDiscardedDismissed: false,
  });
});
it('says an earlier version\'s history was not kept when the backend says so', async () => {
  ipc.mockImplementation(async (command: string) => command === 'history_discarded' || command === 'history_discarded_from_earlier_version');
  await checkHistoryDiscarded();
  expect(useProjectStore.getState().historyDiscarded).toBe(true);
  expect(useProjectStore.getState().historyDiscardedEarlierVersion).toBe(true);
});
it('still raises the banner when only the reason cannot be read', async () => {
  ipc.mockImplementation(async (command: string) => {
    if (command === 'history_discarded') return true;
    throw new Error('reason unavailable');
  });
  await checkHistoryDiscarded();
  expect(useProjectStore.getState().historyDiscarded).toBe(true);
  expect(useProjectStore.getState().historyDiscardedEarlierVersion).toBe(false);
});
it('does not ask for the reason when nothing was discarded', async () => {
  ipc.mockResolvedValue(false);
  await checkHistoryDiscarded();
  expect(ipc).toHaveBeenCalledTimes(1);
  expect(useProjectStore.getState().historyDiscarded).toBe(false);
});
