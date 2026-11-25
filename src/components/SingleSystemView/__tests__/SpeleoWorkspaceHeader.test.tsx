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
  // sdbdemo2 issue 3: a session of this account that is not editing under the
  // account's lock (another computer may be) said only "Read-only".
  it('says the account holds the lock while this session is read-only under it', () => {
    expect(lockLine({ lockHolder: 'ME@example.com' }))
      .toHaveTextContent(/^Read-only - your account holds the write lock$/);
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

// Demo item 6 (screenshot 03): a read-only copy opened offline said "Ready for
// editing" beside the read-only badge, because a status that cannot reach SpeleoDB
// reads no lock holder. While the connection is unavailable the line says it is
// offline and what the last sign-in or lock scan saw.
describe('SpeleoDB workspace lock-state copy, offline', () => {
  it.each([
    [null, 'Offline - the lock was free when last checked'],
    ['editor@example.com', 'Offline - editor@example.com was editing when last checked'],
    ['ME@example.com', 'Offline - your account held the lock when last checked'],
    [undefined, 'Offline - lock state not known'],
  ] as const)('last known holder %s: "%s"', (holder, text) => {
    useSpeleoStore.setState({ remoteProjectId: 'p1', remoteLocks: holder === undefined ? {} : { p1: holder } });
    const line = lockLine({ remoteState: 'unknown' });
    expect(line.textContent).toBe(text);
    expect(screen.getByTestId('speleodb-workspace-project-header')).toHaveTextContent('Connection unavailable');
  });

  it('still says "Ready for editing" once SpeleoDB answers', () => {
    useSpeleoStore.setState({ remoteProjectId: 'p1', remoteLocks: { p1: null } });
    expect(lockLine({ remoteState: 'up_to_date' })).toHaveTextContent(/^Ready for editing$/);
  });
});

// Demo item 5 (screenshot 12): after the toast that SpeleoDB has a newer revision
// faded, nothing said the copy was behind. The tab says so for as long as the
// status knows it, and where Resume editing is offered, that it downloads the
// newer revision first (it does: `resume_editing_on_a_copy_behind_downloads_the_
// newer_revision_first`).
describe('SpeleoDB workspace, a copy behind SpeleoDB', () => {
  const newer = () => screen.queryByTestId('speleodb-workspace-newer-on-server');
  it('says so, and that Resume editing downloads it first', () => {
    lockLine({ remoteState: 'remote_ahead' });
    expect(newer()).toHaveTextContent(
      /^SpeleoDB has a newer revision than this copy\. Resume editing downloads it first\.$/,
    );
  });
  // sdbdemo2 P3 (screenshot 20): while editing the line gave no next step.
  it('while editing, says that Reload Project downloads it and editing goes on', () => {
    lockLine({ remoteState: 'remote_ahead', editState: 'editing', lockHolder: 'me@example.com' });
    expect(newer()).toHaveTextContent(
      /^SpeleoDB has a newer revision than this copy\. Reload Project downloads it, and you go on editing\.$/,
    );
  });
  it('says only that it is behind where neither Resume editing nor Reload is offered', () => {
    lockLine({ remoteState: 'remote_ahead', permission: 'READ_ONLY' });
    expect(newer()).toHaveTextContent(/^SpeleoDB has a newer revision than this copy\.$/);
  });
  it.each(['up_to_date', 'unknown', 'conflict'] as const)('says nothing of the kind when the copy is %s', (remoteState) => {
    lockLine({ remoteState });
    expect(newer()).toBeNull();
  });
});
