/**
 * Component test - the delete-project confirm dialog frames a REMOTE (SpeleoDB)
 * project as a recoverable "local copy" delete, and a local project as a permanent
 * one. The title, body, warning, and button copy all fork on `dialogData.isRemote`.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

import DeleteProjectDialog from './DeleteProjectDialog';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { accepted } from '../../store/commandResult';
import type { DeleteProjectOutcome } from '../../types/ipc/DeleteProjectOutcome';

beforeEach(() => {
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { path: '/tmp/cave.stalx', name: 'My Cave' },
  });
  useProjectStore.setState({
    deleteProject: vi.fn(async () => accepted<DeleteProjectOutcome>({ session_closed: false, warnings: [] })),
  });
});

describe('DeleteProjectDialog', () => {
  it('frames a local project as a permanent, unrecoverable delete', () => {
    useUIStore.setState({ dialogData: { path: '/tmp/cave.stalx', name: 'My Cave', isRemote: false } });
    render(<DeleteProjectDialog />);

    expect(screen.getByText('Delete Project?')).toBeInTheDocument();
    expect(screen.getByText(/Permanently delete/)).toBeInTheDocument();
    expect(screen.getByText(/It cannot be undone\./)).toBeInTheDocument();
    expect(screen.getByTestId('delete-project-confirm')).toHaveTextContent('Delete Project');
  });

  it('frames a remote project as a recoverable local-copy delete, warning about unpushed edits', () => {
    useUIStore.setState({ dialogData: { path: '/tmp/cave.stalx', name: 'My Cave', isRemote: true } });
    render(<DeleteProjectDialog />);

    expect(screen.getByText('Delete local copy?')).toBeInTheDocument();
    expect(screen.getByText(/Delete the local copy of/)).toBeInTheDocument();
    expect(screen.getByText(/stays on SpeleoDB and can be downloaded again/)).toBeInTheDocument();
    // The recoverable framing must still warn that unpushed local work is lost.
    expect(screen.getByText(/not yet pushed to SpeleoDB will be lost/)).toBeInTheDocument();
    expect(screen.queryByText(/It cannot be undone\./)).toBeNull();
    expect(screen.getByTestId('delete-project-confirm')).toHaveTextContent('Delete local copy');
  });

  // #576 D20: the copy is a <Trans> template now (`Permanently delete
  // <name>{{name}}</name>?`). A name or path that looks like markup must render
  // as typed, not be parsed as a tag of the template.
  it('renders a project name and path that look like markup as typed', () => {
    useUIStore.setState({ dialogData: { path: '/caves/a</path>b.stalx', name: 'Main <A> & </name>B', isRemote: false } });
    render(<DeleteProjectDialog />);

    expect(screen.getByText('Main <A> & </name>B')).toHaveClass('font-medium');
    expect(screen.getByText(/^Permanently delete/).textContent).toBe('Permanently delete Main <A> & </name>B?');
    expect(screen.getByText('/caves/a</path>b.stalx')).toHaveClass('break-all');
  });
});
