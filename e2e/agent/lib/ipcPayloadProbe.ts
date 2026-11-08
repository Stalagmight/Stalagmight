/**
 * #576 RV20-13: how `ipc-payload-probe.ts` reads one call's rejection.
 *
 * The probe's premise is that the error says where the body stopped: a body
 * that arrived whole and was refused for its content fails with serde's
 * `missing field ...` (or another data error), a body that was cut short with
 * `premature end of input`. It used to read every error that was not the
 * second as "arrived", so a page script's own failure (`invoke unavailable`)
 * or a command Tauri did not know counted as an arrived body. Each error is
 * now one of a fixed set of classes, or neither, and what prints is that
 * class and the message's length, never its text (#576 W51).
 */
import { withheldAppText } from './shareableFailure';

/** The rejections the probe can read, each with what it says about the body. */
export const PROBE_ERROR_CLASSES: readonly { readonly label: string; readonly arrived: boolean }[] = [
  // Cut short: what the probe is looking for (chrono's and serde_json's words for it).
  { label: 'premature end of input', arrived: false },
  { label: 'EOF while parsing', arrived: false },
  // Parsed, then refused for its content (serde's data errors, chrono's value errors).
  { label: 'missing field', arrived: true },
  { label: 'invalid type', arrived: true },
  { label: 'invalid value', arrived: true },
  { label: 'invalid length', arrived: true },
  { label: 'unknown field', arrived: true },
  { label: 'unknown variant', arrived: true },
  { label: 'duplicate field', arrived: true },
  { label: 'input contains invalid characters', arrived: true },
  { label: 'input is out of range', arrived: true },
  { label: 'trailing input', arrived: true },
];

/** `other`: an error that is none of the classes, which says nothing about the body. */
export type ProbeVerdict = 'arrived' | 'truncated' | 'other';

function probeErrorClass(error: string): (typeof PROBE_ERROR_CLASSES)[number] | undefined {
  return PROBE_ERROR_CLASSES.find((entry) => error.includes(entry.label));
}

/** A call that succeeded arrived; a known rejection says which; any other error is `other`. */
export function probeVerdict(error: string | null): ProbeVerdict {
  if (error === null) return 'arrived';
  const found = probeErrorClass(error);
  if (found === undefined) return 'other';
  return found.arrived ? 'arrived' : 'truncated';
}

/** The error as the probe prints it: its class (or `not a known rejection`) and its length. */
export function shownProbeError(error: string | null): string {
  if (error === null) return '(no error)';
  return `${probeErrorClass(error)?.label ?? 'not a known rejection'} ${withheldAppText(error)}`;
}
