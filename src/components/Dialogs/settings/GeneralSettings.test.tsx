/**
 * Component test (#579) - the Updates section of General settings: staged
 * change behavior for the update-check toggle and update-track select, the
 * same staged-then-Save pattern the rest of the panel uses (see types.ts's
 * field-builder helpers).
 */
import { describe, it, expect, vi, beforeEach, beforeAll } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

// jsdom does not implement scrollIntoView; the Select component calls it on open.
beforeAll(() => {
  Element.prototype.scrollIntoView = vi.fn();
});

/** Pick `value` from a portal-based `Select` (src/components/common/Select.tsx). */
async function pickOption(testId: string, value: string) {
  await userEvent.click(screen.getByTestId(testId));
  await userEvent.click(screen.getByTestId(`${testId}-option-${encodeURIComponent(value)}`));
}

// Real strings, so the Beta warning's wording is what is asserted (#650).
import '../../../i18n';
import { GeneralSettings } from './GeneralSettings';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';

let onChange: ReturnType<typeof vi.fn>;

function renderPanel(overrides: Partial<Config['general']> = {}) {
  const config: Config = {
    ...defaultConfig,
    general: { ...defaultConfig.general, ...overrides },
  };
  render(
    <GeneralSettings
      config={config}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings"
    />
  );
  return config;
}

beforeEach(() => {
  onChange = vi.fn();
});

describe('GeneralSettings updates section (#579)', () => {
  it('marks the row changed and offers reset when update_check_enabled differs from default', () => {
    renderPanel({ update_check_enabled: !defaultConfig.general.update_check_enabled });
    const row = screen.getByTestId('setting-update-check');
    expect(row.querySelector('[data-testid$="-reset"]')).toBeTruthy();
  });

  it('stages a toggle of update_check_enabled without mutating the passed-in config', async () => {
    const config = renderPanel({ update_check_enabled: true });
    await userEvent.click(screen.getByTestId('toggle-update-check'));
    expect(onChange).toHaveBeenCalledWith({
      general: expect.objectContaining({ update_check_enabled: false }),
    });
    expect(config.general.update_check_enabled).toBe(true);
  });

  it('stages a switch to Stable without asking', async () => {
    renderPanel({ update_track: 'beta' });
    await pickOption('select-update-track', 'stable');
    expect(screen.queryByTestId('update-track-beta-dialog')).not.toBeInTheDocument();
    expect(onChange).toHaveBeenCalledWith({
      general: expect.objectContaining({ update_track: 'stable' }),
    });
  });

  it('offers exactly Stable and Beta', async () => {
    renderPanel({ update_track: 'stable' });
    await userEvent.click(screen.getByTestId('select-update-track'));
    expect(screen.getByTestId('select-update-track-option-stable')).toHaveTextContent('Stable');
    expect(screen.getByTestId('select-update-track-option-beta')).toHaveTextContent('Beta');
    expect(screen.queryByTestId('select-update-track-option-weekly')).not.toBeInTheDocument();
  });
});

/**
 * [#650] Beta installs release candidates. Switching to it is a decision with
 * consequences the user has to read first, so it is confirmed before anything
 * is staged.
 */
describe('GeneralSettings switching to Beta (#650)', () => {
  it('warns before staging, and says what the switch means', async () => {
    renderPanel({ update_track: 'stable' });
    await pickOption('select-update-track', 'beta');

    const dialog = screen.getByTestId('update-track-beta-dialog');
    expect(onChange).not.toHaveBeenCalled();
    expect(dialog).toHaveTextContent(/release candidates/i);
    expect(dialog).toHaveTextContent(/may still have bugs/i);
    expect(dialog).toHaveTextContent(/may not open in an older version/i);
    expect(dialog).toHaveTextContent(/does not downgrade/i);
    expect(dialog).toHaveTextContent(/until the next stable release/i);
  });

  it('stages Beta once confirmed', async () => {
    renderPanel({ update_track: 'stable' });
    await pickOption('select-update-track', 'beta');
    await userEvent.click(screen.getByTestId('update-track-beta-confirm'));

    expect(onChange).toHaveBeenCalledTimes(1);
    expect(onChange).toHaveBeenCalledWith({
      general: expect.objectContaining({ update_track: 'beta' }),
    });
    expect(screen.queryByTestId('update-track-beta-dialog')).not.toBeInTheDocument();
  });

  it('Cancel leaves the track unchanged', async () => {
    renderPanel({ update_track: 'stable' });
    await pickOption('select-update-track', 'beta');
    await userEvent.click(screen.getByTestId('update-track-beta-cancel'));

    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByTestId('update-track-beta-dialog')).not.toBeInTheDocument();
    expect(screen.getByTestId('select-update-track')).toHaveTextContent('Stable');
  });

  it('Escape cancels as well', async () => {
    renderPanel({ update_track: 'stable' });
    await pickOption('select-update-track', 'beta');
    await userEvent.keyboard('{Escape}');

    expect(onChange).not.toHaveBeenCalled();
    expect(screen.queryByTestId('update-track-beta-dialog')).not.toBeInTheDocument();
  });

  it('does not ask again when Beta is already the track', async () => {
    renderPanel({ update_track: 'beta' });
    await pickOption('select-update-track', 'beta');
    expect(screen.queryByTestId('update-track-beta-dialog')).not.toBeInTheDocument();
  });
});
