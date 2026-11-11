/**
 * #674: the open-or-add prompt raised when a `.stalx` is picked through an
 * in-project Import door.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen, fireEvent, act } from '@testing-library/react';

const closeDialog = vi.fn();

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, opts?: Record<string, unknown>) => (opts ? `${key}:${JSON.stringify(opts)}` : key),
  }),
}));

const dialogData: { path?: string; fileName?: string } = {};

let activeDialog: string | null = 'add-or-open-project';

vi.mock('../../../store/uiStore', () => ({
  useUIStore: Object.assign((sel: (s: unknown) => unknown) => sel({ closeDialog, dialogData }), {
    getState: () => ({ activeDialog }),
  }),
}));

vi.mock('../../../store/projectStore', () => ({
  useProjectStore: (sel: (s: unknown) => unknown) => sel({ project: { name: 'My Cave' } }),
}));

let blocked = false;
vi.mock('../../../hooks/useEditingBlocked', () => ({
  useEditingBlocked: () => blocked,
  isEditingBlockedNow: () => blocked,
}));

vi.mock('../../../utils/openOrImport', () => ({ routeOpenedFile: vi.fn() }));
vi.mock('../../../utils/addProjectFile', () => ({ runAddProjectFile: vi.fn() }));

vi.mock('../../../utils/telemetry', () => ({
  trackDialogComplete: vi.fn(),
  clearProjectActionSource: vi.fn(),
}));

let wrapperOnClose: () => void = () => {};
vi.mock('../DialogWrapper', () => ({
  DialogWrapper: ({ children, onClose }: { children: React.ReactNode; onClose: () => void }) => {
    wrapperOnClose = onClose;
    return <div>{children}</div>;
  },
}));

import { routeOpenedFile } from '../../../utils/openOrImport';
import { runAddProjectFile } from '../../../utils/addProjectFile';
import { trackDialogComplete, clearProjectActionSource } from '../../../utils/telemetry';
import AddOrOpenProjectDialog from '../AddOrOpenProjectDialog';

describe('AddOrOpenProjectDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    blocked = false;
    activeDialog = 'add-or-open-project';
    dialogData.path = '/cave/other.stalx';
    dialogData.fileName = 'other.stalx';
  });

  it('renders nothing without a pending path', () => {
    dialogData.path = '';
    const { container } = render(<AddOrOpenProjectDialog />);
    expect(container).toBeEmptyDOMElement();
  });

  it('puts initial focus on Cancel, never on the Add button', () => {
    render(<AddOrOpenProjectDialog />);
    expect(screen.getByTestId('add-or-open-project-cancel')).toHaveAttribute('data-autofocus');
    expect(screen.getByTestId('add-or-open-project-add')).not.toHaveAttribute('data-autofocus');
  });

  it('two clicks in the same task start the add once', () => {
    vi.mocked(runAddProjectFile).mockReturnValue(new Promise(() => {}));
    render(<AddOrOpenProjectDialog />);
    const add = screen.getByTestId('add-or-open-project-add');
    act(() => {
      add.click();
      add.click();
    });
    expect(runAddProjectFile).toHaveBeenCalledTimes(1);
  });

  it('ellipsizes a long project name in the Add label and keeps the full text as a tooltip', () => {
    render(<AddOrOpenProjectDialog />);
    const add = screen.getByTestId('add-or-open-project-add');
    expect(screen.getByTestId('add-or-open-project-add-label')).toHaveClass('truncate');
    expect(add).toHaveClass('max-w-full');
    expect(add).toHaveAttribute('title', add.textContent ?? '');
    expect(screen.getByTestId('add-or-open-project-open')).toHaveClass('whitespace-nowrap');
  });

  it('names the open project on the Add choice', () => {
    render(<AddOrOpenProjectDialog />);
    expect(screen.getByTestId('add-or-open-project-add').textContent).toContain('My Cave');
  });

  it('"Open as project" re-routes through the guarded open and never adds', () => {
    render(<AddOrOpenProjectDialog />);
    fireEvent.click(screen.getByTestId('add-or-open-project-open'));
    expect(closeDialog).toHaveBeenCalled();
    expect(routeOpenedFile).toHaveBeenCalledWith('/cave/other.stalx', 'Open Failed', { swapConfirmed: true });
    expect(runAddProjectFile).not.toHaveBeenCalled();
    expect(trackDialogComplete).toHaveBeenCalledWith('add-or-open-project');
  });

  it('"Add to" appends the file and never opens it as a project', async () => {
    vi.mocked(runAddProjectFile).mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
    render(<AddOrOpenProjectDialog />);
    await act(async () => fireEvent.click(screen.getByTestId('add-or-open-project-add')));
    expect(runAddProjectFile).toHaveBeenCalledWith('/cave/other.stalx');
    expect(routeOpenedFile).not.toHaveBeenCalled();
  });

  /**
   * The add copies and loads a whole project. While it runs, this dialog must
   * keep the slot: an active dialog is what makes the router, Ctrl+O and the
   * launch channel refuse another open. On success the add's own tail closes or
   * replaces the dialog (and records its completion, once), so the dialog
   * neither closes itself nor marks itself complete.
   *
   * Injection: restore `closeDialog()` before `runAddProjectFile` in handleAdd
   * and the slot empties while the add is still in flight.
   */
  it('keeps the slot busy until the add resolves, then leaves the ending to the tail', async () => {
    let resolveAdd: (r: unknown) => void = () => {};
    vi.mocked(runAddProjectFile).mockReturnValue(new Promise((r) => { resolveAdd = r; }) as never);
    render(<AddOrOpenProjectDialog />);
    await act(async () => fireEvent.click(screen.getByTestId('add-or-open-project-add')));

    expect(closeDialog).not.toHaveBeenCalled();
    expect((screen.getByTestId('add-or-open-project-add') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('add-or-open-project-open') as HTMLButtonElement).disabled).toBe(true);
    expect((screen.getByTestId('add-or-open-project-cancel') as HTMLButtonElement).disabled).toBe(true);
    expect(screen.getByTestId('add-or-open-project-add').textContent).toBe('dialogs.addOrOpenProject.adding');
    wrapperOnClose(); // Escape / X / backdrop
    expect(closeDialog).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('add-or-open-project-add'));
    expect(runAddProjectFile).toHaveBeenCalledTimes(1);

    // The tail finished the flow: orphan placement replaced this dialog.
    activeDialog = 'orphan-placement';
    await act(async () => resolveAdd({ success: true, warnings: [], error: null, section_ids: [7] }));
    expect(closeDialog).not.toHaveBeenCalled();
    expect(trackDialogComplete).not.toHaveBeenCalled();
  });

  it('a failed add shows why inline and hands the choice back', async () => {
    vi.mocked(runAddProjectFile).mockResolvedValue({ success: false, warnings: [], error: 'boom', section_ids: [] });
    render(<AddOrOpenProjectDialog />);
    await act(async () => fireEvent.click(screen.getByTestId('add-or-open-project-add')));
    expect(screen.getByTestId('add-or-open-project-error').textContent).toBe('boom');
    expect(closeDialog).not.toHaveBeenCalled();
    expect((screen.getByTestId('add-or-open-project-open') as HTMLButtonElement).disabled).toBe(false);
    expect((screen.getByTestId('add-or-open-project-add') as HTMLButtonElement).disabled).toBe(false);
  });

  it('closes itself when the add landed but its project moved (the tail was dropped)', async () => {
    vi.mocked(runAddProjectFile).mockResolvedValue({ success: true, warnings: [], error: null, section_ids: [] });
    render(<AddOrOpenProjectDialog />);
    await act(async () => fireEvent.click(screen.getByTestId('add-or-open-project-add')));
    expect(closeDialog).toHaveBeenCalledTimes(1);
  });

  it('Add is disabled and inert while editing is blocked, but Open as project still works', () => {
    blocked = true;
    render(<AddOrOpenProjectDialog />);
    const add = screen.getByTestId('add-or-open-project-add') as HTMLButtonElement;
    expect(add.disabled).toBe(true);
    expect(screen.getByTestId('add-or-open-project-blocked')).toBeTruthy();
    fireEvent.click(add);
    expect(runAddProjectFile).not.toHaveBeenCalled();
    fireEvent.click(screen.getByTestId('add-or-open-project-open'));
    expect(routeOpenedFile).toHaveBeenCalledTimes(1);
  });

  it('Add stays inert if editing becomes blocked after render (click-time re-check)', () => {
    render(<AddOrOpenProjectDialog />);
    blocked = true;
    fireEvent.click(screen.getByTestId('add-or-open-project-add'));
    expect(runAddProjectFile).not.toHaveBeenCalled();
    expect(closeDialog).not.toHaveBeenCalled();
  });

  it('Cancel abandons the add and drops the pending source without routing', () => {
    render(<AddOrOpenProjectDialog />);
    fireEvent.click(screen.getByTestId('add-or-open-project-cancel'));
    expect(clearProjectActionSource).toHaveBeenCalled();
    expect(closeDialog).toHaveBeenCalled();
    expect(routeOpenedFile).not.toHaveBeenCalled();
    expect(runAddProjectFile).not.toHaveBeenCalled();
  });
});
