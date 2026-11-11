import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen } from '@testing-library/react';
import '../../../../i18n';
import { SettingRow } from '../SettingRow';

/** The one reset control every settings row uses: an icon button, never a text button. */
describe('SettingRow reset', () => {
  it('is the standard reset icon, with its tooltip and no text', () => {
    const onReset = vi.fn();
    render(
      <SettingRow label="Synthetic setting" onReset={onReset} isDefault={false} data-testid="setting-synthetic">
        <input aria-label="Synthetic value" />
      </SettingRow>,
    );
    const reset = screen.getByTestId('setting-synthetic-reset');
    expect(reset.tagName).toBe('BUTTON');
    expect(reset).toHaveAttribute('title', 'Reset to default');
    expect(reset).toHaveTextContent(/^$/);
    expect(reset.querySelector('svg')).not.toBeNull();
    expect(reset).toBeEnabled();
    fireEvent.click(reset);
    expect(onReset).toHaveBeenCalledOnce();
  });

  it('is disabled at the default value', () => {
    render(
      <SettingRow label="Synthetic setting" onReset={vi.fn()} isDefault data-testid="setting-synthetic">
        <input aria-label="Synthetic value" />
      </SettingRow>,
    );
    expect(screen.getByTestId('setting-synthetic-reset')).toBeDisabled();
  });

  it('has no labelled-button variant', () => {
    render(
      // The text-button path (`resetLabel`) is gone: the type check fails if it returns.
      // @ts-expect-error resetLabel is not a SettingRow prop
      <SettingRow label="Synthetic setting" onReset={vi.fn()} isDefault={false} data-testid="setting-synthetic" resetLabel="Reset">
        <input aria-label="Synthetic value" />
      </SettingRow>,
    );
    const reset = screen.getByTestId('setting-synthetic-reset');
    expect(reset).toHaveTextContent(/^$/);
    expect(reset.querySelector('svg')).not.toBeNull();
  });
});
