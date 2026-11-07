/**
 * #576 HFI-4-D2 "prompt after a good parse": "Import as new project" over a project whose
 * close must ask first (unsaved changes, a SpeleoDB hand-back) parses the file FIRST and
 * only then asks, handing the prompt the parsed file as `pendingImport`
 * (`routeOpenedFile`, openOrImport.importAsNew.test.ts). These are the prompts' halves:
 * confirming closes the project and then opens the new-project dialog with that file;
 * cancelling keeps the project and drops the file. Before, the prompts asked BEFORE the
 * parse, carried nothing, and their confirm only closed the project: the import ended
 * there and the file had to be picked again.
 *
 * `.tsx` so vitest runs it under jsdom.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
vi.mock('../../../utils/ipc');
import '../../../i18n';
import UnsavedChangesDialog from '../UnsavedChangesDialog';
import SpeleoCloseDialog from '../SpeleoCloseDialog';
import { useUIStore } from '../../../store/uiStore';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore } from '../../../store/speleoStore';
import { handBackProject, readPendingExit } from '../../../utils/quit';
import { invokeIpcMock } from '../../../test/factories/ipc';
import { makeSpeleoLink } from '../../../test/factories/speleo';

/** What the router hands the prompt: the new-project-for-import dialog data of a parsed .srv. */
const WALLS = { project_name: 'CaveA', surveys: [] };
const PENDING = { wallsFile: WALLS, filePath: '/cave/CaveA.srv', defaultName: 'CaveA', importType: 'walls' };

const REAL_OPEN_DIALOG = useUIStore.getState().openDialog;
const REAL_CLOSE_DIALOG = useUIStore.getState().closeDialog;
let closeProject: ReturnType<typeof vi.fn>;

/** What App renders in the dialog slot, for the dialogs these flows move between. */
function DialogHost() {
  const active = useUIStore((s) => s.activeDialog);
  if (active === 'unsaved-changes') return <UnsavedChangesDialog />;
  if (active === 'speleodb-close') return <SpeleoCloseDialog />;
  return active ? <div data-testid={`open-${active}`} /> : null;
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  closeProject = vi.fn(async () => {
    useProjectStore.setState({ project: null, isDirty: false });
    return { success: true };
  });
  useProjectStore.setState({
    project: { name: 'ProjectA', zero_datums: [{}] } as never,
    isDirty: true,
    closeProject: closeProject as never,
  });
  useSpeleoStore.setState({ link: null, projectLinkKind: 'local' });
  useUIStore.setState({
    activeDialog: 'unsaved-changes',
    dialogData: { pendingImport: PENDING },
    importContext: null,
    openDialog: REAL_OPEN_DIALOG,
    closeDialog: REAL_CLOSE_DIALOG,
  });
});

describe('the unsaved-changes prompt after a good parse (#576 HFI-4-D2)', () => {
  it('Discard closes the project, then opens the new-project dialog with the parsed file', async () => {
    render(<DialogHost />);
    await userEvent.click(screen.getByTestId('unsaved-changes-discard'));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('new-project-for-import'));
    expect(closeProject).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().dialogData).toEqual(PENDING);
    expect(useUIStore.getState().importContext).toBe('new-project');
  });

  it('Cancel keeps the open project and drops the parsed file', async () => {
    render(<DialogHost />);
    await userEvent.click(screen.getByTestId('unsaved-changes-cancel'));

    expect(useUIStore.getState().activeDialog).toBeNull();
    expect(closeProject).not.toHaveBeenCalled();
    expect(useProjectStore.getState().project).toMatchObject({ name: 'ProjectA' });
    expect(useUIStore.getState().importContext).toBeNull();
  });

  it('a close that fails opens no new-project dialog', async () => {
    closeProject.mockResolvedValue({ success: false, error: 'busy' });
    render(<DialogHost />);
    await userEvent.click(screen.getByTestId('unsaved-changes-discard'));

    await waitFor(() => expect(closeProject).toHaveBeenCalled());
    expect(useUIStore.getState().activeDialog).toBeNull();
  });

  // #576 HFI-4-D3: the prompt names what it is about (the import, the project it closes),
  // not the generic "Are you sure you want to go back?"; the buttons stay the same.
  it('asks "Import as a new project?" and names the project whose changes are lost', () => {
    render(<DialogHost />);
    const dialog = screen.getByTestId('unsaved-changes-dialog');

    expect(dialog).toHaveTextContent('Import as a new project?');
    expect(dialog).toHaveTextContent('Unsaved changes in ProjectA will be lost.');
    expect(dialog).not.toHaveTextContent('go back');
    expect(dialog).not.toHaveTextContent('Your changes will be lost.');
    expect(screen.getByTestId('unsaved-changes-cancel')).toHaveTextContent('Cancel');
    expect(screen.getByTestId('unsaved-changes-discard')).toHaveTextContent('Discard Changes');
  });

  // #576 RV13-9: Discard does not end the flow; the prompt says what follows it.
  it('says the import continues after it', () => {
    render(<DialogHost />);
    expect(screen.getByTestId('unsaved-changes-dialog')).toHaveTextContent(
      'Unsaved changes in ProjectA will be lost. The import continues after that.',
    );
  });

  it('the plain unsaved-changes prompt (no parsed file) keeps its own text', () => {
    useUIStore.setState({ dialogData: {} });
    render(<DialogHost />);
    const dialog = screen.getByTestId('unsaved-changes-dialog');

    expect(dialog).toHaveTextContent('You have unsaved changes. Are you sure you want to go back?');
    expect(dialog).toHaveTextContent('Your changes will be lost.');
    expect(dialog).not.toHaveTextContent('Import as a new project?');
    expect(dialog).not.toHaveTextContent('The import continues');
  });
});

describe('the SpeleoDB hand-back prompt after a good parse (#576 HFI-4-D2)', () => {
  beforeEach(() => {
    useSpeleoStore.setState({ link: makeSpeleoLink({ editState: 'read-only', hasUnpushedEdits: true }), projectLinkKind: 'remote' });
    useProjectStore.setState({ isDirty: false });
    useUIStore.setState({ activeDialog: 'speleodb-close', dialogData: { pendingImport: PENDING } });
  });

  it('closing hands the project back, then opens the new-project dialog with the parsed file', async () => {
    render(<DialogHost />);
    await userEvent.click(screen.getByTestId('speleodb-close-anyway'));

    await waitFor(() => expect(useUIStore.getState().activeDialog).toBe('new-project-for-import'));
    expect(closeProject).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().dialogData).toEqual(PENDING);
    expect(useUIStore.getState().importContext).toBe('new-project');
  });

  // #576 RV13-9: like the Open / New Project notes, it says the request survives the prompt.
  it('says the import continues once the project is closed', () => {
    render(<DialogHost />);
    expect(screen.getByTestId('speleodb-close-pending-note')).toHaveTextContent(
      'Once this project is closed, the import continues.',
    );
  });

  it('the parsed file rides through the unwound-push confirmation too (readPendingExit / handBackProject)', async () => {
    const exit = readPendingExit({ pendingExit: { kind: 'close', pendingImport: PENDING } });
    expect(exit).toMatchObject({ kind: 'close', pendingImport: PENDING });
    if (!exit || exit.kind !== 'close') throw new Error('no close exit');

    await handBackProject(exit);

    expect(closeProject).toHaveBeenCalledTimes(1);
    expect(useUIStore.getState().activeDialog).toBe('new-project-for-import');
    expect(useUIStore.getState().dialogData).toEqual(PENDING);
  });

  it('a pendingImport that is not an object is not trusted', () => {
    expect(readPendingExit({ pendingExit: { kind: 'close', pendingImport: 'x' } })).toEqual({
      kind: 'close',
      pendingOpenPath: null,
      pendingNewProject: false,
    });
  });
});
