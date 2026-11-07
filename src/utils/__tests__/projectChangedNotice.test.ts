/**
 * [#576 Q-W44-7-D1] The helper every user-started verb reads a refusal because the
 * project changed through: the short line for the backend's PROJECT_CHANGED and for
 * the store's local twin of it, nothing for any other failure, and a warning toast
 * under its own key for a verb whose errors are toasts.
 */
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { projectChangedMessage, toastProjectChanged } from '../projectChangedNotice';
import { IpcCommandError } from '../commandError';
import { PROJECT_CHANGED_REFUSAL, refusedLocally } from '../../store/commandResult';
import { useUIStore } from '../../store/uiStore';
import { TOAST_KEYS } from '../toastKeys';

const SHORT = 'The project changed before this finished.';
const changed = new IpcCommandError({
  code: 'PROJECT_CHANGED',
  message: 'Release lock was not applied: another project is open now.',
});

let showToast: ReturnType<typeof vi.fn>;

beforeEach(() => {
  showToast = vi.fn();
  useUIStore.setState({ showToast });
});

describe('projectChangedMessage (#576 Q-W44-7-D1)', () => {
  it('is the short line for the backend PROJECT_CHANGED, thrown or plain', () => {
    expect(projectChangedMessage(changed)).toBe(SHORT);
    expect(projectChangedMessage({ code: 'PROJECT_CHANGED', message: 'x' })).toBe(SHORT);
  });

  it("is the short line for the store's local twin", () => {
    expect(projectChangedMessage(refusedLocally(PROJECT_CHANGED_REFUSAL))).toBe(SHORT);
  });

  it('is null for any other failure', () => {
    expect(projectChangedMessage(new IpcCommandError({ code: 'SPELEODB_RELOGIN', message: 'x' }))).toBeNull();
    expect(projectChangedMessage(new Error('boom'))).toBeNull();
    expect(projectChangedMessage('plain string')).toBeNull();
  });
});

describe('toastProjectChanged (#576 Q-W44-7-D1)', () => {
  it('toasts the short line as a warning under its own key, and says it did', () => {
    expect(toastProjectChanged(changed)).toBe(true);
    expect(showToast).toHaveBeenCalledTimes(1);
    expect(showToast).toHaveBeenCalledWith(SHORT, TOAST_KEYS.PROJECT_CHANGED, 'warning');
  });

  it('shows nothing for any other failure, and says it did not', () => {
    expect(toastProjectChanged(new Error('boom'))).toBe(false);
    expect(showToast).not.toHaveBeenCalled();
  });
});
