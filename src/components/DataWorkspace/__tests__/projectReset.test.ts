import { describe, expect, it } from 'vitest';
import '../projectReset';
import { useDataWorkspaceStore } from '../store';
import { useProjectStore } from '../../../store/projectStore';

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
});
