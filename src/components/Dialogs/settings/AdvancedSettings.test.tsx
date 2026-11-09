import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { AdvancedSettings } from './AdvancedSettings';
import { defaultConfig } from '../../../types/config';
import { useSpeleoStore } from '../../../store/speleoStore';

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock('../../../utils/ipc', () => ({ invokeIpc: invokeMock }));

beforeEach(() => {
  invokeMock.mockReset();
  useSpeleoStore.setState({
    session: { loggedIn: true, userEmail: 'old@example.com', endpoint: 'https://old.example.com' },
    sessionGeneration: 5,
    link: { editState: 'editing', forceEditing: false, hasUnpushedEdits: true } as never,
  });
});

describe('credential identity on settings reload', () => {
  it.each([false, true])('resets the session only when credentials changed: %s', async (changed) => {
    const onChange = vi.fn();
    invokeMock.mockImplementation(async (command: string) => {
      if (command === 'reload_config') return [defaultConfig, changed];
      if (command === 'speleodb_session') return { logged_in: true, user_email: 'new@example.com', endpoint: 'https://new.example.com' };
      return null;
    });
    render(<AdvancedSettings config={defaultConfig} onChange={onChange} changedFields={new Set()} onValidationError={vi.fn()} panelIdBase="settings" />);
    await userEvent.click(screen.getByTestId('reload-config-button'));
    await waitFor(() => expect(onChange).toHaveBeenCalledWith(defaultConfig));
    const state = useSpeleoStore.getState();
    expect(state.link?.editState).toBe(changed ? 'read-only' : 'editing');
    expect(state.session?.userEmail).toBe(changed ? 'new@example.com' : 'old@example.com');
    expect(state.link?.hasUnpushedEdits).toBe(true);
    expect(state.sessionGeneration > 5).toBe(changed);
  });
});
