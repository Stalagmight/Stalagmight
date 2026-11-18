import type { ZeroDatumConfigDraft } from '../components/Dialogs/ZeroDatumConfigDialog';

/**
 * The `dialogData` key App.tsx relays the zero-datum config carry through
 * (#394 F10, extended by #534). Centralized so a rename is a type error at
 * every call site instead of a silent no-carry from a stale string literal.
 */
export const ZERO_DATUM_CARRY_KEY = 'previousZeroDatumConfig' as const;

/** Typed read of the carry out of `dialogData` - the counterpart to writing
 *  `{ [ZERO_DATUM_CARRY_KEY]: ... }` into it. */
export function readCarriedZeroDatumConfig(
  dialogData: Record<string, unknown>
): ZeroDatumConfigDraft | undefined {
  return dialogData[ZERO_DATUM_CARRY_KEY] as ZeroDatumConfigDraft | undefined;
}

/**
 * Decide whether a config carried out of `ZeroDatumConfigDialog`'s own Back
 * (#534) survives the forward pass from 'zero-datum-selection': only when the
 * re-picked line matches the one the carry was entered for. A different
 * pick means the name/GPS/shots answered a different question, so it starts
 * fresh.
 *
 * Exported so App.tsx's routing and its tests share this exact decision -
 * re-implementing it in a test would let deleting the production code leave
 * the test green.
 */
export function carryZeroDatumConfig(
  carried: ZeroDatumConfigDraft | undefined,
  lineIndex: number
): ZeroDatumConfigDraft | undefined {
  return carried?.lineIndex === lineIndex ? carried : undefined;
}
