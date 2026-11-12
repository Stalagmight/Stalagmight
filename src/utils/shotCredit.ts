/**
 * Per-shot survey credit (#707): who surveyed a shot and when, where that
 * differs from its section. A shot's `credit` field is absent when it inherits
 * both values; inside it, an absent field inherits that one value and `null`
 * means explicitly unknown.
 */
import type { Shot } from '../types/ipc/Shot';

export interface Credit {
  surveyor: string | null;
  date: string | null;
}

/** The surveyor and date to credit for `shot`, given its section's. */
export function effectiveCredit(shot: Pick<Shot, 'credit'>, section: Credit): Credit {
  const own = shot.credit;
  return {
    surveyor: own?.surveyor !== undefined ? own.surveyor : section.surveyor,
    date: own?.date !== undefined ? own.date : section.date,
  };
}

/**
 * The shot's credit when it differs from its section's, or `null` when the
 * section's credit already describes it - so callers show it only where it
 * tells the reader something new.
 */
export function creditOverride(shot: Pick<Shot, 'credit'>, section: Credit): Credit | null {
  const credit = effectiveCredit(shot, section);
  return credit.surveyor === section.surveyor && credit.date === section.date ? null : credit;
}
