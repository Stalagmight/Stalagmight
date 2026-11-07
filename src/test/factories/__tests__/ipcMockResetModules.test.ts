/**
 * A file that calls vi.resetModules() mocks utils/ipc with a factory that imports
 * the manual mock (#576 E15): vitest keeps a factory's result across the reset,
 * so a module the test re-imports still calls the invokeIpc the test scripts. (A
 * factory-less vi.mock is re-evaluated by the reset and would hand the re-imported
 * module a fresh vi.fn.)
 */
import { expect, it, vi } from 'vitest';
import { invokeIpcMock } from '../ipc';

vi.mock('../../../utils/ipc', () => import('../../../utils/__mocks__/ipc'));

it('hands a module re-imported after vi.resetModules() the same invokeIpc', async () => {
  vi.resetModules();
  const reimported = await import('../../../utils/ipc');
  expect(reimported.invokeIpc).toBe(invokeIpcMock);
});
