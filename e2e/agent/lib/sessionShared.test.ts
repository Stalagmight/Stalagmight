import { describe, expect, it, vi } from 'vitest';

const remoteMock = vi.hoisted(() => vi.fn());
vi.mock('webdriverio', () => ({ remote: remoteMock }));
vi.mock('./waiters', () => ({
  waitForAppReady: vi.fn(),
  waitForE2EHooks: vi.fn(),
  disableOnboardingGuides: vi.fn(),
}));

import { attachBrowser } from './sessionShared';
import { shareableFailureText } from './shareableFailure';

describe('attachBrowser', () => {
  it("ends its headline in WebDriver's error, so a runner prints only its length (#576 W51)", async () => {
    remoteMock.mockRejectedValueOnce(new Error('session not created: /home/qzUser/qzCaveA/stalagmight exited'));
    const err = await attachBrowser(4544, {}).then(
      () => new Error('attached'),
      (e: unknown) => e as Error,
    );
    expect(err.message).toBe(
      'Failed to attach to tauri-driver on port 4544; the session request failed: ' +
        'session not created: /home/qzUser/qzCaveA/stalagmight exited',
    );
    const printed = shareableFailureText(err.message);
    expect(printed).toMatch(/^Failed to attach to tauri-driver on port 4544; the session request failed: \(\d+ characters/);
    expect(printed).not.toContain('qzCaveA');
  });
});
