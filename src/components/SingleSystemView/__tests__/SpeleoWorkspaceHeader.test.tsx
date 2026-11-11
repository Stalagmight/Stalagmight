import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
import { SpeleoWorkspaceHeader } from '../SpeleoWorkspaceHeader';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null,
  lockedByOther: false, hasUnpushedEdits: false, unwound: false,
  undoMode: 'full', remoteState: 'up_to_date', noAccess: false, serverError: null,
};
beforeEach(() => {
  useProjectStore.setState({ project: { name: 'Synthetic cave', file_path: '/synthetic.stalx' } as never });
  useSpeleoStore.setState({ session: { loggedIn: true, userEmail: 'me@example.com', endpoint: null } });
});
const lockLine = (link: Partial<SpeleoLinkState>) => {
  render(<SpeleoWorkspaceHeader link={{ ...LINK, ...link }} />);
  return screen.getByTestId('speleodb-workspace-lock-state');
};

describe('SpeleoDB workspace lock-state copy', () => {
  it('says "You are editing" while we hold the lock', () => {
    expect(lockLine({ editState: 'editing', lockHolder: 'me@example.com' })).toHaveTextContent(/^You are editing$/);
  });
  it('names who else is editing', () => {
    expect(lockLine({ lockHolder: 'editor@example.com', lockedByOther: true }))
      .toHaveTextContent(/^Currently being edited by editor@example.com$/);
  });
  it('says "Ready for editing" when the lock is free and we may take it', () => {
    expect(lockLine({})).toHaveTextContent(/^Ready for editing$/);
  });
  it('says why a read-only account cannot edit', () => {
    expect(lockLine({ permission: 'READ_ONLY' }))
      .toHaveTextContent(/^Read-only - your SpeleoDB access does not allow editing$/);
  });
  it('stays "Read-only" when project access is gone', () => {
    expect(lockLine({ noAccess: true })).toHaveTextContent(/^Read-only$/);
  });
  it('says "Read-only copy - connect to SpeleoDB to edit" while signed out', () => {
    useSpeleoStore.setState({ session: { loggedIn: false, userEmail: null, endpoint: null } });
    expect(lockLine({})).toHaveTextContent(/^Read-only copy - connect to SpeleoDB to edit$/);
    // The connection line uses the Settings words (Connect / Disconnect) too.
    expect(screen.getByTestId('speleodb-workspace-project-header')).toHaveTextContent('Not connected');
  });
  it('says "Read-only copy - connect to SpeleoDB to edit" after the session was cleared', () => {
    useSpeleoStore.setState({ session: null });
    expect(lockLine({ lockHolder: 'editor@example.com', lockedByOther: true }))
      .toHaveTextContent(/^Read-only copy - connect to SpeleoDB to edit$/);
  });
});
