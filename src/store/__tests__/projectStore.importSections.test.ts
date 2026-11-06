/**
 * `importSections` forwards every argument it takes, in its own key.
 *
 * The wrapper could not express `existingShotUpdates` - the overlap
 * interpolation the DMP wizard computes for shots that ALREADY exist in the
 * project - even though `import_sections` has taken them since the overlap
 * work. The only route to the overlap path was the wizard's own direct
 * `invokeIpc`, so any other caller silently dropped them (#9).
 *
 * The parameter was appended in SIXTH position on purpose: appending leaves
 * every existing call's positions untouched. That is only true while the
 * payload keys stay pinned to the right parameters, which is the failure this
 * file exists to catch - a 6th parameter inserted anywhere but the end
 * reshuffles `virtualShotMode` and `provenance` into each other's keys, and
 * every one of those is optional, so nothing else would complain.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';

vi.mock('../../utils/ipc');
stubGetErrorMessage(stringOfError);

import { useProjectStore } from '../projectStore';
import type { ImportSectionConfig } from '../../types/commands';
import type { ExistingShotUpdate } from '../../types/ipc/ExistingShotUpdate';
import { invokeIpcMock, stubGetErrorMessage, stringOfError } from '../../test/factories/ipc';

const SECTIONS = [] as ImportSectionConfig[];

const UPDATES: ExistingShotUpdate[] = [
  { section_id: 7, shot_id: 3, updates: { length: 4.25 } as ExistingShotUpdate['updates'] },
];

/** The args object `import_sections` was invoked with, or null. */
function importSectionsArgs(): Record<string, unknown> | null {
  const call = invokeIpcMock.mock.calls.find((c) => c[0] === 'import_sections');
  return (call?.[1] as Record<string, unknown> | undefined) ?? null;
}

describe('projectStore.importSections argument forwarding', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    invokeIpcMock.mockResolvedValue({ success: true });
    // Stub the post-command refresh so executeCommand's success path doesn't
    // fan out into the rest of the IPC surface.
    useProjectStore.setState({
      refreshProjectState: vi.fn().mockResolvedValue(undefined),
      checkProjectIntegrity: vi.fn().mockResolvedValue(undefined),
    } as Partial<ReturnType<typeof useProjectStore.getState>> as never);
  });

  it('forwards existingShotUpdates to the command', async () => {
    await useProjectStore
      .getState()
      .importSections(SECTIONS, 'hash', ['sec-hash'], 'AllJumps', undefined, UPDATES);

    expect(importSectionsArgs()).toMatchObject({ existingShotUpdates: UPDATES });
  });

  it('sends null, not undefined, when the caller omits them', async () => {
    await useProjectStore.getState().importSections(SECTIONS);

    const args = importSectionsArgs();
    // `toBeNull` and not `toEqual(null)`: vitest treats a missing key and an
    // explicit undefined as equal to nothing, which is exactly the distinction
    // under test. Tauri accepts either (a missing key hits `visit_none` in
    // `ipc/command.rs`, so `Option<Vec<ExistingShotUpdate>>` resolves to
    // `None`) - this pins the store's own convention, which every other
    // optional argument on this call already follows, so the payload has one
    // readable shape rather than a mix of absent and null keys.
    expect(args?.existingShotUpdates).toBeNull();
    expect(args).toHaveProperty('existingShotUpdates');
  });

  it('keeps each positional argument in its own key', async () => {
    await useProjectStore
      .getState()
      .importSections(
        SECTIONS,
        'file-hash',
        ['section-hash'],
        'AllReferences',
        {
          sourceLabel: 'mNemo DMP',
          sourceFileName: 'dive.dmp',
          sectionsFound: 4,
          sourceWarnings: ['Section "BBB" (#2 in the file) has no end-of-section marker'],
        },
        UPDATES,
      );

    expect(importSectionsArgs()).toEqual({
      sections: SECTIONS,
      fileHash: 'file-hash',
      sectionHashes: ['section-hash'],
      existingShotUpdates: UPDATES,
      virtualShotMode: 'AllReferences',
      sourceLabel: 'mNemo DMP',
      sourceFileName: 'dive.dmp',
      sectionsFound: 4,
      // #576 B13: the DMP file's parse warnings, led onto the receipt.
      sourceWarnings: ['Section "BBB" (#2 in the file) has no end-of-section marker'],
      // #576 FZ-FZ-INT-7: the project it was issued against (none open here).
      expectedProject: null,
    });
  });

  it('lands virtualShotMode in virtualShotMode when the later arguments are omitted', async () => {
    // The exact regression appending a parameter risks: this call predates
    // `existingShotUpdates` and passes nothing after `virtualShotMode`.
    await useProjectStore.getState().importSections(SECTIONS, 'h', null, 'Auto');

    const args = importSectionsArgs();
    expect(args?.virtualShotMode).toBe('Auto');
    expect(args?.existingShotUpdates).toBeNull();
    expect(args?.sectionHashes).toBeNull();
  });
});
