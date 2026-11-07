/**
 * [#576 NF-W6A-5] A guard for the refresh every edit, import and project switch
 * ends with (`projectStore.refreshProjectState`).
 *
 * Since #576 A9-D2 the refresh reads `get_project_state_binary`, and an IPC mock
 * that answers it with nothing (`undefined`, `null`, a JSON object) makes the
 * decoder throw: the store logs "Failed to refresh project state", skips the undo
 * and warning re-reads that follow it, and the test still passes - over a path
 * the app never takes. A mock answers it with `new ArrayBuffer(0)` (no project
 * open) or a real envelope (`src/test/projectStateEnvelope.ts`); a file that calls
 * {@link failOnRefreshErrors} then fails if its mock stops doing so.
 */
import { beforeEach, expect, vi, type MockInstance } from 'vitest';

/**
 * Fail every test in the calling file whose run logged a failed refresh. Call it
 * at the top level of the file (it installs a `beforeEach`).
 *
 * [#576 RV6b-F2] The check runs when the test has FINISHED (`onTestFinished`),
 * after every afterEach - the file's own and src/test/setup.ts's unmount - and
 * after one more macrotask. It used to be an afterEach of its own, and vitest runs
 * afterEach hooks in reverse registration order, so one registered earlier in the
 * file (DmpNewImportDialog.runFailures' final `act` flush) ran AFTER the check: a
 * refresh that failed in that flush was printed and the test passed.
 */
export function failOnRefreshErrors(): void {
  beforeEach(({ onTestFinished }) => {
    const errors: MockInstance<typeof console.error> = vi.spyOn(console, 'error');
    onTestFinished(async () => {
      await new Promise((resolve) => setTimeout(resolve, 0));
      const failed = errors.mock.calls.filter(
        ([message]) => typeof message === 'string' && message.startsWith('Failed to refresh project state'),
      );
      errors.mockRestore();
      expect(
        failed.map(([, error]) => String(error)),
        "refreshProjectState failed: answer get_project_state_binary in this file's IPC mock",
      ).toEqual([]);
    });
  });
}
