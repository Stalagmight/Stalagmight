import { describe, it, expect } from 'vitest';
import { isPathError, isPermissionError, isUnreadableRefusal, saveErrorKind } from '../saveErrors';

describe('isPermissionError', () => {
  it('detects the backend permission code', () => {
    expect(isPermissionError('FS_PERMISSION_DENIED')).toBe(true);
  });

  it('does not flag unrelated save errors', () => {
    // The crux: a real corruption / generic failure must not be treated as a
    // permission problem (which would show the wrong remediation).
    expect(isPermissionError('FAILED')).toBe(false);
    expect(isPermissionError('INTERNAL_ERROR')).toBe(false);
  });

  it('does not flag a destination failure', () => {
    // #429: the missing-folder and over-MAX_PATH codes used to arrive as
    // FS_PERMISSION_DENIED, which is what put Windows users in front of the
    // macOS folder-access dialog.
    expect(isPermissionError('FS_PATH_NOT_FOUND')).toBe(false);
    expect(isPermissionError('FS_PATH_TOO_LONG')).toBe(false);
  });

  it('handles null/undefined safely', () => {
    expect(isPermissionError(null)).toBe(false);
    expect(isPermissionError(undefined)).toBe(false);
  });
});

describe('saveErrorKind', () => {
  it('maps each backend code to its own kind (#576 A6)', () => {
    expect(saveErrorKind('FS_PERMISSION_DENIED')).toBe('permission');
    expect(saveErrorKind('FS_PATH_NOT_FOUND')).toBe('path-not-found');
    expect(saveErrorKind('FS_PATH_TOO_LONG')).toBe('path-too-long');
    expect(saveErrorKind('IGNORES_UNREADABLE')).toBe('ignores-unreadable');
    expect(saveErrorKind('SPELEODB_LINK_UNREADABLE')).toBe('side-table-unreadable');
    expect(saveErrorKind('UNDO_MARKERS_UNREADABLE')).toBe('side-table-unreadable');
    expect(saveErrorKind('NEWER_FILE_READ_ONLY')).toBe('newer-file');
    expect(saveErrorKind('PROJECT_SETTINGS_UNREADABLE')).toBe('settings-unreadable');
  });

  // [#517] The code exists to keep this refusal out of the Review & Repair route.
  it('routes the unreadable side tables away from Review & Repair', () => {
    expect(isUnreadableRefusal(saveErrorKind('IGNORES_UNREADABLE'))).toBe(true);
    expect(isUnreadableRefusal(saveErrorKind('SPELEODB_LINK_UNREADABLE'))).toBe(true);
    expect(isUnreadableRefusal(saveErrorKind('UNDO_MARKERS_UNREADABLE'))).toBe(true);
    expect(isUnreadableRefusal(saveErrorKind('FAILED'))).toBe(false);
  });

  it('does not take an EDIT refusal of a newer file for a refused save of one', () => {
    // The old string started with EDIT_DISABLED, so it never classified as newer-file.
    expect(saveErrorKind('NEWER_FILE_EDIT_DISABLED')).toBe('other');
    expect(saveErrorKind('EDIT_DISABLED')).toBe('other');
  });

  it('treats an uncoded failure - and no failure at all - as other', () => {
    expect(saveErrorKind('FAILED')).toBe('other');
    expect(saveErrorKind(null)).toBe('other');
    expect(saveErrorKind(undefined)).toBe('other');
  });
});

describe('isPathError', () => {
  it('covers both destination failures and nothing else', () => {
    expect(isPathError('FS_PATH_NOT_FOUND')).toBe(true);
    expect(isPathError('FS_PATH_TOO_LONG')).toBe(true);
    expect(isPathError('FS_PERMISSION_DENIED')).toBe(false);
    expect(isPathError('FAILED')).toBe(false);
    expect(isPathError(null)).toBe(false);
  });
});
