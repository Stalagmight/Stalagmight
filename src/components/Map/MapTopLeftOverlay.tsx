import type { ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import type { AdjustmentReport } from '../../types/ipc/AdjustmentReport';
import { AlertTriangleIcon, FitToViewIcon, LockIcon } from '../common/icons';
import {
  PILL_LABEL_CLS,
  RawDataResumeButton,
  SpeleoLogInButton,
  SpeleoReadOnlyLabel,
  SpeleoResumeEditingButton,
} from './MapReadOnlyResume';

/**
 * The map's top-left overlay row: the fit-to-view / exit-focus button, and the
 * read-only notices that sit beside it.
 *
 * Extracted from `MapCanvas` because the two notices used to be independent
 * `absolute top-2 left-2` elements, which can only ever draw on top of each
 * other - a remote read-only project in the closure-adjusted view stacked the
 * SpeleoDB badge and the closure badge over the same pixels. As flex siblings
 * they lay out instead, and a future third notice cannot reintroduce the bug.
 *
 * Every gate arrives pre-computed. That is deliberate: it keeps the raw
 * geometry-mode comparison (and the `speleoReadOnly` subscription) in
 * `MapCanvas`, where `scripts/check-readonly-gates.mjs` allow-lists them by
 * exact count, so this file adds none of its own - which is also why the
 * comparison is not spelled out literally anywhere below. That lint scans
 * source text, comments included.
 */
/**
 * A read-only notice: a pill holding a glyph, the words, and (when there is one)
 * the action. The glyph sits in its own 20px slot beside the words, never on a
 * line of its own; the words and the action share the body, which wraps.
 *
 * The body keeps the action on the words' row while the words still get their
 * minimum column (`PILL_LABEL_CLS`); the words wrap inside that column, and the
 * action stays at the row's end, centred on the text block. Only when the pill is
 * narrower than that minimum plus the action does the action drop to the next
 * line, where it starts under the words (the body begins after the glyph). The
 * pill itself is `min-w-0`, so it shrinks inside the bounded row rather than
 * running under the zoom control.
 */
const NOTICE_CLS =
  'pointer-events-auto min-w-0 bg-panel border border-accent rounded px-3 py-1.5 text-xs flex items-start gap-1.5';

function ReadOnlyNotice({ testId, glyph, children }: { testId: string; glyph: ReactNode; children: ReactNode }) {
  return (
    <div className={NOTICE_CLS} data-testid={testId}>
      <span className="flex h-5 shrink-0 items-center" data-testid={`${testId}-glyph`}>{glyph}</span>
      <div className="flex min-w-0 flex-1 flex-wrap items-center gap-x-2 gap-y-1" data-testid={`${testId}-body`}>
        {children}
      </div>
    </div>
  );
}

const SHOW_WARNINGS_CLS =
  'shrink-0 whitespace-nowrap inline-flex items-center rounded border border-accent px-2 py-0.5 font-medium text-accent hover:bg-accent-10 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent';

/**
 * What the Closures Computed View left out (#737), as one notice: the loop
 * closures it skips because they span a single shot, and the parts of the
 * survey whose solve failed and are drawn measured. Both have Warnings-tab
 * entries, which the button opens. Renders nothing for an empty report.
 */
function AdjustmentNotice({ report }: { report: AdjustmentReport }) {
  const { t } = useTranslation();
  const openWarningsTab = useUIStore((s) => s.openWarningsTab);
  const skipped = report.skipped_closures.length;
  const failed = report.failures.length;
  if (skipped === 0 && failed === 0) return null;
  return (
    <ReadOnlyNotice testId="adjustment-notice" glyph={<AlertTriangleIcon size={13} className="text-warning" />}>
      <span className={`${PILL_LABEL_CLS} flex flex-col`}>
        {failed > 0 && (
          <span data-testid="adjustment-notice-failed">{t('map.geometry.failedParts', { count: failed })}</span>
        )}
        {skipped > 0 && (
          <span data-testid="adjustment-notice-skipped">{t('map.geometry.skippedClosures', { count: skipped })}</span>
        )}
      </span>
      <button type="button" className={SHOW_WARNINGS_CLS} onClick={openWarningsTab} data-testid="adjustment-notice-show-warnings">
        {t('map.geometry.showWarnings')}
      </button>
    </ReadOnlyNotice>
  );
}

export function MapTopLeftOverlay({
  focused,
  onFitOrExitFocus,
  showSpeleoReadOnly,
  showClosureReadOnly,
  showClosureComputing,
  adjustmentReport = null,
}: {
  /** In section-focus mode, the fit button doubles as "exit focus". */
  focused: boolean;
  onFitOrExitFocus: () => void;
  /** Remote project open without the write lock (Survey mode only). */
  showSpeleoReadOnly: boolean;
  /**
   * Closure-adjusted geometry is why editing is off. Shown only when nothing
   * else already explains the read-only state - see `MapCanvas` for the rule.
   */
  showClosureReadOnly: boolean;
  /** The closure solve is running (can be true while the notice above is not). */
  showClosureComputing: boolean;
  /**
   * What the adjusted geometry on screen left out (#737), or null when no
   * adjusted geometry is shown. Gated in `MapCanvas` like the notices above.
   */
  adjustmentReport?: AdjustmentReport | null;
}) {
  const { t } = useTranslation();

  // Bounded on the right at `right-14` (56px), the column of the top-right map
  // chrome: the MapLibre +/- control is ~43px in from the edge (10px margin, 31px
  // box, 2px ring) and the offline indicator sits at `right-14` too. Unbounded,
  // an absolutely positioned row grows to the map's right edge and the notices ran
  // UNDER the zoom control on a narrow map (MAP_MIN_WIDTH, i.e. a small window or
  // a large UI zoom). Inside the bound the fit button keeps its size, the notices
  // shrink and wrap (the words in their own column beside the glyph, the action
  // under the words only once that column would get too narrow), and an action
  // never shrinks. The row spans the map's width, so it lets clicks through where
  // nothing is drawn.
  return (
    <div
      className="pointer-events-none absolute top-2 left-2 right-14 flex items-start gap-2"
      data-testid="map-top-left-overlay"
    >
      {/* The solve's announcement channel, and the ONLY thing that speaks it.
          Mounted unconditionally and empty until the solve starts, because a live
          region injected at the same moment as its text is unreliably announced -
          the chip below appears and disappears with the solve, so it can never be
          the region itself. Same reason `ToastStack` and the selection announcer
          in `App` are permanent. `sr-only` is absolutely positioned, so this is
          not a flex item in any layout sense. */}
      <div role="status" aria-live="polite" aria-atomic="true" className="sr-only">
        {showClosureComputing
          ? t(
              showClosureReadOnly
                ? 'map.geometry.computingReadOnly'
                : 'map.geometry.computing'
            )
          : ''}
      </div>

      <button
        onClick={onFitOrExitFocus}
        className="pointer-events-auto shrink-0 flex items-center gap-1.5 bg-panel border border-border rounded px-2 py-1 text-sm hover:bg-background"
        title={focused ? t('sections.focus.exit') : t('map.fitToViewTitle')}
        aria-label={focused ? t('sections.focus.exit') : t('map.fitToViewTitle')}
        data-testid="fit-to-view-button"
      >
        <FitToViewIcon size={16} />
        {/* In focus mode the button also exits focus - label it so that's clear. */}
        {focused && <span>{t('sections.focus.exitLabel')}</span>}
      </button>

      {showSpeleoReadOnly && (
        <ReadOnlyNotice testId="speleodb-readonly-banner" glyph={<LockIcon size={13} />}>
          <SpeleoReadOnlyLabel />
          <SpeleoResumeEditingButton />
          <SpeleoLogInButton />
        </ReadOnlyNotice>
      )}

      {/* One chip, two reasons to exist. The read-only testid rides ONLY on the
          notice itself: a spec asserting "no closure read-only banner in Plan
          mode" must not be satisfied by a progress indicator.

          Purely visual - it carries no live-region role, because it is mounted and
          unmounted BY the thing it would have to announce. The region above speaks
          for it.

          The glyph is the closed padlock, exactly as the SpeleoDB notice draws it:
          both tell the user the same thing (editing is off). While the solve runs
          the spinner owns that slot instead - a solve in progress is not itself
          the reason editing is off. */}
      {(showClosureReadOnly || showClosureComputing) && (
        <ReadOnlyNotice
          testId={showClosureReadOnly ? 'geometry-readonly-banner' : 'geometry-computing-banner'}
          glyph={showClosureComputing ? (
            <span
              className="w-3.5 h-3.5 border-2 border-accent border-t-transparent rounded-full animate-spin"
              aria-hidden="true"
              data-testid="geometry-computing-spinner"
            />
          ) : <LockIcon size={13} />}
        >
          {showClosureComputing ? (
            <span className={PILL_LABEL_CLS} data-testid="geometry-computing">
              {t(
                showClosureReadOnly
                  ? 'map.geometry.computingReadOnly'
                  : 'map.geometry.computing'
              )}
            </span>
          ) : (
            <span className={PILL_LABEL_CLS}>{t('map.geometry.adjustedBadge')}</span>
          )}
          {/* Leaving the computed view applies whether or not the solve is still
              running; outside Survey (Plan/Export force it) the notice is off and
              so is this. */}
          {showClosureReadOnly && <RawDataResumeButton />}
        </ReadOnlyNotice>
      )}

      {adjustmentReport && <AdjustmentNotice report={adjustmentReport} />}
    </div>
  );
}
