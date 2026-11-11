import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/react';
import { SpeleoProjectInfo } from '../SpeleoProjectInfo';
import { resolveSpeleoLock } from '../SpeleoLockBadge';
import type { SpeleoProjectSummary } from '../../../types/ipc.generated';

const summary: SpeleoProjectSummary = {
  id: 'project', name: 'Cave', permission: 'ADMIN', lock_holder: null,
  type_tag: 'stalx', latest_commit_sha: null,
};

function show(holder: string | null | undefined, permission = summary.permission) {
  const lock = resolveSpeleoLock({ is_remote: true, speleodb_project_id: summary.id },
    holder === undefined ? {} : { project: holder }, 'me@example.com');
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
