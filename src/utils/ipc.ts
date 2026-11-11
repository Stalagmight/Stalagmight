import { invoke } from '@tauri-apps/api/core';
import { trackCommand } from './telemetry';
import { trackInvoke } from './debugLockup';
import { speleoNetworkKind } from './speleoNetworkCommands';
import { trackSpeleoNetworkCall } from '../store/speleoNetworkStore';

const IPC_TRACE = import.meta.env.VITE_IPC_TRACE === 'true';

/** Commands that should not be tracked (telemetry's own IPC calls). */
const UNTRACKED_COMMANDS = new Set([
  'track_events',
  'flush_telemetry',
  'is_telemetry_available',
  'set_telemetry_enabled',
]);

export type IpcMeta = {
  purpose?: string;
  fields?: Record<string, unknown>;
  level?: 'debug' | 'info';
};

function nowMs(): number {
  if (typeof performance !== 'undefined' && typeof performance.now === 'function') {
    return performance.now();
  }
  return Date.now();
}

function errorToString(err: unknown): string {
  if (err instanceof Error) return `${err.name}: ${err.message}`;
  return String(err);
}

export type InvokeIpc = <T>(
  command: string,
  args?: Record<string, unknown>,
  meta?: IpcMeta
) => Promise<T>;

/** Send one IPC, counted as in flight while it runs when it reaches SpeleoDB
    (the busy indicator and the SpeleoDB UI gating read that count). */
function sendInvoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  return trackSpeleoNetworkCall(speleoNetworkKind(command, args), () =>
    trackInvoke(command, invoke<T>(command, args)),
  );
}

const invokeWithTrace: InvokeIpc = async <T>(command: string, args?: Record<string, unknown>, meta?: IpcMeta) => {
  if (!UNTRACKED_COMMANDS.has(command)) trackCommand(command);
  const start = nowMs();
  const purpose = meta?.purpose ?? 'unspecified purpose';
  const fields = meta?.fields ?? {};
  const logStart = meta?.level === 'info' ? console.info : console.debug;

  logStart(`[IPC] invoking ${command} for ${purpose}`, { command, purpose, ...fields });

  try {
    const result = await sendInvoke<T>(command, args);
    const durationMs = Math.round(nowMs() - start);
    console.debug(`[IPC] success ${command}`, { command, purpose, duration_ms: durationMs, ...fields });
    return result;
  } catch (err) {
    const durationMs = Math.round(nowMs() - start);
    console.error(`[IPC] failed ${command} for ${purpose}`, {
      command,
      purpose,
      duration_ms: durationMs,
      reason: errorToString(err),
      ...fields,
    });
    throw err;
  }
};

const invokeNoTrace: InvokeIpc = <T>(command: string, args?: Record<string, unknown>, _meta?: IpcMeta) => {
  if (!UNTRACKED_COMMANDS.has(command)) trackCommand(command);
  return sendInvoke<T>(command, args);
};

/**
 * Invoke a Tauri IPC command with optional trace logging.
 * Logs are compiled out in production unless VITE_IPC_TRACE is enabled.
 */
export const invokeIpc: InvokeIpc = IPC_TRACE ? invokeWithTrace : invokeNoTrace;

/** Extract error message from unknown catch value */
export function getErrorMessage(e: unknown, fallback?: string): string {
  if (e instanceof Error) return e.message;
  if (typeof e === 'string') return e;
  // [Track A] #186/D-B2: structured IPC errors (e.g. SpeleoErrorDto) reject with
  // an object carrying a `message` string - surface it instead of "[object
  // Object]". The typed `action` field is branched on separately (#204).
  if (e && typeof e === 'object' && 'message' in e && typeof e.message === 'string') {
    return e.message;
  }
  return fallback ?? String(e);
}
