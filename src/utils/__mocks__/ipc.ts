/**
 * Manual vitest mock of `src/utils/ipc` (#576 E15). A test file opts in with a
 * factory-less `vi.mock('<rel>/utils/ipc')` - vitest then loads this file in
 * place of the real module - and drives it through `src/test/factories/ipc.ts`
 * (`invokeIpcMock`, `stubGetErrorMessage`, `emptyBinaryRefresh`). Each test file
 * gets its own instance, so nothing set here leaks between files.
 *
 * - `invokeIpc` is a bare `vi.fn()`: it answers `undefined` until the test
 *   scripts it, and records every call.
 * - `getErrorMessage` is the real reader (from `../commandError`, which no test
 *   mocks) unless the file swapped it. It is a plain function, not a `vi.fn`, so
 *   `vi.resetAllMocks()` / `vi.restoreAllMocks()` leave a swapped reader in place,
 *   as they left the inline stubs this module replaces.
 */
import { vi, type Mock } from 'vitest';
import { getErrorMessage as realGetErrorMessage } from '../commandError';

/** What a file's `getErrorMessage` stub may answer (the inline stubs it replaces
 *  could answer `undefined` for a call without a fallback). */
export type ErrorMessageReader = (e: unknown, fallback?: string) => string | undefined;

const calls: Mock = vi.fn();
let readErrorMessage: ErrorMessageReader = realGetErrorMessage;
let emptyRefresh = false;

/** True for the bytes a binary IPC answer arrives as. */
const isBytes = (answer: unknown): boolean => answer instanceof ArrayBuffer || answer instanceof Uint8Array;

/**
 * The mocked `invokeIpc`: `calls` itself to every reader (`vi.isMockFunction`,
 * `.mock.calls`, `mockResolvedValue`, ...) - the Proxy only intercepts the call, so
 * that `emptyBinaryRefresh` can stand in for an unscripted refresh answer.
 */
export const invokeIpc: Mock = new Proxy(calls, {
  apply(target, thisArg, args: unknown[]) {
    const answer: unknown = Reflect.apply(target, thisArg, args);
    if (!emptyRefresh || args[0] !== 'get_project_state_binary') return answer;
    return Promise.resolve(answer).then((bytes) => (isBytes(bytes) ? bytes : new ArrayBuffer(0)));
  },
});

export function getErrorMessage(e: unknown, fallback?: string): string {
  return readErrorMessage(e, fallback) as string;
}

/** Test hook behind `stubGetErrorMessage`: swap the reader for the rest of this file. */
export function setErrorMessageReader(reader: ErrorMessageReader): void {
  readErrorMessage = reader;
}

/** Test hook behind `emptyBinaryRefresh`. */
export function setEmptyBinaryRefresh(on: boolean): void {
  emptyRefresh = on;
}
