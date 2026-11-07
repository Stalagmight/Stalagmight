/**
 * [#576 RV7a-2] A survey mutation that crashed AFTER it marked the SpeleoDB copy
 * leaves the copy un-pushed on screen at once.
 *
 * A single undo stamps the link before its geometry reconcile; a crash in the
 * reconcile used to answer `INTERNAL_ERROR` with no link bits, so the status button
 * and the quit guard kept the clean state until the next status poll. The backend's
 * crash answer now carries `remote_link` (`pool::run_mutation`), and `invokeIpc`
 * applies the bits of a rejection exactly as it does a resolve's. The real
 * `invokeIpc` runs here; only Tauri's `invoke` is mocked.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import type { SpeleoLinkStatus } from '../../types/ipc.generated';

const invokeMock = vi.hoisted(() => vi.fn());

vi.mock('@tauri-apps/api/core', async () => (await import('../../test/factories/tauri')).tauriCore(invokeMock));

const status = (o: Partial<SpeleoLinkStatus> = {}): SpeleoLinkStatus => ({
  is_remote: true,
  edit_enabled: true,
  has_unpushed_edits: false,
  unwound: false,
  permission: 'READ_AND_WRITE',
  lock_holder: null,
  remote_state: 'up_to_date',
  access_denied: false,
  server_error: null,
  exclude_geojson: false,
  undo_mode: 'full',
  force_editing: false,
  allow_force_edit: false,
  project_id: null,
  ...o,
});

beforeEach(() => {
  invokeMock.mockReset();
  useUIStore.setState({ showToast: vi.fn() });
  useProjectStore.setState({ isDirty: false, project: null });
});

describe('a crashed survey mutation carries the link bits it left (#576 RV7a-2)', () => {
  it('an INTERNAL_ERROR rejection with dirty bits marks the copy un-pushed', async () => {
    useSpeleoStore.getState().seedFromStatus(status({ has_unpushed_edits: false, unwound: true }));
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'reverse_section') {
        throw {
          code: 'INTERNAL_ERROR',
          message: 'The operation failed unexpectedly (synthetic crash)',
          remote_link: { has_unpushed_edits: true, unwound: false },
        };
      }
      return null;
    });

    await useProjectStore.getState().reverseSection(7, false);

    const link = useSpeleoStore.getState().link;
    expect(link?.hasUnpushedEdits).toBe(true);
    expect(link?.unwound).toBe(false);
  });

  it('a crash with no bits (a local project) leaves the link as it was', async () => {
    useSpeleoStore.getState().seedFromStatus(status({ has_unpushed_edits: false, unwound: true }));
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'reverse_section') {
        throw { code: 'INTERNAL_ERROR', message: 'The operation failed unexpectedly (synthetic crash)' };
      }
      return null;
    });

    await useProjectStore.getState().reverseSection(7, false);

    const link = useSpeleoStore.getState().link;
    expect(link?.hasUnpushedEdits).toBe(false);
    expect(link?.unwound).toBe(true);
  });
});
