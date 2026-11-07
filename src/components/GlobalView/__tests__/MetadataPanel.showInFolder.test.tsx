/**
 * Component test (#576 HFI-5): the project panel's "Show in folder" reveals the
 * project file through the opener plugin's own call - the one the recents
 * right-click menu makes (`revealItemInDir`). The panel used to hand-build the IPC
 * with `{ path }`, which the plugin's `reveal_item_in_dir` (it takes `paths`)
 * rejects, and then fell back to `open_path`, which the app's capabilities do not
 * grant: the button did nothing. The assertions sit on the Tauri IPC bridge
 * itself (`mockIPC`), under both `utils/ipc` and the plugin's JS API, so they see
 * the exact arguments that reach the plugin whichever route the panel takes.
 */
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks';
import { revealItemInDir } from '@tauri-apps/plugin-opener';
import MetadataPanel from '../MetadataPanel';
import type { ProjectMetadata } from '../../../types/project';

const projectAt = (file_path: string): ProjectMetadata =>
  ({
    name: 'CaveA',
    location: null,
    file_path,
    modified_at: new Date(0).toISOString(),
    section_count: 1,
    total_length: 10,
    gps_center: null,
    gps_bounds: null,
  } as ProjectMetadata);

const REVEAL = 'plugin:opener|reveal_item_in_dir';

/** Every command that reached the bridge, with its arguments. */
const invoke = vi.fn<(cmd: string, args: unknown) => unknown>();

/** Click Show in folder for a project at `filePath`; resolve once a reveal went out. */
const clickShowInFolder = async (filePath: string): Promise<void> => {
  render(<MetadataPanel project={projectAt(filePath)} onOpenProject={vi.fn()} onRemoveFromRecent={vi.fn()} />);
  await userEvent.click(screen.getByTestId('metadata-show-folder-button'));
  await waitFor(() => expect(invoke).toHaveBeenCalled());
};

beforeEach(() => {
  vi.spyOn(console, 'error').mockImplementation(() => {});
  invoke.mockReset();
  invoke.mockResolvedValue(undefined);
  mockIPC((cmd, args) => invoke(cmd, args));
});

afterEach(() => {
  clearMocks();
});

describe('MetadataPanel Show in folder (#576 HFI-5)', () => {
  it.each(['/home/user/caves/CaveA.stalx', 'C:\\caves\\CaveA.stalx'])(
    'reveals %s with the plugin\'s `paths` argument',
    async (filePath) => {
      await clickShowInFolder(filePath);

      expect(invoke).toHaveBeenCalledTimes(1);
      expect(invoke).toHaveBeenCalledWith(REVEAL, { paths: [filePath] });
    },
  );

  it('sends exactly what the recents right-click menu sends', async () => {
    const filePath = '/home/user/caves/CaveA.stalx';
    await revealItemInDir(filePath); // the right-click path (GlobalView handleShowInFolder)
    const rightClick = invoke.mock.calls[0];
    invoke.mockClear();

    await clickShowInFolder(filePath);

    expect(invoke.mock.calls).toEqual([rightClick]);
  });

  it('never reaches for open_path, which the capabilities do not grant, when the reveal fails', async () => {
    invoke.mockRejectedValue(new Error('reveal unavailable'));

    await clickShowInFolder('/home/user/caves/CaveA.stalx');

    await waitFor(() => expect(console.error).toHaveBeenCalled());
    expect(invoke.mock.calls.map(([cmd]) => cmd)).toEqual([REVEAL]);
  });
});
