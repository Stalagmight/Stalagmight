/**
 * `@tauri-apps/api/core` stand-ins (#576 E15-NF1). The app reaches the backend
 * through `utils/ipc` (faked by the shared mock, see `./ipc.ts`), but a few
 * modules (telemetry, the binary loader, ...) call `invoke` directly, and a test
 * that drives one of those calls, or asserts on them, mocks the Tauri module too.
 * Merely importing such a module needs no stub - the real one loads fine and only
 * fails when `invoke` is called (#576 E15T-NF1):
 *
 *     vi.mock('@tauri-apps/api/core', async () => (await import('<rel>/test/factories/tauri')).tauriCoreResolving({}));
 *
 * - `tauriCoreResolving(answer)`: `invoke` is a `vi.fn()` resolving `answer`
 *   for every command.
 * - `tauriCore(invoke)`: the test's own `invoke` (a bare `vi.fn()`, or the
 *   file's `vi.hoisted` spy that it scripts and asserts on - hoisted, it outlives
 *   `vi.resetModules()`).
 */
import { vi, type Mock } from 'vitest';

/** The module with `invoke` resolving `answer` to every command. */
export function tauriCoreResolving(answer: unknown): { invoke: Mock } {
  return { invoke: vi.fn().mockResolvedValue(answer) };
}

/** The module around the test's own `invoke`. */
export function tauriCore<F>(invoke: F): { invoke: F } {
  return { invoke };
}
