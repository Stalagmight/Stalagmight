/**
 * #391: the "import as new project?" confirm dialog that replaces the old
 * close-first dead end for a new-project-only format (GPX/Ariane/Compass/Walls)
 * picked while a project is already open.
 *
 * #576 HFI-4-D1 "parse first, then switch": confirming no longer closes the
 * project here. It hands the router the guarded close and stays up showing the
 * parse in progress; the router closes the project only after a successful parse
 * (its rules are pinned in utils/__tests__/openOrImport.importAsNew.test.ts).
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/react';
import { deferred } from '../../../test/factories/async';

const closeDialog = vi.fn();
const handleCloseProject = vi.fn();

vi.mock('react-i18next', async () => (await import('../../../test/factories/i18n')).translationStub('keyAndOptions'));

const dialogData: { path?: string; fileName?: string } = {};

vi.mock('../../../store/uiStore', () => ({
  useUIStore: (sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData }),
}));

let projectName: string | undefined;
vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: projectName ? { name: projectName } : null }),
}));

vi.mock('../../../hooks/useProject', () => ({
  useProject: () => ({ handleCloseProject }),
}));

vi.mock('../../../utils/openOrImport', () => ({ routeOpenedFile: vi.fn() }));

vi.mock('../../../utils/telemetry', async () =>
  (await import('../../../test/factories/telemetry')).telemetryStubs({ clearProjectActionSource: vi.fn() }),
);

vi.mock('../DialogWrapper', async () => (await import('../../../test/factories/components')).dialogWrapperStub());

vi.mock('../../common/Button', async () => (await import('../../../test/factories/components')).buttonStub());

import { routeOpenedFile } from '../../../utils/openOrImport';
import { clearProjectActionSource } from '../../../utils/telemetry';
import ImportAsNewProjectDialog from '../ImportAsNewProjectDialog';

describe('ImportAsNewProjectDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    dialogData.path = '/cave/track.gpx';
    dialogData.fileName = 'track.gpx';
    projectName = 'My Cave';
    handleCloseProject.mockResolvedValue({ success: true });
  });

  it('renders nothing without a pending path', () => {
    dialogData.path = '';
    const { container } = render(<ImportAsNewProjectDialog />);
    expect(container).toBeEmptyDOMElement();
  });

  it('confirming hands the picked file and the guarded close to the router, closing nothing itself', async () => {
    render(<ImportAsNewProjectDialog />);
    fireEvent.click(screen.getByTestId('import-as-new-project-confirm'));

    await waitFor(() =>
      expect(routeOpenedFile).toHaveBeenCalledWith('/cave/track.gpx', undefined, {
        importAsNew: { closeCurrent: handleCloseProject },
      }),
    );
    // The project stays open while the file parses: the router closes it, and
    // only after a successful parse (#576 HFI-4-D1).
    expect(handleCloseProject).not.toHaveBeenCalled();
    // The dialog stays up: whatever comes next replaces it.
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('shows the parse in progress until the router settles, with Close and import disabled', async () => {
    const parse = deferred();
    vi.mocked(routeOpenedFile).mockReturnValueOnce(parse.promise);
    render(<ImportAsNewProjectDialog />);
    expect(screen.queryByTestId('import-as-new-project-parsing')).toBeNull();

    fireEvent.click(screen.getByTestId('import-as-new-project-confirm'));

    const status = await screen.findByTestId('import-as-new-project-parsing');
    expect.soft(status).toHaveAttribute('role', 'status');
    expect.soft(status).toHaveTextContent('dialogs.importAsNewProject.parsing:{"fileName":"track.gpx"}');
    expect.soft(screen.getByTestId('import-as-new-project-confirm')).toBeDisabled();
    expect.soft(screen.getByTestId('import-as-new-project-cancel')).toBeEnabled();

    // A router run that leaves this dialog up (a close refused without a prompt)
    // gives the confirm back.
    parse.resolve();
    await waitFor(() => expect(screen.queryByTestId('import-as-new-project-parsing')).toBeNull());
    expect(screen.getByTestId('import-as-new-project-confirm')).toBeEnabled();
  });

  it('cancelling while the file parses abandons the import: the source is cleared and the dialog closes', async () => {
    vi.mocked(routeOpenedFile).mockReturnValueOnce(deferred().promise);
    render(<ImportAsNewProjectDialog />);
    fireEvent.click(screen.getByTestId('import-as-new-project-confirm'));
    await screen.findByTestId('import-as-new-project-parsing');

    fireEvent.click(screen.getByTestId('import-as-new-project-cancel'));

    expect.soft(closeDialog).toHaveBeenCalled();
    expect.soft(clearProjectActionSource).toHaveBeenCalled();
    expect.soft(handleCloseProject).not.toHaveBeenCalled();
  });

  it('cancelling closes without touching the project or the picked file, clearing the pending source', () => {
    render(<ImportAsNewProjectDialog />);
    fireEvent.click(screen.getByTestId('import-as-new-project-cancel'));
    expect(handleCloseProject).not.toHaveBeenCalled();
    expect(routeOpenedFile).not.toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(clearProjectActionSource).toHaveBeenCalled();
  });
});
