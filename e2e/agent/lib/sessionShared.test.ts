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

  it('keeps the whole failure in actions.jsonl only with STALAGMIGHT_AGENT_PRIVATE_ERRORS=1, and prints nothing more (#576 RV20-13)', async () => {
    const reply = 'session not created: /home/qzUser/qzCaveB/stalagmight exited';
    const events: { event: string; detail?: unknown }[] = [];
    const recorder = { event: (event: string, detail?: unknown) => void events.push({ event, detail }) };
    const stdout = vi.spyOn(process.stdout, 'write');
    const stderr = vi.spyOn(process.stderr, 'write');
    const attach = (): Promise<Error> => {
      remoteMock.mockRejectedValueOnce(new Error(reply));
      return attachBrowser(4544, {}, 'tauri-driver', recorder).then(
        () => new Error('attached'),
        (e: unknown) => e as Error,
      );
    };
    try {
      vi.stubEnv('STALAGMIGHT_AGENT_PRIVATE_ERRORS', '');
      const unset = await attach();
      expect(events).toEqual([]);
      vi.stubEnv('STALAGMIGHT_AGENT_PRIVATE_ERRORS', '1');
      const set = await attach();
      expect(events).toEqual([
        {
          event: 'attach-failed',
          detail: { message: `Failed to attach to tauri-driver on port 4544; the session request failed: ${reply}` },
        },
      ]);
      // The error a runner prints is the same either way, its app text withheld; nothing went to stdout or stderr.
      expect(set.message).toBe(unset.message);
      expect(shareableFailureText(set.message)).not.toContain('qzCaveB');
      expect(stdout).not.toHaveBeenCalled();
      expect(stderr).not.toHaveBeenCalled();
    } finally {
      vi.unstubAllEnvs();
      stdout.mockRestore();
      stderr.mockRestore();
    }
  });
});
