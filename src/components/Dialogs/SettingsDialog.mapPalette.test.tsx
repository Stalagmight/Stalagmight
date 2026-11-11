import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import '../../i18n';
import SettingsDialog from './SettingsDialog';
import { defaultConfig, type Config } from '../../types/config';
import { emptyCommandResult, useProjectStore } from '../../store/projectStore';
import { useUIStore } from '../../store/uiStore';

/**
 * Settings saves the whole config, but the map palette is not a Settings field:
 * the legend cog and the export panel write it through `update_map_palette`. A
 * Settings save must send the palette as it is NOW, not as it was when the
 * dialog opened, or a pick made in between is silently reverted on disk.
 */
const initialProject = useProjectStore.getState();
const initialUi = useUIStore.getState();

beforeEach(() => {
  useUIStore.setState({ activeDialog: 'settings', dialogData: { initialCategory: 'general' }, closeDialog: vi.fn() });
});
afterEach(() => {
  useProjectStore.setState(initialProject, true);
  useUIStore.setState(initialUi, true);
});

it('saves the live map palette, not the one Settings opened with', async () => {
  const updateConfig = vi.fn(async (_config: Config) => emptyCommandResult(true));
  useProjectStore.setState({ config: structuredClone(defaultConfig), updateConfig });
  render(<SettingsDialog />);
  fireEvent.click(screen.getByTestId('toggle-auto-save'));

  const live = useProjectStore.getState().config!;
  act(() => {
    useProjectStore.setState({
      config: {
        ...live,
        map_palette: { ...live.map_palette, depth: { id: 'mpl.viridis', reversed: true }, pinned: ['grad.cenote'] },
      },
    });
  });

  fireEvent.click(screen.getByTestId('settings-save-button'));
  await act(async () => {});
  expect(updateConfig).toHaveBeenCalledOnce();
  const saved = updateConfig.mock.calls[0]![0];
  expect(saved.general.auto_save).toBe(!defaultConfig.general.auto_save);
  expect(saved.map_palette.depth).toEqual({ id: 'mpl.viridis', reversed: true });
  expect(saved.map_palette.pinned).toEqual(['grad.cenote']);
});
