/**
 * Component test (#448 followups) - a successful merge whose `CommandResult`
 * carries a warning (the surface_elevation no-clobber mismatch) must reach the
 * user. The dialog closes on success, so nothing else is watching
 * `result.warnings` - this pins the toast wiring per docs/notifications.md.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

vi.mock('../../utils/fileDialogs', () => ({
  saveDialog: vi.fn(async () => '/tmp/merged.stalx'),
  ensureExtension: vi.fn((p: string) => p),
  projectSaveFilters: vi.fn(() => []),
  PROJECT_EXTENSION: '.stalx',
}));

vi.mock('../../utils/telemetry', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/telemetry')>()),
  trackDialogComplete: vi.fn(),
  setProjectActionSource: vi.fn(),
}));

import MergeProjectsDialog from './MergeProjectsDialog';
import { useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';
import type { CommandResult, ValidationWarning } from '../../types/commands';
import type { ProjectMetadata } from '../../types/project';

const SURFACE_ELEVATION_WARNING: ValidationWarning = {
  code: 'SURFACE_ELEVATION_MISMATCH',
  message:
    "Source project's entrance elevation (150.00 m) differs from the merged project's value (100.00 m); kept 100.00 m",
  severity: 'warning',
};

const projectMeta = (name: string): ProjectMetadata =>
  ({
    name,
    file_path: `/tmp/${name}.stalx`,
    line_count: 1,
    total_length: 10,
  } as ProjectMetadata);

const ok = (warnings: ValidationWarning[]): CommandResult =>
  ({ success: true, error: null, warnings, line_ids: [] } as CommandResult);

let mergeProjects: ReturnType<typeof vi.fn>;
let showToast: ReturnType<typeof vi.fn>;
let closeDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  mergeProjects = vi.fn();
  showToast = vi.fn();
  closeDialog = vi.fn();
  useProjectStore.setState({
    mergeProjects,
    loadProject: vi.fn(),
  });
  useUIStore.setState({
    closeDialog,
    showToast,
    dialogData: { selectedProjects: [projectMeta('A'), projectMeta('B')] },
  });
});

describe('MergeProjectsDialog warning surfacing', () => {
  it('toasts the merge warning when the merge succeeds with one', async () => {
    mergeProjects.mockResolvedValue(ok([SURFACE_ELEVATION_WARNING]));

    render(<MergeProjectsDialog />);
    await userEvent.click(await screen.findByTestId('merge-projects-confirm-button'));

    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(
      expect.stringContaining(SURFACE_ELEVATION_WARNING.message),
      expect.any(String),
      'warning'
    );
    expect(closeDialog).toHaveBeenCalled();
  });

  it('names every converted source in the one toast (RS 8.2, 8.3)', async () => {
    const converted: ValidationWarning = {
      code: 'SOURCE_CONVERTED',
      message:
        "'B' was written by an earlier version and was converted as it was read (the file itself is unchanged): 1 station now has a letter suffix",
      severity: 'warning',
    };
    mergeProjects.mockResolvedValue(ok([SURFACE_ELEVATION_WARNING, converted]));

    render(<MergeProjectsDialog />);
    await userEvent.click(await screen.findByTestId('merge-projects-confirm-button'));

    expect(showToast).toHaveBeenCalledTimes(1);
    const [message] = showToast.mock.calls[0] as [string];
    expect(message).toContain(SURFACE_ELEVATION_WARNING.message);
    expect(message).toContain(converted.message);
  });

  it('does not toast when the merge succeeds with no warnings', async () => {
    mergeProjects.mockResolvedValue(ok([]));

    render(<MergeProjectsDialog />);
    await userEvent.click(await screen.findByTestId('merge-projects-confirm-button'));

    expect(showToast).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
  });
});
