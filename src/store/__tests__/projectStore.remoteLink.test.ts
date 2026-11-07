/**
 * [#576 D22] The SpeleoDB link bits after an edit are the BACKEND's, not a
 * frontend re-derivation.
 *
 * The backend decides whether an edit dirties the remote copy - and deliberately
 * does NOT for a provable no-op (reversing a shot-less section,
 * `is_provable_reverse_noop`; moving a comment onto its own holder). The frontend
 * used to mirror that bookkeeping by hand (`markForwardEdit` after every
 * successful command), so a no-op stamped the copy un-pushed and dropped its
 * Unwind position on screen while the backend stayed clean: the status button and
 * the quit guard read a phantom edit. Every survey mutation's answer now carries
 * `remote_link`, read from the backend's own state, and `invokeIpc` applies it.
 *
 * The real `invokeIpc` runs here (only Tauri's `invoke` is mocked), so the one
 * place that applies the bits is the thing under test. The replies carry the
 * success fields of both wire generations, so this file also runs, and fails, on
 * the code from before the change.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

import { useProjectStore } from '../projectStore';
import { useSpeleoStore } from '../speleoStore';
import { useUIStore } from '../uiStore';
import { projectStateEnvelope } from '../../test/projectStateEnvelope';
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

/** `reverse_section` answers `reply`; every refresh round trip answers an empty project. */
function serveReverse(reply: unknown) {
  invokeMock.mockImplementation(async (command: string) => {
    switch (command) {
      case 'reverse_section':
        return reply;
      case 'get_project_state_binary':
        return projectStateEnvelope({
          project: {
            name: 'CaveA',
            location: null,
            created_at: '2026-01-01T00:00:00Z',
            modified_at: '2026-01-01T00:00:01Z',
            sections: [],
            station_merges: [],
            zero_datums: [],
            loop_closures: [],
            file_path: '/tmp/cavea.stalx',
            next_section_id: 1,
            section_offsets: {},
            imported_file_hashes: [],
            imported_section_hashes: [],
          },
          stations: [],
          stats: null,
          frozen_shots: [],
        });
      case 'can_undo':
      case 'can_redo':
        return false;
      case 'check_project_integrity':
        return { messages: [], save_blocked: false };
      default:
        return null;
    }
  });
}

/** The answer to a reverse, in the success fields of both wire generations. */
const reverseReply = (remoteLink: { has_unpushed_edits: boolean; unwound: boolean }) => ({
  success: true,
  error: null,
  section_ids: [],
  reversed_sections: [],
  warnings: [],
  remote_link: remoteLink,
});

beforeEach(() => {
  invokeMock.mockReset();
  useUIStore.setState({ showToast: vi.fn() });
  useProjectStore.setState({ isDirty: false, project: null });
});

describe('the SpeleoDB link bits after an edit come from the backend (#576 D22)', () => {
  it('a no-op the backend did not dirty leaves a clean copy clean', async () => {
    useSpeleoStore.getState().seedFromStatus(status());
    serveReverse(reverseReply({ has_unpushed_edits: false, unwound: false }));

    const result = await useProjectStore.getState().reverseSection(7, false);

    expect(result.success).toBe(true);
    expect(useSpeleoStore.getState().link?.hasUnpushedEdits).toBe(false);
  });

  it('a no-op leaves an unwound copy unwound', async () => {
    // Walked back by an Unwind: dirty only because of the chain step.
    useSpeleoStore.getState().seedFromStatus(status({ has_unpushed_edits: true, unwound: true }));
    serveReverse(reverseReply({ has_unpushed_edits: true, unwound: true }));

    await useProjectStore.getState().reverseSection(7, false);

    expect(useSpeleoStore.getState().link?.unwound).toBe(true);
  });

  it('a real edit marks the copy exactly as the backend did', async () => {
    useSpeleoStore.getState().seedFromStatus(status({ has_unpushed_edits: true, unwound: true }));
    serveReverse(reverseReply({ has_unpushed_edits: true, unwound: false }));

    await useProjectStore.getState().reverseSection(7, false);

    const link = useSpeleoStore.getState().link;
    expect(link?.hasUnpushedEdits).toBe(true);
    expect(link?.unwound).toBe(false);
  });
});
