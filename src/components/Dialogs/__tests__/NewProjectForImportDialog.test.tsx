/**
 * #391 one-way messaging: the new-project-for-import dialog is the last stop
 * before a foreign survey file becomes a Stalagmight project, so it must say
 * so plainly - interpolated with the actual file name, not a generic notice.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub('keyAndOptions'));

const dialogData: {
  gpxFile?: { name: string | null; waypoints: unknown[]; tracks: unknown[]; routes: unknown[] };
  filePath?: string | undefined;
  importType?: string;
  defaultName?: string;
} = {};

const closeDialog = vi.fn();
const openDialog = vi.fn();
const setImportContext = vi.fn();

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData, openDialog, setImportContext }),
}));

const createProject = vi.fn();
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ createProject }),
}));

vi.mock('../DialogWrapper', async () =>
  (await import('../../../test/factories/components')).dialogWrapperStub({ title: 'testid' }),
);

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import NewProjectForImportDialog from '../NewProjectForImportDialog';

describe('NewProjectForImportDialog (#391)', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    dialogData.gpxFile = { name: 'Track', waypoints: [], tracks: [], routes: [] };
    dialogData.filePath = '/cave/exports/track.gpx';
    dialogData.importType = 'gpx';
    dialogData.defaultName = 'Track';
  });

  it('states the one-way promise with the actual file name', () => {
    render(<NewProjectForImportDialog />);

    const promise = screen.getByTestId('new-project-for-import-one-way-promise');
    expect(promise).toHaveTextContent(
      'dialogs.newProjectForImport.oneWayPromise:{"fileName":"track.gpx"}',
    );
  });

  it('derives the file name from a Windows-style path too', () => {
    dialogData.filePath = 'C:\\caves\\exports\\track.gpx';
    render(<NewProjectForImportDialog />);

    expect(screen.getByTestId('new-project-for-import-one-way-promise')).toHaveTextContent(
      'dialogs.newProjectForImport.oneWayPromise:{"fileName":"track.gpx"}',
    );
  });

  it('renders nothing for the promise line without a pending file path', () => {
    dialogData.filePath = undefined;
    render(<NewProjectForImportDialog />);

    expect(screen.queryByTestId('new-project-for-import-one-way-promise')).not.toBeInTheDocument();
  });
});
