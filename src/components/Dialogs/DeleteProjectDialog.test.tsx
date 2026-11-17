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

beforeEach(() => {
  useUIStore.setState({
    closeDialog: vi.fn(),
    dialogData: { path: '/tmp/cave.stalx', name: 'My Cave' },
  });
  useProjectStore.setState({
    deleteProject: vi.fn(async () => ({ success: true, warnings: [], error: null, line_ids: [] })),
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
});
