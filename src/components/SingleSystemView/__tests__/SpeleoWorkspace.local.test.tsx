import { beforeEach, expect, it, vi } from 'vitest';
import { act, fireEvent, render, screen, within } from '@testing-library/react';
import { LeftWorkspace } from '../LeftWorkspace';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
vi.mock('../../../utils/ipc', () => ({ invokeIpc: vi.fn(), getErrorMessage: String }));
beforeEach(() => {
  useProjectStore.setState({ project: { name: 'Local cave', file_path: '/local.stalx', zero_datums: [] } as never });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local', remoteProjectId: null, session: null, busy: false });
  useUIStore.setState({ activeDialog: null, dialogData: {}, appMode: 'survey', leftWorkspace: 'sections', surveyLeftWorkspace: 'sections' });
});
it('local SpeleoDB tab offers only conversion and opens the project creation form', () => {
  render(<LeftWorkspace sections={<div>Sections content</div>} />);
  fireEvent.click(screen.getByRole('tab', { name: 'SpeleoDB' }));
  const panel = screen.getByRole('tabpanel', { name: 'SpeleoDB' });
  expect(within(panel).getAllByRole('button')).toHaveLength(1);
  fireEvent.click(within(panel).getByRole('button', { name: 'Convert to a SpeleoDB project' }));
  expect(useUIStore.getState().activeDialog).toBe('publish-to-speleodb');
  expect(screen.queryByTestId('speleodb-workspace-upload')).toBeNull();
  expect(screen.queryByTestId('speleodb-workspace-section-history')).toBeNull();
});
it('the local SpeleoDB tab, opened outside Survey, offers conversion without section editors', () => {
  render(<LeftWorkspace sections={<div>Sections content</div>} />);
  for (const mode of ['data', 'plan', 'export'] as const) {
    act(() => useUIStore.getState().setAppMode(mode));
    act(() => useUIStore.getState().openSpeleoWorkspace());
    expect(screen.getByRole('button', { name: 'Convert to a SpeleoDB project' })).toBeVisible();
    expect(screen.queryByRole('tab', { name: 'Sections' })).toBeNull();
  }
});
