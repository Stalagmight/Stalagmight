// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';

const setTitle = vi.fn();
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ setTitle }),
}));

import { applyWindowTitle, resetWindowTitleWarningForTest } from '../windowTitle';

describe('applyWindowTitle', () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    setTitle.mockReset();
    resetWindowTitleWarningForTest();
    document.title = '';
  });

  it('sets the native title and leaves document.title alone on success', async () => {
    setTitle.mockResolvedValue(undefined);
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await applyWindowTitle('Stalagmight - a.stalx *');
    expect(setTitle).toHaveBeenCalledWith('Stalagmight - a.stalx *');
    expect(document.title).toBe('');
    expect(warn).not.toHaveBeenCalled();
  });

  it('falls back to document.title and warns once on rejection', async () => {
    setTitle.mockRejectedValue(new Error('not allowed'));
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    await applyWindowTitle('one');
    await applyWindowTitle('two');
    expect(document.title).toBe('two');
    expect(warn).toHaveBeenCalledTimes(1);
  });
});

// The mocks above and the e2e IPC mock both accept set_title unconditionally,
// so neither can see the ACL. core:default grants only the window getters
// (#618): without this explicit grant the real app rejects every setTitle.
describe('capability grant', () => {
  it('default.json grants core:window:allow-set-title to the main window', () => {
    const path = resolve(__dirname, '../../../src-tauri/capabilities/default.json');
    const cap = JSON.parse(readFileSync(path, 'utf8')) as { windows: string[]; permissions: unknown[] };
    expect(cap.windows).toContain('main');
    expect(cap.permissions).toContain('core:window:allow-set-title');
  });
});
