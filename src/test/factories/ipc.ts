/**
 * The vitest IPC fake (#576 E15): the test-facing side of the manual mock in
 * `src/utils/__mocks__/ipc.ts`. A test that fakes the backend writes
 *
 *     import { invokeIpcMock } from '<rel>/test/factories/ipc';
 *
 *     vi.mock('<rel>/utils/ipc');
 *
 * and scripts `invokeIpcMock` like any `vi.fn()`. The `vi.mock` call has no
 * factory - vitest hoists it and loads the `__mocks__` file - so this module's
 * own import of `utils/ipc` already gets the mock; it refuses to load in a file
 * that forgot the `vi.mock`, where it would hand out the real IPC.
 *
 * Not re-exported from `./index`: importing it is the statement that the file
 * mocks `utils/ipc`.
 */
import { vi, type Mock } from 'vitest';
import * as ipc from '../../utils/ipc';
import type * as IpcMockModule from '../../utils/__mocks__/ipc';

const mocked = ipc as unknown as typeof IpcMockModule;

if (!vi.isMockFunction(mocked.invokeIpc)) {
  throw new Error(
    "src/test/factories/ipc: this test file does not mock utils/ipc - add vi.mock('<rel>/utils/ipc') (no factory)",
  );
}

/**
 * The file's mocked `invokeIpc`. Typed like a bare `vi.fn()` so a test can
 * script any command's answer without restating the command manifest.
 */
export const invokeIpcMock: Mock = mocked.invokeIpc;

/** What a `getErrorMessage` stub may answer. */
export type ErrorMessageReader = IpcMockModule.ErrorMessageReader;

/**
 * Swap the mocked `getErrorMessage` for the rest of this file (it is the real
 * reader until then). Call it at the top level, next to the `vi.mock`.
 */
export function stubGetErrorMessage(reader: ErrorMessageReader): void {
  mocked.setErrorMessageReader(reader);
}

/**
 * Answer a `get_project_state_binary` that the test scripted with anything but
 * bytes (an unscripted call answers `undefined`) with `new ArrayBuffer(0)` - "no
 * project open" - instead of handing the decoder a value it throws on (#576
 * NF-W6A-5). The call is still recorded on `invokeIpcMock` as made.
 */
export function emptyBinaryRefresh(): void {
  mocked.setEmptyBinaryRefresh(true);
}

/** `(e, fallback) => fallback ?? text`: the caller's fallback wins over the error's own message. */
export const fallbackOr =
  (text?: string): ErrorMessageReader =>
  (_e, fallback) =>
    fallback ?? text;

/** `(e, fallback) => e?.message ?? fallback ?? text`: an error's message, else the fallback. */
export const messageOr =
  (text: string): ErrorMessageReader =>
  (e, fallback) =>
    (e as { message?: string } | null | undefined)?.message ?? fallback ?? text;

/** `e instanceof Error ? e.message : String(e)`: ignores the fallback. */
export const errorMessageOrString: ErrorMessageReader = (e) => (e instanceof Error ? e.message : String(e));

/** `String(e)`: ignores the fallback. */
export const stringOfError: ErrorMessageReader = (e) => String(e);
