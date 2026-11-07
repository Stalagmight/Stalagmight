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
  it('stages a toggle of update_check_enabled without mutating the passed-in config', async () => {
    const config = renderPanel({ update_check_enabled: true });
    await userEvent.click(screen.getByTestId('toggle-update-check'));
    expect(onChange).toHaveBeenCalledWith({
      general: expect.objectContaining({ update_check_enabled: false }),
    });
    expect(config.general.update_check_enabled).toBe(true);
  });

  it('stages an update_track select change', async () => {
    renderPanel({ update_track: 'stable' });
    await pickOption('select-update-track', 'weekly');
    expect(onChange).toHaveBeenCalledWith({
      general: expect.objectContaining({ update_track: 'weekly' }),
    });
  });

  it('marks the row changed and offers reset when update_check_enabled differs from default', () => {
    renderPanel({ update_check_enabled: !defaultConfig.general.update_check_enabled });
    const row = screen.getByTestId('setting-update-check');
    expect(row.querySelector('[data-testid$="-reset"]')).toBeTruthy();
  });
});
