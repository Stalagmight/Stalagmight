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

vi.mock('../../utils/telemetry', async () =>
  (await import('../../test/factories/telemetry')).telemetryWith({ setProjectActionSource: vi.fn() }),
);

import MergeProjectsDialog from './MergeProjectsDialog';
import { saveDialog } from '../../utils/fileDialogs';
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
    section_count: 1,
    total_length: 10,
  } as ProjectMetadata);

const ok = (warnings: ValidationWarning[]): CommandResult =>
  ({ success: true, error: null, warnings, section_ids: [] } as CommandResult);

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

  it('does not toast when the merge succeeds with no warnings', async () => {
    mergeProjects.mockResolvedValue(ok([]));

    render(<MergeProjectsDialog />);
    await userEvent.click(await screen.findByTestId('merge-projects-confirm-button'));

    expect(showToast).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
  });
});

// The Save dialog's proposed location is the first project's folder. Splitting
// on '/' alone left a Windows path with no folder at all, so the dialog was
// offered '/CaveA_merged.stalx' instead of a file next to the source (#576).
describe('MergeProjectsDialog default output location', () => {
  const pathProject = (name: string, file_path: string): ProjectMetadata =>
    ({ ...projectMeta(name), file_path } as ProjectMetadata);

  const proposedDefaultPath = async (firstPath: string): Promise<string | undefined> => {
    useUIStore.setState({
      dialogData: { selectedProjects: [pathProject('CaveA', firstPath), pathProject('CaveB', '/tmp/CaveB.stalx')] },
    });
    mergeProjects.mockResolvedValue(ok([]));
    vi.mocked(saveDialog).mockClear();

    render(<MergeProjectsDialog />);
    await userEvent.click(await screen.findByTestId('merge-projects-confirm-button'));

    expect(saveDialog).toHaveBeenCalledTimes(1);
    return vi.mocked(saveDialog).mock.calls[0]?.[0]?.defaultPath;
  };

  it('proposes the first project folder for a Windows backslash path', async () => {
    expect(await proposedDefaultPath('C:\\caves\\CaveA.stalx')).toBe('C:\\caves\\CaveA_merged.stalx');
  });

  it('keeps proposing the first project folder for a POSIX path', async () => {
    expect(await proposedDefaultPath('/home/user/caves/CaveA.stalx')).toBe('/home/user/caves/CaveA_merged.stalx');
  });

  // On Linux and macOS '\' is an ordinary file-name character, so a path that is
  // not Windows-shaped must get exactly the string the '/'-only split gave.
  it.each([
    '/home/u/caves/CaveA.stalx',
    '/home/u/we\\ird.stalx',
    '/CaveA.stalx',
    'CaveA.stalx',
    '/home/u/caves//CaveA.stalx',
  ])('proposes the same folder as before for the POSIX path %s', async (firstPath) => {
    const before = `${firstPath.substring(0, firstPath.lastIndexOf('/'))}/CaveA_merged.stalx`;
    expect(await proposedDefaultPath(firstPath)).toBe(before);
  });

  it.each([
    ['C:\\CaveA.stalx', 'C:\\CaveA_merged.stalx'],
    ['\\\\srv\\share\\CaveA.stalx', '\\\\srv\\share\\CaveA_merged.stalx'],
    ['C:/caves\\CaveA.stalx', 'C:/caves\\CaveA_merged.stalx'],
  ])('proposes the first project folder for the Windows path %s', async (firstPath, expected) => {
    expect(await proposedDefaultPath(firstPath)).toBe(expected);
  });
});
