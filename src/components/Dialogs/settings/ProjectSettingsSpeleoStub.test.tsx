/**
 * Component test - the SpeleoDB tab of the local Project Settings dialog is now a
 * pointer at the SpeleoDB project dialog, not a panel of its own.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

import { ProjectSettingsDialog } from '../ProjectSettingsDialog';
import { useSpeleoStore, type SpeleoLinkState } from '../../../store/speleoStore';
import { useProjectStore } from '../../../store/projectStore';
import { useUIStore } from '../../../store/uiStore';
import { makeSpeleoLink } from '../../../test/factories/speleo';

const LINK: SpeleoLinkState = makeSpeleoLink({ permission: 'ADMIN' });

let openDialog: ReturnType<typeof vi.fn>;

beforeEach(() => {
  openDialog = vi.fn();
  useSpeleoStore.setState({ link: LINK });
  useProjectStore.setState({ project: { name: 'Cave A' } as never });
  useUIStore.setState({
    activeDialog: 'project-settings',
    dialogData: {},
    openDialog,
    closeDialog: vi.fn(),
    showToast: vi.fn(),
  });
});

describe('ProjectSettingsSpeleoStub', () => {
  it('replaces the old SpeleoDB panel and opens the SpeleoDB dialog', async () => {
    render(<ProjectSettingsDialog />);
    await userEvent.click(screen.getByTestId('project-settings-tab-speleodb'));

    expect(screen.getByTestId('project-settings-speleodb-stub')).toBeInTheDocument();
    // The server-side administration moved out wholesale.
    expect(screen.queryByTestId('speleodb-settings-tab')).toBeNull();
    expect(screen.queryByTestId('speleodb-settings-loading')).toBeNull();

    await userEvent.click(screen.getByTestId('project-settings-open-speleodb'));
    expect(openDialog).toHaveBeenCalledWith('speleodb-project');
  });

  it('is not offered for a local project', () => {
    useSpeleoStore.setState({ link: null });
    render(<ProjectSettingsDialog />);
    expect(screen.queryByTestId('project-settings-tab-speleodb')).toBeNull();
  });
});
