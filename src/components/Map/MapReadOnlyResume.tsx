import { useTranslation } from 'react-i18next';
import { useSpeleoActions } from '../../hooks/useSpeleoActions';
import { useSpeleoStore } from '../../store/speleoStore';
import { useUIStore } from '../../store/uiStore';
import { canResumeSpeleoEditing } from '../../utils/speleoPermission';
import { speleoReadOnlyReason, speleoReadOnlyReasonKey } from '../../utils/speleoReadOnlyReason';
import { PencilIcon, SignInIcon } from '../common/icons';

const RESUME_CLS =
  'shrink-0 whitespace-nowrap inline-flex items-center gap-1 rounded bg-accent px-2 py-0.5 font-medium text-accent-on hover:bg-accent-90 disabled:cursor-not-allowed disabled:opacity-50 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent';

/**
 * A read-only pill's words (`MapTopLeftOverlay`'s notices). They sit beside the
 * pill's action and wrap in their own column. The basis is the narrowest that
 * column may get before the action drops under it: `flex-[1_1_5rem]` with
 * `max-w-max`, so a short label (min of 5rem and its own width) never pushes the
 * action down, and a long one grows to fill the row. 5rem keeps every pill's
 * action beside its words at the minimum map width (MAP_MIN_WIDTH) with no other
 * notice in the row; the action drops only when the pill gets narrower than that.
 * `py-0.5` puts the first line on the action's and the glyph's centre line (all
 * three are 20px).
 */
export const PILL_LABEL_CLS = 'flex-[1_1_5rem] max-w-max py-0.5 text-accent font-medium';

/**
 * "Resume editing" on the SpeleoDB read-only pill: takes the write lock again
 * through the same start-editing flow as the SpeleoDB tab, with its error
 * handling (someone else editing, no permission, divergence). Rendered only when
 * it can apply - signed in, with project access and write permission - which is
 * the same rule the tab's Resume editing uses.
 */
export function SpeleoResumeEditingButton() {
  const { t } = useTranslation();
  const link = useSpeleoStore((s) => s.link);
  const session = useSpeleoStore((s) => s.session);
  const { busy, startEditing } = useSpeleoActions();
  if (!canResumeSpeleoEditing(link, session)) return null;
  return (
    <button type="button" className={RESUME_CLS} disabled={busy}
      onClick={() => void startEditing()}
      title={t('map.readOnlyPill.resumeSpeleoTitle')}
      data-testid="speleodb-readonly-resume">
      <PencilIcon size={12} />
      {t('map.readOnlyPill.resume')}
    </button>
  );
}

/**
 * The SpeleoDB read-only pill's words: why the project cannot be edited when that
 * is a lasting reason (signed out: "Read-only copy - connect to SpeleoDB to edit";
 * view-only access), and the plain badge otherwise.
 */
export function SpeleoReadOnlyLabel() {
  const { t } = useTranslation();
  const link = useSpeleoStore((s) => s.link);
  const session = useSpeleoStore((s) => s.session);
  const reason = speleoReadOnlyReason(link, session);
  return (
    <span className={PILL_LABEL_CLS} data-testid="speleodb-readonly-reason" data-reason={reason ?? 'none'}>
      {reason ? t(speleoReadOnlyReasonKey(reason)) : t('speleodb.map.readOnlyBadge')}
    </span>
  );
}

/**
 * "Sign in" on the SpeleoDB read-only pill while signed out: opens the SpeleoDB
 * settings, where signing in happens. Nothing is acquired here; once signed in,
 * "Resume editing" takes over.
 */
export function SpeleoLogInButton() {
  const { t } = useTranslation();
  const session = useSpeleoStore((s) => s.session);
  if (session?.loggedIn === true) return null;
  return (
    <button type="button" className={RESUME_CLS}
      onClick={() => useUIStore.getState().openDialog('settings', { initialCategory: 'speleodb' })}
      title={t('speleodb.cachedCopies.logInTitle')}
      data-testid="speleodb-readonly-login">
      <SignInIcon size={12} />
      {t('speleodb.cachedCopies.logIn')}
    </button>
  );
}

/**
 * "Resume editing" on the Closures Computed View pill: back to the Raw Data View,
 * the same geometry-mode write the Display menu's toggle makes. The pill itself
 * is shown only where that applies (Survey, no other read-only cause).
 */
export function RawDataResumeButton() {
  const { t } = useTranslation();
  return (
    <button type="button" className={RESUME_CLS}
      onClick={() => useUIStore.getState().setGeometryMode('measured', 'map')}
      title={t('map.readOnlyPill.resumeRawTitle')}
      data-testid="geometry-readonly-resume">
      <PencilIcon size={12} />
      {t('map.readOnlyPill.resume')}
    </button>
  );
}
