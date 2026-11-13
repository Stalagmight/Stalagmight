/**
 * Recent people: remembered across projects in config.toml through the
 * partial `remember_people` command. Published at once, one write per gesture,
 * replaced by what the backend stored, and a failure stays quiet.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import { MAX_RECENT_PEOPLE, mergeRecent, rememberPeople } from '../recentPeople';
import { useProjectStore } from '../projectStore';
import { useUIStore } from '../uiStore';
import { defaultConfig, type Config } from '../../types/config';

const flush = async () => {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
};
const recent = () => useProjectStore.getState().config?.people.recent;
const rememberCalls = () => invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'remember_people');

beforeEach(() => {
  invokeIpcMock.mockReset();
  const config = structuredClone(defaultConfig) as Config;
  config.people = { recent: [{ name: 'Old Timer' }] };
  useProjectStore.setState({ config });
  useUIStore.setState({ toasts: [] });
});

describe('mergeRecent', () => {
  it('puts the new people first, drops their older copies and caps the list', () => {
    const current = Array.from({ length: MAX_RECENT_PEOPLE }, (_, i) => ({ name: `P${i}` }));
    const merged = mergeRecent(current, [{ name: 'p5' }, { name: 'New' }]);
    expect(merged).toHaveLength(MAX_RECENT_PEOPLE);
    expect(merged.slice(0, 3)).toEqual([{ name: 'p5' }, { name: 'New' }, { name: 'P0' }]);
    expect(merged.filter((p) => p.name.toLowerCase() === 'p5')).toHaveLength(1);
  });
});

describe('rememberPeople', () => {
  it('publishes at once and sends one write per gesture, newest first', async () => {
    invokeIpcMock.mockImplementation(async (_cmd: string, args: { people: unknown[] }) => [...args.people, { name: 'Old Timer' }]);
    rememberPeople([{ name: '  Ann   Diver ' }]);
    rememberPeople([{ name: 'Bo Lead', email: ' bo@caves.example ' }]);
    // Optimistic: visible before the backend answers.
    expect(recent()).toEqual([{ name: 'Bo Lead', email: 'bo@caves.example' }, { name: 'Ann Diver' }, { name: 'Old Timer' }]);
    await flush();
    expect(rememberCalls()).toHaveLength(1);
    expect(rememberCalls()[0]?.[1]).toEqual({ people: [{ name: 'Bo Lead', email: 'bo@caves.example' }, { name: 'Ann Diver' }] });
  });

  it('adopts the list the backend stored', async () => {
    invokeIpcMock.mockResolvedValue([{ name: 'Ann Diver' }, { name: 'Stored Elsewhere' }]);
    rememberPeople([{ name: 'Ann Diver' }]);
    await flush();
    expect(recent()).toEqual([{ name: 'Ann Diver' }, { name: 'Stored Elsewhere' }]);
  });

  it('ignores nameless people and writes nothing for them', async () => {
    rememberPeople([{ name: '   ' }]);
    await flush();
    expect(rememberCalls()).toHaveLength(0);
    expect(recent()).toEqual([{ name: 'Old Timer' }]);
  });

  it('keeps the optimistic list and shows no toast when the write fails', async () => {
    invokeIpcMock.mockRejectedValue(new Error('disk full'));
    rememberPeople([{ name: 'Ann Diver' }]);
    await flush();
    expect(recent()).toEqual([{ name: 'Ann Diver' }, { name: 'Old Timer' }]);
    expect(useUIStore.getState().toasts).toEqual([]);
  });
});
