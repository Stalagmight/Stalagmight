/**
 * [#576 B6-D1] A project saved by a NEWER Stalagmight is open read-only.
 *
 * The backend refuses every edit (`guard_mutation`, code `NEWER_FILE_EDIT_DISABLED`)
 * and every save (code `NEWER_FILE_READ_ONLY`) of such a project, and reports the
 * load's verdict as `newerSchemaVersion`. These pin the
 * frontend half: the canonical edit gate composes the flag, a refused save is a
 * read-only notice (not an error dialog, not Review & Repair), and a refused edit
 * says which read-only it is.
 */
import { describe, it, expect, beforeEach, vi } from 'vitest';
import { renderHook, act } from '@testing-library/react';
import { useUIStore } from '../uiStore';
import { useSpeleoStore } from '../speleoStore';
import { useProjectStore, selectNewerFileReadOnly } from '../projectStore';
import { useEditingBlocked, isEditingBlockedNow } from '../../hooks/useEditingBlocked';
import { saveErrorKind } from '../../utils/saveErrors';
import { surfaceSaveFailure, autosaveFailureMessage, type SaveFailure } from '../../utils/saveFeedback';
import { READ_ONLY_EDIT_NOTICE } from '../../utils/editErrors';
import { TOAST_KEYS } from '../../utils/toastKeys';
import i18n from '../../i18n';

/** The backend's refusals, verbatim in shape (`project_io.rs`, `app_state.rs`): code + sentence. */
const SAVE_REFUSAL: SaveFailure = {
  code: 'NEWER_FILE_READ_ONLY',
  error:
    'Saving is disabled: this project was saved by a newer version of ' +
    'Stalagmight (file format 19) and is open read-only. Update Stalagmight to edit it.',
};
const EDIT_REFUSAL: SaveFailure = {
  code: 'NEWER_FILE_EDIT_DISABLED',
  error:
    'Update shot 0:1 is disabled: this project was saved ' +
    'by a newer version of Stalagmight (file format 19) and is open read-only. Update Stalagmight ' +
    'to edit it.',
};

beforeEach(() => {
  // The one fully-editable state: survey, measured geometry, a confirmed-LOCAL
  // project (#270 fails closed on `unknown`), and a file this build can write.
  useUIStore.setState({ appMode: 'survey', geometryMode: 'measured' });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useProjectStore.setState({
    newerSchemaVersion: null,
    integritySaveBlocked: false,
    integrityDialogOpen: false,
  });
});

describe('the canonical edit gate', () => {
  it('blocks editing while a newer-version project is open, and only then', () => {
    expect(selectNewerFileReadOnly({ newerSchemaVersion: null })).toBe(false);
    expect(selectNewerFileReadOnly({ newerSchemaVersion: 19 })).toBe(true);

    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(false);
    expect(isEditingBlockedNow()).toBe(false);

    act(() => useProjectStore.setState({ newerSchemaVersion: 19 }));
    expect(result.current).toBe(true);
    expect(isEditingBlockedNow()).toBe(true);

    // Per project: the next (ordinary) project is editable again.
    act(() => useProjectStore.setState({ newerSchemaVersion: null }));
    expect(result.current).toBe(false);
  });

  it('keeps every hook call across a flip of the other terms (Rules of Hooks)', () => {
    useProjectStore.setState({ newerSchemaVersion: 19 });
    const { result } = renderHook(() => useEditingBlocked());
    expect(result.current).toBe(true);
    // Flipping the FIRST term on and off must not change the hook count.
    act(() => useUIStore.setState({ appMode: 'plan' }));
    expect(result.current).toBe(true);
    act(() => useUIStore.setState({ appMode: 'survey' }));
    expect(result.current).toBe(true);
  });
});

describe('a refused save', () => {
  it('is classified by its code', () => {
    expect(saveErrorKind(SAVE_REFUSAL.code)).toBe('newer-file');
    // The edit refusal is about the same newer file, but it is an edit-gate code:
    // not a save failure, and it must not classify as one.
    expect(saveErrorKind(EDIT_REFUSAL.code)).toBe('other');
  });

  it('is a read-only warning toast - no error dialog, no Review & Repair', () => {
    // Even on a project the integrity check flagged: Repair is an edit the read-only
    // project refuses too, so routing there would be a dead end.
    useProjectStore.setState({ integritySaveBlocked: true });
    const showToast = vi.spyOn(useUIStore.getState(), 'showToast');
    const openDialog = vi.fn();

    surfaceSaveFailure(SAVE_REFUSAL, openDialog, useProjectStore.getState());

    expect(openDialog).not.toHaveBeenCalled();
    expect(useProjectStore.getState().integrityDialogOpen).toBe(false);
    expect(showToast).toHaveBeenCalledWith(
      i18n.t('banners.newerFile.saveRefused'),
      TOAST_KEYS.NEWER_FILE_READ_ONLY,
      'warning'
    );
    showToast.mockRestore();
  });

  it('never shows its code on the autosave surface', () => {
    const message = autosaveFailureMessage(SAVE_REFUSAL);
    expect(message).not.toContain('NEWER_FILE_READ_ONLY');
    expect(message).toBe(i18n.t('banners.newerFile.saveRefused'));
  });
});

describe('a refused edit', () => {
  it('says the project is from a newer version, not "take the write lock"', () => {
    expect(READ_ONLY_EDIT_NOTICE(EDIT_REFUSAL.code)).toBe(i18n.t('editGate.newerFileNotice'));
    // The SpeleoDB read-only keeps its own line.
    expect(READ_ONLY_EDIT_NOTICE('EDIT_DISABLED')).toBe(
      i18n.t('editGate.readOnlyNotice')
    );
    expect(READ_ONLY_EDIT_NOTICE()).toBe(i18n.t('editGate.readOnlyNotice'));
  });
});
