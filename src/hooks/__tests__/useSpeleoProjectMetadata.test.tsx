/**
 * [#576 D26] The SpeleoDB project-settings metadata load must not clobber what the
 * user is typing, and an older reply must not overwrite a newer one.
 *
 * The hook reloaded on every change of `t` (a language switch) and on Retry, with no
 * request sequence and no cancellation: whichever reply resolved LAST replaced both
 * the baseline and the form, discarding in-progress edits.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';

vi.mock('../../utils/ipc');
stubGetErrorMessage(errorMessageOrString);

import { useSpeleoProjectMetadata } from '../useSpeleoProjectMetadata';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import type { SpeleoProjectMetadata } from '../../types/ipc.generated';
import { invokeIpcMock, stubGetErrorMessage, errorMessageOrString } from '../../test/factories/ipc';
import { deferred } from '../../test/factories/async';

const META: SpeleoProjectMetadata = {
  name: 'CaveA',
  description: 'desc',
  country: 'US',
  visibility: 'PRIVATE',
  latitude: null,
  longitude: null,
  exclude_geojson: false,
};

beforeEach(() => {
  invokeIpcMock.mockReset();
  useSpeleoStore.setState({
    link: {
      permission: 'READ_AND_WRITE',
      editState: 'read-only',
      lockHolder: null,
      lockedByOther: false,
      hasUnpushedEdits: false,
      unwound: false,
      forceEditing: false,
      forceEditAllowed: false,
      undoMode: 'full',
      remoteState: 'up_to_date',
      noAccess: false,
      serverError: null,
    },
    refreshLinkStatus: vi.fn(async () => {}),
  });
  useUIStore.setState({ showToast: vi.fn() });
});

describe('useSpeleoProjectMetadata reloads (#576 D26)', () => {
  it('keeps what the user typed when a second load lands', async () => {
    invokeIpcMock.mockResolvedValueOnce(META);
    const { result } = renderHook(() => useSpeleoProjectMetadata());
    await waitFor(() => expect(result.current.form?.name).toBe('CaveA'));

    act(() => result.current.update('description', 'typed by the user'));

    // A background reload (Retry, or the language switch that re-created `t`) brings
    // back a newer server truth for a field the user did not touch.
    invokeIpcMock.mockResolvedValueOnce({ ...META, country: 'FR' });
    await act(async () => {
      await result.current.reload();
    });

    expect(result.current.form?.description).toBe('typed by the user');
    // The baseline moved, so the diff (and the PATCH) is against the new truth.
    expect(result.current.baseline?.country).toBe('FR');
    expect(result.current.canSave).toBe(true);
  });

  /**
   * #576 RV4-FE-SP2: the reload keeps only the fields the user TYPED. It used to keep
   * the whole previous form, so every untouched field held its old value, differed
   * from the new baseline, and the next Save PATCHed it - silently reverting what
   * another client had changed on the server.
   */
  it('rebases the untouched fields on the new truth, so Save sends only what was typed', async () => {
    invokeIpcMock.mockResolvedValueOnce({ ...META, latitude: 10, longitude: 20 });
    const { result } = renderHook(() => useSpeleoProjectMetadata());
    await waitFor(() => expect(result.current.form?.name).toBe('CaveA'));

    act(() => result.current.update('description', 'typed by the user'));
    // Another client changed the country and the coordinates meanwhile.
    invokeIpcMock.mockResolvedValueOnce({ ...META, country: 'FR', latitude: 11, longitude: 21 });
    await act(async () => {
      await result.current.reload();
    });

    const formAfterReload = result.current.form;
    invokeIpcMock.mockResolvedValueOnce({ ...META, country: 'FR', latitude: 11, longitude: 21, description: 'typed by the user' });
    await act(async () => {
      await result.current.handleSave();
    });
    const patchCall = invokeIpcMock.mock.calls.find(([command]) => command === 'speleodb_patch_project');
    console.log('[SP2] form after the reload:', JSON.stringify(formAfterReload));
    console.log('[SP2] PATCH sent:', JSON.stringify(patchCall?.[1]));
    expect(formAfterReload).toMatchObject({
      description: 'typed by the user',
      country: 'FR',
      latitude: '11',
      longitude: '21',
    });
    expect(patchCall?.[1]).toEqual({
      patch: {
        name: null,
        description: 'typed by the user',
        country: null,
        visibility: null,
        coordinates_edited: false,
        latitude: null,
        longitude: null,
      },
    });
  });

  it('keeps a typed coordinate pair whole across a reload', async () => {
    invokeIpcMock.mockResolvedValueOnce({ ...META, latitude: 10, longitude: 20 });
    const { result } = renderHook(() => useSpeleoProjectMetadata());
    await waitFor(() => expect(result.current.form?.latitude).toBe('10'));

    // Only the latitude was retyped, but the two travel as one pair.
    act(() => result.current.update('latitude', '12'));
    invokeIpcMock.mockResolvedValueOnce({ ...META, latitude: 11, longitude: 21 });
    await act(async () => {
      await result.current.reload();
    });

    expect(result.current.form).toMatchObject({ latitude: '12', longitude: '20' });
  });

  it('still refreshes a form the user has not touched', async () => {
    invokeIpcMock.mockResolvedValueOnce(META);
    const { result } = renderHook(() => useSpeleoProjectMetadata());
    await waitFor(() => expect(result.current.form?.name).toBe('CaveA'));

    invokeIpcMock.mockResolvedValueOnce({ ...META, name: 'CaveA renamed' });
    await act(async () => {
      await result.current.reload();
    });

    expect(result.current.form?.name).toBe('CaveA renamed');
  });

  it('lets the last load ISSUED win, not the last one to resolve', async () => {
    invokeIpcMock.mockResolvedValueOnce(META);
    const { result } = renderHook(() => useSpeleoProjectMetadata());
    await waitFor(() => expect(result.current.form?.name).toBe('CaveA'));

    const older = deferred<SpeleoProjectMetadata>();
    const newer = deferred<SpeleoProjectMetadata>();
    invokeIpcMock.mockReturnValueOnce(older.promise).mockReturnValueOnce(newer.promise);
    let first!: Promise<void>;
    let second!: Promise<void>;
    act(() => {
      first = result.current.reload();
      second = result.current.reload();
    });

    await act(async () => {
      newer.resolve({ ...META, name: 'newer' });
      await second;
    });
    await act(async () => {
      older.resolve({ ...META, name: 'older' });
      await first;
    });

    expect(result.current.baseline?.name).toBe('newer');
    expect(result.current.form?.name).toBe('newer');
    expect(result.current.loading).toBe(false);
  });

  it('applies nothing from a load that resolves after unmount', async () => {
    const pending = deferred<SpeleoProjectMetadata>();
    invokeIpcMock.mockReturnValueOnce(pending.promise);
    const { result, unmount } = renderHook(() => useSpeleoProjectMetadata());
    const before = result.current;
    unmount();
    await act(async () => {
      pending.resolve({ ...META, name: 'late' });
      await pending.promise;
    });
    // Nothing re-rendered the unmounted hook: the last snapshot is the loading one.
    expect(before.form).toBeNull();
    expect(result.current.form).toBeNull();
  });
});
