/**
 * `utils/telemetry` stand-ins (#576 E15-NF1). Dialog tests silence
 * `trackDialogComplete` (and sometimes a source setter) and keep the rest:
 *
 *     vi.mock('<rel>/utils/telemetry', async () =>
 *       (await import('<rel>/test/factories/telemetry')).telemetryWith({ setSource: vi.fn() }));
 *
 * - `telemetryWith(stubs)`: the real module with `trackDialogComplete` a bare
 *   `vi.fn()` and each of `stubs` in place of its export.
 * - `telemetryStubs(stubs)`: only `trackDialogComplete` (a bare `vi.fn()`) and
 *   `stubs`; any other export is absent, so code that reaches one fails loudly.
 *
 * A stub the test asserts on is the test's own `vi.hoisted` spy, passed in; a
 * spy declared without `vi.hoisted` is passed as a forwarding arrow
 * (`(...a: unknown[]) => spy(...a)`), because the factory runs before the
 * file's own top-level statements.
 */
import { vi } from 'vitest';
import type * as Telemetry from '../../utils/telemetry';

type TelemetryModule = typeof Telemetry;

/** Replacements for some of the module's exports, by export name. */
export type TelemetryStubs = { [K in keyof TelemetryModule]?: unknown };

/** The real `utils/telemetry`, with `trackDialogComplete` and each of `stubs` replaced. */
export async function telemetryWith(stubs: TelemetryStubs = {}) {
  return {
    ...(await vi.importActual<TelemetryModule>('../../utils/telemetry')),
    trackDialogComplete: vi.fn(),
    ...stubs,
  };
}

/** `trackDialogComplete` (a bare `vi.fn()`) and `stubs`, and no other export. */
export function telemetryStubs(stubs: TelemetryStubs = {}) {
  return { trackDialogComplete: vi.fn(), ...stubs };
}
