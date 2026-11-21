import { describe, expect, it } from 'vitest';
import '../projectReset';
import { useDataWorkspaceStore } from '../store';
import { useProjectStore } from '../../../store/projectStore';

const startLoad = () => {
  const { loading } = useProjectStore.getState();
  useProjectStore.setState({ loading: { ...loading, clientOpId: loading.clientOpId + 1 } });
};

describe('Data workspace project reset', () => {
  it('clears project state when another project opens or the project closes, keeps it across edits', () => {
    useProjectStore.setState({ project: { created_at: 'A', modified_at: '1' } as never });
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [7] });
    useDataWorkspaceStore.getState().setReturnToData({ rowKey: 'st:7:0' });

    useProjectStore.setState({ project: { created_at: 'A', modified_at: '2' } as never });
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([7]);

    useProjectStore.setState({ project: { created_at: 'B', modified_at: '1' } as never });
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toBeNull();
    expect(useDataWorkspaceStore.getState().returnToData).toBeNull();

    useDataWorkspaceStore.getState().setFilters({ sectionIds: [3] });
    useProjectStore.setState({ project: null });
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toBeNull();
  });

  it('keeps the view across a Save As, but resets when the copy (same creation stamp) is opened', () => {
    startLoad();
    useProjectStore.setState({ project: { created_at: 'C', file_path: '/caves/a.stalx' } as never });
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [2] });
    useDataWorkspaceStore.getState().setActiveRowKey('st:2:1');

    // Save As re-points the open project; the same data stays on screen.
    useProjectStore.setState({ project: { created_at: 'C', file_path: '/caves/b.stalx' } as never });
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([2]);

    // Opening the other file: a new load, a project with the same created_at.
    startLoad();
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([2]);
    useProjectStore.setState({ project: { created_at: 'C', file_path: '/caves/a.stalx' } as never });
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toBeNull();
    expect(useDataWorkspaceStore.getState().activeRowKey).toBeNull();
  });

  it('a load that starts and is cancelled leaves the open project state alone', () => {
    useProjectStore.setState({ project: { created_at: 'D' } as never });
    useDataWorkspaceStore.getState().setFilters({ sectionIds: [5] });
    startLoad();
    useProjectStore.setState({ isDirty: true } as never);
    expect(useDataWorkspaceStore.getState().filters.sectionIds).toEqual([5]);
  });
});
