/** SpeleoDB account and connection settings. */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';

const invokeIpcMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeIpcMock }));

// Real copy, so the four states are asserted as the user reads them.
import '../../../i18n';
import { SpeleoDbSettings } from './SpeleoDbSettings';
import { defaultConfig } from '../../../types/config';
import type { Config } from '../../../types/config';
import { useSpeleoStore } from '../../../store/speleoStore';

let onChange: ReturnType<typeof vi.fn>;

function renderPanel(overrides: Partial<Config['speleodb']> = {}) {
  const config: Config = {
    ...defaultConfig,
    speleodb: { ...defaultConfig.speleodb, ...overrides },
  };
  render(
    <SpeleoDbSettings
      config={config}
      onChange={onChange}
      changedFields={new Set()}
      onValidationError={vi.fn()}
      panelIdBase="settings"
    />,
  );
  return config;
}

beforeEach(() => {
  invokeIpcMock.mockReset();
  invokeIpcMock.mockResolvedValue({ logged_in: false });
  onChange = vi.fn();
  // A signed-in-less session so the account panel stays inert.
  useSpeleoStore.setState({ session: null, refreshSession: vi.fn(async () => {}) });
});

describe('SpeleoDbSettings allow-force-edit toggle (#213)', () => {
  it('stages allow_force_edit=true through onChange when toggled on', async () => {
    renderPanel({ allow_force_edit: false });
    await userEvent.click(screen.getByTestId('toggle-speleodb-allow-force-edit'));
    expect(onChange).toHaveBeenCalledWith({
      speleodb: expect.objectContaining({ allow_force_edit: true }),
    });
  });

  it('shows the warning only while enabled', () => {
    renderPanel({ allow_force_edit: true });
    expect(screen.getByTestId('speleodb-allow-force-edit-warning')).toBeInTheDocument();
  });

  it('hides the warning while disabled', () => {
    renderPanel({ allow_force_edit: false });
    expect(screen.queryByTestId('speleodb-allow-force-edit-warning')).not.toBeInTheDocument();
  });
});


it('explains the single plaintext configuration file without storage controls', () => {
  renderPanel({});
  expect(screen.getByTestId('speleodb-credential-storage')).toHaveTextContent('speleodb/speleodb.conf');
  expect(screen.getByTestId('speleodb-credential-storage')).toHaveTextContent('plaintext');
  expect(screen.queryByTestId('toggle-speleodb-disable-keychain')).not.toBeInTheDocument();
  expect(invokeIpcMock).not.toHaveBeenCalled();
});
