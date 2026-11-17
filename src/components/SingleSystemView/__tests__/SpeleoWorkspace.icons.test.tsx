/**
 * Every action button in the SpeleoDB tab carries an icon: the everyday actions,
 * the header's recovery actions, the History disclosure, the version steps and
 * the revision rows.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';

const ipc = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: ipc, getErrorMessage: String }));

import { SpeleoWorkspace } from '../SpeleoWorkspace';
import { useProjectStore } from '../../../store/projectStore';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useUIStore } from '../../../store/uiStore';
import { MENU_ICON_COLORS } from '../../common/menuColors';

const LINK: SpeleoLinkState = {
  permission: 'READ_AND_WRITE', editState: 'read-only', lockHolder: null,
  lockedByOther: false, hasUnpushedEdits: false, unwound: false,
  undoMode: 'full', remoteState: 'conflict', noAccess: true, serverError: null,
};
const revision = (id: string) => ({ id, parent_ids: [], author_email: 'synthetic@example.com', authored_date: null, message: `Synthetic ${id}` });

/** A button "has an icon" when it holds an svg glyph or the (masked) SpeleoDB mark. */
const iconOf = (button: HTMLElement) => button.querySelector('svg, [style*="mask-image"]');

beforeEach(() => {
  ipc.mockReset();
  ipc.mockImplementation(async (cmd: string) => cmd === 'speleodb_get_revisions'
    ? [revision('aaaaaaaa1111'), revision('bbbbbbbb2222')] : null);
  useProjectStore.setState({ project: { name: 'Synthetic cave', file_path: '/synthetic.stalx', zero_datums: [] } as never, canUndo: false, canRedo: false });
  useUIStore.setState({ appMode: 'survey', speleoWorkspaceSection: 'history' });
  useSpeleoStore.setState({ link: LINK, remoteProjectId: 'synthetic', busy: false,
    session: { loggedIn: true, userEmail: 'me@example.com', endpoint: 'https://synthetic.example' } });
});

describe('SpeleoDB tab button icons', () => {
  it('a linked project: every button in the workspace has an icon', async () => {
    render(<SpeleoWorkspace />);
    const workspace = screen.getByTestId('speleodb-workspace');
    await waitFor(() => expect(screen.getByTestId('speleodb-revision-view-bbbbbbbb2222')).toBeInTheDocument());
    const buttons = within(workspace).getAllByRole('button');
    // Header recovery x2, everyday x5, Revision History disclosure, web link, View + Revert.
    expect(buttons.length).toBeGreaterThanOrEqual(11);
    for (const button of buttons) {
      expect(iconOf(button), button.textContent ?? button.outerHTML).not.toBeNull();
    }
  });

  it.each([
    ['editing', { editState: 'editing' as const, noAccess: false, remoteState: 'up_to_date' as const }],
    ['signed out', { noAccess: false, remoteState: 'up_to_date' as const }],
  ])('the editing toggle keeps an icon when %s', (_label, link) => {
    useSpeleoStore.setState({ link: { ...LINK, ...link } });
    if (_label === 'signed out') useSpeleoStore.setState({ session: null });
    render(<SpeleoWorkspace />);
    expect(iconOf(screen.getByTestId('speleodb-workspace-editing'))).not.toBeNull();
  });

  it('a local project: the conversion button has the SpeleoDB mark', () => {
    useSpeleoStore.setState({ link: null });
    render(<SpeleoWorkspace />);
    expect(iconOf(screen.getByTestId('speleodb-workspace-convert'))).not.toBeNull();
  });
});

describe('SpeleoDB tab icon colours follow the menus', () => {
  const tint = (testId: string) =>
    screen.getByTestId(testId).querySelector('[data-icon-color]')?.getAttribute('data-icon-color') ?? null;
  const clean: Partial<SpeleoLinkState> = { noAccess: false, remoteState: 'up_to_date' };

  it('read-only and able to resume: each action wears its menu colour, the primary keeps its own', () => {
    // Resume editing is the primary here, so its icon stays the button's text colour.
    useSpeleoStore.setState({ link: { ...LINK, ...clean } });
    render(<SpeleoWorkspace />);
    expect(tint('speleodb-workspace-upload')).toBe(MENU_ICON_COLORS.save);
    expect(tint('speleodb-workspace-editing')).toBeNull();
    expect(tint('speleodb-workspace-latest')).toBe(MENU_ICON_COLORS.open);
    expect(tint('speleodb-workspace-close')).toBe(MENU_ICON_COLORS.close);
    expect(tint('speleodb-workspace-open-web')).toBe(MENU_ICON_COLORS.projectDetails);
  });

  it('with changes to save: Resume editing wears Import, Save (now primary) keeps its own', () => {
    useSpeleoStore.setState({ link: { ...LINK, ...clean, hasUnpushedEdits: true } });
    render(<SpeleoWorkspace />);
    expect(tint('speleodb-workspace-upload')).toBeNull();
    expect(tint('speleodb-workspace-editing')).toBe(MENU_ICON_COLORS.import);
  });

  it('editing with nothing to save: Switch to Read Only Mode wears the neutral grey', () => {
    useSpeleoStore.setState({ link: { ...LINK, ...clean, editState: 'editing' } });
    render(<SpeleoWorkspace />);
    expect(tint('speleodb-workspace-upload')).toBe(MENU_ICON_COLORS.save);
    expect(tint('speleodb-workspace-editing')).toBe(MENU_ICON_COLORS.close);
  });

  it('the Revision History website button wears Project Details, like Open on SpeleoDB', async () => {
    useSpeleoStore.setState({ link: { ...LINK, ...clean } });
    render(<SpeleoWorkspace />);
    await waitFor(() => expect(screen.getByTestId('speleodb-web-revisions')).toBeInTheDocument());
    expect(tint('speleodb-web-revisions')).toBe(MENU_ICON_COLORS.projectDetails);
  });
});
