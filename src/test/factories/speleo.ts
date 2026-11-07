/**
 * Builder for the SpeleoDB link state the speleo store holds for an open remote
 * project (#576 E15).
 */
import type { SpeleoLinkState } from '../../store/speleoStore';

/**
 * A linked project we may write to, opened read-only: nobody holds the lock, no
 * unpushed edits, on the server tip, full undo, force-edit not offered.
 */
export function makeSpeleoLink(overrides: Partial<SpeleoLinkState> = {}): SpeleoLinkState {
  return {
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
    ...overrides,
  };
}
