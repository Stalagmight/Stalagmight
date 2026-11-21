/**
 * The status bar's save state names the file's own state where "Saved" would
 * mislead:
 * - demo issue N-6: a project opened from a 0.6.0 file read "Saved" while the
 *   file on disk was still the 0.6.0 file. It now says the old format is
 *   converted when you save, until the converting save; unsaved changes still
 *   say so first.
 * - demo issue N-7: a file made with a newer Stalagmight read "Saved", though
 *   this version never saves it. It now reads "Read-only".
 * Injection: making `SaveStatus` ignore `legacyFileOnDisk` or
 * `fileReadOnlyReason` (always the plain "Saved") fails the first two cases.
 */
import { beforeEach, describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SaveStatus } from '../SaveStatus';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { FILE_STATUS_CLEARED } from '../../../store/projectFileStatusActions';
import type { AutoSaveStatus } from '../../../hooks/useAutoSave';

function statusText(status: AutoSaveStatus, isDirty = false): string {
  render(<SaveStatus status={status} countdown={12} isDirty={isDirty} onSave={() => {}} />);
  return screen.getByTestId('save-status').textContent ?? '';
}

beforeEach(() => {
  useProjectStore.setState({ ...FILE_STATUS_CLEARED });
  useSpeleoStore.setState({ remoteProjectId: null } as never);
});

describe('SaveStatus and the file on disk', () => {
  it('says a 0.6.0 file is converted when you save, not "Saved"', () => {
    useProjectStore.setState({ legacyFileOnDisk: true });
    expect(statusText('idle')).toBe('Old format: converted when you save');
  });

  it('says "Read-only" for a file made with a newer Stalagmight, whatever autosave last did', () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' });
    expect(statusText('idle')).toBe('Read-only');
  });

  it('keeps "Read-only" over every autosave state', () => {
    useProjectStore.setState({ fileReadOnlyReason: 'requires zz-future' });
    for (const status of ['saved', 'pending', 'saving', 'error'] as const) {
      const { unmount } = render(<SaveStatus status={status} countdown={3} isDirty onSave={() => {}} />);
      expect(screen.getByTestId('save-status').textContent).toBe('Read-only');
      unmount();
    }
  });

  it('still puts unsaved changes and the countdown first on a 0.6.0 file', () => {
    useProjectStore.setState({ legacyFileOnDisk: true });
    expect(statusText('idle', true)).toBe('Unsaved changes');
  });

  it('shows the autosave countdown on a 0.6.0 file', () => {
    useProjectStore.setState({ legacyFileOnDisk: true });
    expect(statusText('pending', true)).toBe('Auto-save in 12s');
  });

  it('says "Saved" for an ordinary version 2 file', () => {
    expect(statusText('idle')).toBe('Saved');
  });
});
