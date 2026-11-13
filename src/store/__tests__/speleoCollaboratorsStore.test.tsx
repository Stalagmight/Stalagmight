/**
 * The linked SpeleoDB project's collaborators: fetched in the background only
 * for a linked project with a signed-in account, once per (server, account,
 * project), never blocking anything, and silent when the fetch fails.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

import {
  collaboratorPerson,
  useCollaboratorsSync,
  useLinkedCollaborators,
  useSpeleoCollaboratorsStore,
} from '../speleoCollaboratorsStore';
import { useSpeleoStore } from '../speleoStore';

const calls = () => invokeIpcMock.mock.calls.filter(([cmd]) => cmd === 'speleodb_project_collaborators');

function useBoth() {
  useCollaboratorsSync();
  return useLinkedCollaborators();
}

function linked(projectId: string | null, loggedIn = true, userEmail = 'me@caves.example') {
  useSpeleoStore.setState({
    projectLinkKind: projectId ? 'remote' : 'local',
    remoteProjectId: projectId,
    session: { loggedIn, userEmail, endpoint: 'https://speleodb.internal' },
  });
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue([
    { name: 'Ann Diver', email: 'ann@caves.example', level: 'READ_AND_WRITE' },
    { name: null, email: 'quiet@caves.example', level: 'READ_ONLY' },
  ]);
  useSpeleoCollaboratorsStore.getState().reset();
});

describe('collaborator suggestions', () => {
  it('fetches once for a linked project with a signed-in account', async () => {
    linked('p-1');
    const { result, rerender } = renderHook(() => useBoth());
    expect(result.current).toEqual([]); // nothing waits for it
    await waitFor(() => expect(result.current).toHaveLength(2));
    expect(calls()).toEqual([['speleodb_project_collaborators', { projectId: 'p-1' }, expect.anything()]]);
    rerender();
    renderHook(() => useBoth());
    expect(calls()).toHaveLength(1);
  });

  it('names a collaborator with no known name by their email local part, never the address', () => {
    // The NAME reaches sections.surveyor and the TML / JSON exports, which must
    // not carry an email.
    expect(collaboratorPerson({ name: null, email: ' quiet@caves.example ', level: 'READ_ONLY' })).toEqual({
      name: 'quiet',
      email: 'quiet@caves.example',
    });
    expect(collaboratorPerson({ name: '  ', email: 'a.b@x.org', level: 'ADMIN' }).name).toBe('a.b');
    expect(collaboratorPerson({ name: 'Ann Smith', email: 'ann@x.org', level: 'ADMIN' }).name).toBe('Ann Smith');
  });

  it('never asks for a local project or while signed out', () => {
    linked(null);
    renderHook(() => useBoth());
    linked('p-1', false);
    renderHook(() => useBoth());
    expect(calls()).toHaveLength(0);
  });

  it('asks again for another account or project', async () => {
    linked('p-1');
    const { result } = renderHook(() => useBoth());
    await waitFor(() => expect(result.current).toHaveLength(2));
    act(() => linked('p-1', true, 'other@caves.example'));
    await waitFor(() => expect(calls()).toHaveLength(2));
    act(() => linked('p-2', true, 'other@caves.example'));
    await waitFor(() => expect(calls()).toHaveLength(3));
  });

  it('a failed fetch leaves no suggestions, no retry loop and no error', async () => {
    invokeIpcMock.mockRejectedValue(new Error('offline'));
    linked('p-9');
    const { result, rerender } = renderHook(() => useBoth());
    await waitFor(() => expect(useSpeleoCollaboratorsStore.getState().entries).not.toEqual({}));
    await waitFor(() =>
      expect(Object.values(useSpeleoCollaboratorsStore.getState().entries)[0]).toEqual({ status: 'failed' }),
    );
    rerender();
    expect(result.current).toEqual([]);
    expect(calls()).toHaveLength(1);
  });
});
