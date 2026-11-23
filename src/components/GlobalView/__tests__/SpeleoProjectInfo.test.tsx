import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SpeleoProjectInfo } from '../SpeleoProjectInfo';
import { resolveSpeleoLock } from '../SpeleoLockBadge';
import type { SpeleoProjectSummary } from '../../../types/ipc.generated';
import type { SpeleoLocalStateMap } from '../../../utils/speleoLocalState';

const summary: SpeleoProjectSummary = {
  id: 'project', name: 'Cave', permission: 'ADMIN', lock_holder: null,
  type_tag: 'stalx', latest_commit_sha: null,
};

/** This computer's local state for the project: this run's claim, a copy. */
function local(claimedHere: boolean, copy = false): SpeleoLocalStateMap {
  return { [summary.id]: {
    has_local_copy: copy, has_unpushed_edits: false, last_edit_at: null, is_open: false,
    state_unknown: false, lock_claimed_here: claimedHere,
  } };
}

function show(
  holder: string | null | undefined,
  permission = summary.permission,
  localStates: SpeleoLocalStateMap = local(true),
  linkedHere = false,
) {
  const lock = resolveSpeleoLock({ is_remote: true, speleodb_project_id: summary.id },
    holder === undefined ? {} : { project: holder }, 'me@example.com', {}, localStates, {}, linkedHere);
  render(<SpeleoProjectInfo summary={{ ...summary, permission }} lock={lock} testIdPrefix="project" />);
}

describe('SpeleoDB listing information', () => {
  it('shows the SpeleoDB identity, full permission and unlocked state', () => {
    show(null);
    expect(screen.getByText('SpeleoDB')).toBeVisible();
    expect(screen.getByTestId('project-permission')).toHaveTextContent('Permission: Admin');
    expect(screen.getByTestId('project-lock')).toHaveTextContent('Ready for editing');
  });
  it('recognises our own lock by the account email, whatever its case', () => {
    show('ME@example.com');
    expect(screen.getByTestId('project-lock')).toHaveTextContent(/^You are editing$/);
  });
  // sdbdemo2 issue 3 (screenshots 11, 12): a second session of the account saw
  // the held-lock prompt say "not by this computer" while this card behind it
  // said "You are editing". The card now uses the prompt's own rule.
  it('says "You are editing" only for a lock this computer took', () => {
    show('me@example.com', summary.permission, local(true));
    expect(screen.getByTestId('project-lock')).toHaveTextContent(/^You are editing$/);
  });
  it('says the prompt\'s words for a lock this computer can positively not have taken', () => {
    show('me@example.com', summary.permission, local(false));
    expect(screen.getByTestId('project-lock')).toHaveTextContent(/^Held by this account, not by this computer$/);
  });
  // A copy here (a session that crashed over it leaves exactly that), a recent
  // file linked to the project, or no local read at all: cannot tell.
  it.each([
    ['a copy on this computer', local(false, true), false],
    ['a recent file linked to the project', local(false), true],
    ['no local read', {}, false],
  ] as const)('says only that the account holds it with %s', (_case, states, linked) => {
    show('me@example.com', summary.permission, states, linked);
    expect(screen.getByTestId('project-lock')).toHaveTextContent(/^Held by this account$/);
  });
  it('names another lock owner', () => {
    show('other@example.com');
    expect(screen.getByTestId('project-lock')).toHaveTextContent('Currently being edited by other@example.com');
  });
  it('does not invent permission or unlocked state before the server answers', () => {
    show(undefined, null);
    expect(screen.getByTestId('project-permission')).toHaveTextContent('Permission: Unknown');
    expect(screen.getByTestId('project-lock')).toHaveTextContent('Editing status unknown');
  });
  it('says why an account without write access cannot edit, whoever holds the lock', () => {
    show('other@example.com', 'READ_ONLY');
    expect(screen.getByTestId('project-lock'))
      .toHaveTextContent(/^Read-only - your SpeleoDB access does not allow editing$/);
  });
  it('signed out, a cached copy says it is a read-only copy and claims no permission', () => {
    const lock = resolveSpeleoLock({ is_remote: true, speleodb_project_id: summary.id }, {}, null);
    render(<SpeleoProjectInfo summary={{ ...summary, permission: null }} lock={lock} testIdPrefix="project" cached={{
      project_id: summary.id, project_name: 'Cave', local_path: '/cache/project.stalx',
      ownership: 'signed_out', has_unpushed_edits: false, state_unknown: false, is_open: false,
    }} />);
    expect(screen.getByTestId('project-lock')).toHaveTextContent(/^Read-only copy - connect to SpeleoDB to edit$/);
    expect(screen.queryByTestId('project-permission')).toBeNull();
  });
});
