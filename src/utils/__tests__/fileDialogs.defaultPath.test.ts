/**
 * saveDialog roots a RELATIVE default path in the documents folder, and must
 * leave an absolute one alone. A Windows network path (`\\srv\share\...`) or a
 * verbatim one (`\\?\C:\...`) is absolute too: prefixing it handed the native
 * dialog `C:\Users\u\Documents\\\srv\share\...` (#576).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';

const saveMock = vi.hoisted(() => vi.fn());
vi.mock('@tauri-apps/plugin-dialog', () => ({ save: saveMock }));
vi.mock('@tauri-apps/api/path', () => ({
  documentDir: async () => 'C:\\Users\\u\\Documents',
  sep: () => '\\',
}));

import { saveDialog } from '../fileDialogs';

/** The default path saveDialog hands the native dialog. */
async function offered(defaultPath: string): Promise<unknown> {
  await saveDialog({ defaultPath });
  return (saveMock.mock.lastCall?.[0] as { defaultPath?: unknown } | undefined)?.defaultPath;
}

beforeEach(() => {
  saveMock.mockReset();
  saveMock.mockResolvedValue(null);
});

describe('saveDialog default path', () => {
  it.each([
    '\\\\srv\\share\\CaveA_merged.stalx',
    '\\\\?\\C:\\caves\\CaveA_merged.stalx',
    'C:\\caves\\CaveA_merged.stalx',
    'C:/caves/CaveA_merged.stalx',
    '/home/u/caves/CaveA_merged.stalx',
  ])('passes the absolute path %s through unchanged', async (path) => {
    expect(await offered(path)).toBe(path);
  });

  it('roots a bare file name in the documents folder', async () => {
    expect(await offered('CaveA_merged.stalx')).toBe('C:\\Users\\u\\Documents\\CaveA_merged.stalx');
  });
});
