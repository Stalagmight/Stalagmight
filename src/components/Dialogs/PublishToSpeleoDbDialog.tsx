import { publishProjectAndReopen } from '../../utils/publishProjectAndReopen';
import { getPublishContinuation, publishFormDraft, setPublishContinuation, type PublishDraft } from '../../utils/publishContinuation';
import { registerWorkspaceDraft } from '../../utils/workspaceDrafts';
import { captureUploadOwner, uploadOwnerStillCurrent } from '../../utils/uploadContinuation';
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { SpeleoProgressOverlay } from './SpeleoProgressOverlay';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { useSpeleoOperationBusy } from '../../hooks/useSpeleoOperationBusy';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { validateCoordinates } from '../../utils/gps';
import { reverseGeocodeCountry } from '../../utils/reverseGeocodeCountry';
import { COUNTRIES } from '../../data/countries';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { holdQuitDuring } from '../../utils/quitAfterWork';
import type {
  SpeleoPublishParams,
} from '../../types/ipc.generated';

const inputClass = 'w-full px-2 py-1 bg-background border border-border rounded text-sm';

/**
 * Publish the OPEN local project to SpeleoDB (#207). Collects the required metadata
 * (name + country) plus optional description / coordinates, calls
 * `speleodb_publish`, then reloads the now-remote project so the link status
 * re-seeds. Reachable from the project menu and from New Project's follow-up
 * checkbox.
 */
export default function PublishToSpeleoDbDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const showToast = useUIStore((s) => s.showToast);
  const project = useProjectStore((s) => s.project);

  const [name] = useState(project?.name ?? '');
  const [owner] = useState(captureUploadOwner);
  const [draft] = useState(getPublishContinuation);
  const loggedIn = useSpeleoStore((s) => s.session?.loggedIn === true);
  const busy = useSpeleoOperationBusy();
  const [description, setDescription] = useState(draft?.description ?? '');
  // Best-effort prefill from the first georeferenced zero datum, if any.
  const firstGps = useMemo(() => project?.zero_datums?.[0]?.gps ?? null, [project]);
  // Offline reverse-geocode the required country from that datum (#217). Lazy
  // initializer so it runs once at mount; empty when there is no GPS or the point
  // resolves to no country. Always user-editable via the <select> below.
  const [country, setCountry] = useState(() => {
    if (draft) return draft.country;
    const gps = project?.zero_datums?.[0]?.gps ?? null;
    return gps ? (reverseGeocodeCountry(gps.latitude, gps.longitude) ?? '') : '';
  });
  const [latitude, setLatitude] = useState(draft?.latitude ?? (firstGps ? String(firstGps.latitude) : ''));
  const [longitude, setLongitude] = useState(draft?.longitude ?? (firstGps ? String(firstGps.longitude) : ''));
  const [publishing, setPublishing] = useState(false);
  const [progressExpired, setProgressExpired] = useState(false);
  const attempt = useRef(false);
  useEffect(() => {
    if (!publishing) return;
    const timer = window.setTimeout(() => setProgressExpired(true), 120_000);
    return () => window.clearTimeout(timer);
  }, [publishing]);
  const blocking = publishing && !progressExpired;
  // What is typed here is unsent SpeleoDB input: every way out asks before
  // dropping it (`utils/workspaceDrafts`). Not while it is being published.
  const form = useRef<PublishDraft | null>(null);
  form.current = publishing ? null : { description, country, latitude, longitude };
  useEffect(() => registerWorkspaceDraft(publishFormDraft(owner, () => form.current)), [owner]);

  const latTrim = latitude.trim();
  const lonTrim = longitude.trim();
  const latNum = latTrim === '' ? null : Number(latTrim);
  const lonNum = lonTrim === '' ? null : Number(lonTrim);
  const coordsPairValid = (latTrim === '') === (lonTrim === '');
  // When both are provided they must be finite AND in range - reuse the shared
  // validator (finiteness alone would let latitude 999 through to the server).
  const bothProvided = latTrim !== '' && lonTrim !== '';
  const coordsRangeValid =
    !bothProvided || (latNum !== null && lonNum !== null && validateCoordinates(latNum, lonNum).valid);
  const coordsValid = coordsPairValid && coordsRangeValid;

  // SpeleoDB's Project model requires a non-blank description (`blank=False,
  // null=False`), so an empty one 400s the create before anything is written. Gate
  // it here as required rather than let Publish silently fail after the click.
  const canPublish =
    name.trim().length > 0 &&
    description.trim().length > 0 &&
    country !== '' &&
    coordsValid &&
    !publishing && !busy && loggedIn;

  const handlePublish = useCallback(async () => {
    if (!canPublish || attempt.current || !uploadOwnerStillCurrent(owner)) return;
    attempt.current = true;
    const issuedDialog = useUIStore.getState().dialogData;
    const ownsDialog = () => useUIStore.getState().activeDialog === 'publish-to-speleodb' &&
      useUIStore.getState().dialogData === issuedDialog;
    setProgressExpired(false);
    setPublishing(true);
    // Both-or-neither: only send numbers when both are present and valid.
    const bothCoords = latTrim !== '' && lonTrim !== '';
    const params: SpeleoPublishParams = {
      name: name.trim(),
      description,
      message: 'Project Creation',
      country,
      visibility: 'PRIVATE',
      latitude: bothCoords ? latNum : null,
      longitude: bothCoords ? lonNum : null,
    };
    try {
      // lock#6: a quit meanwhile waits for the whole publish, not just its calls.
      const { outcome, undoHistoryOmitted } = await holdQuitDuring(() => publishProjectAndReopen(params));
      if (outcome === 'stale') return;
      setPublishContinuation(null);
      if (ownsDialog()) closeDialog();
      // sdk#3: the first revision went up without its undo history to fit the server's
      // size limit. One event, one surface: the note rides the outcome's own toast,
      // and stands alone (as a warning) when the outcome has none.
      const historyNote = undoHistoryOmitted ? t('speleodb.publish.withoutHistory') : null;
      const withNote = (message: string) => (historyNote ? `${message} ${historyNote}` : message);
      if (outcome === 'upload_pending') {
        showToast(withNote(t('speleodb.publish.uploadPending')), TOAST_KEYS.SPELEODB_PUBLISH, 'warning');
      } else if (outcome === 'recent_cleanup_failed') {
        showToast(withNote(t('speleodb.publish.recentCleanupFailed')), TOAST_KEYS.SPELEODB_PUBLISH, 'warning');
      } else if (outcome === 'reload_failed') {
        showToast(withNote(t('speleodb.publish.reloadFailed')), TOAST_KEYS.SPELEODB_PUBLISH, 'error');
      } else if (historyNote) {
        showToast(historyNote, TOAST_KEYS.SPELEODB_PUBLISH, 'warning');
      }
    } catch (e) {
      showToast(humanizeSpeleoError(e, t('speleodb.publish.failed')), TOAST_KEYS.SPELEODB_PUBLISH, 'error');
    } finally {
      attempt.current = false;
      setPublishing(false);
    }
  }, [
    canPublish,
    owner,
    name,
    description,
    country,
    latTrim,
    lonTrim,
    latNum,
    lonNum,
    showToast,
    closeDialog,
    t,
  ]);

  const isDirty =
    description.trim() !== '' ||
    country !== '' ||
    // Include the coordinate pins so entering ONLY a location still prompts on close.
    latTrim !== '' ||
    lonTrim !== '';

  const dismiss = () => { setPublishContinuation(null); closeDialog(); };
  const signIn = () => {
    if (!uploadOwnerStillCurrent(owner)) return;
    setPublishContinuation({ description, country, latitude, longitude }, owner);
    useUIStore.getState().openDialog('settings', { initialCategory: 'speleodb' });
  };

  return (
    <>
    <DialogWrapper
      title={t('speleodb.publish.title')}
      onClose={blocking ? () => {} : dismiss}
      size="md"
      isDirty={isDirty && !publishing}
      data-testid="publish-to-speleodb-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary text-sm">{t('speleodb.publish.description')}</p>
        {progressExpired && publishing && <p role="alert" className="text-sm text-warning" data-testid="speleodb-publish-timeout">
          Publishing has taken more than 120 seconds. Your request may still finish. Close this dialog to continue working.
        </p>}
        {!loggedIn && <Button onClick={signIn} data-testid="publish-to-speleodb-dialog-sign-in">Connect in Settings</Button>}

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.publish.name')}
          </label>
          <input
            type="text"
            value={name}
            disabled
            className={`${inputClass} opacity-50 cursor-not-allowed`}
            aria-label={t('speleodb.publish.name')}
            data-testid="publish-to-speleodb-dialog-name"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.publish.descriptionLabel')} <span className="text-error">*</span>
          </label>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={3}
            className={inputClass}
            aria-label={t('speleodb.publish.descriptionLabel')}
            data-testid="publish-to-speleodb-dialog-description"
          />
          {description.trim() === '' && (
            <p className="text-error text-xs mt-1" data-testid="publish-to-speleodb-dialog-description-error">
              {t('speleodb.publish.descriptionRequired')}
            </p>
          )}
        </div>

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.publish.country')} <span className="text-error">*</span>
          </label>
          <Select
            searchLabel={t('speleodb.publish.searchCountry')}
            noResultsLabel={t('speleodb.publish.noCountries')}
            value={country}
            onChange={setCountry}
            options={[
              { value: '', label: t('speleodb.publish.countryPlaceholder'), disabled: true },
              ...COUNTRIES.map((c) => ({ value: c.code, label: c.name })),
            ]}
            aria-label={t('speleodb.publish.country')}
            data-testid="publish-to-speleodb-dialog-country"
          />
        </div>

        <div className="grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm font-medium text-text-primary mb-1">
              {t('speleodb.publish.latitude')}
            </label>
            <input
              type="number"
              value={latitude}
              onChange={(e) => setLatitude(e.target.value)}
              className={inputClass}
              aria-label={t('speleodb.publish.latitude')}
              data-testid="publish-to-speleodb-dialog-latitude"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-text-primary mb-1">
              {t('speleodb.publish.longitude')}
            </label>
            <input
              type="number"
              value={longitude}
              onChange={(e) => setLongitude(e.target.value)}
              className={inputClass}
              aria-label={t('speleodb.publish.longitude')}
              data-testid="publish-to-speleodb-dialog-longitude"
            />
          </div>
        </div>
        {!coordsValid && (
          <p className="text-error text-xs" data-testid="publish-to-speleodb-dialog-coords-error">
            {t('speleodb.publish.coordinatesHint')}
          </p>
        )}

        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={dismiss} disabled={blocking} data-testid="publish-to-speleodb-dialog-cancel">
            {t('common.actions.cancel')}
          </Button>
          <Button
            variant="primary"
            onClick={() => void handlePublish()}
            disabled={!canPublish}
            data-testid="publish-to-speleodb-dialog-submit"
          >
            {publishing ? t('speleodb.publish.submitting') : t('speleodb.publish.submit')}
          </Button>
        </div>
      </div>
    </DialogWrapper>
    {blocking && <SpeleoProgressOverlay title="Publishing to SpeleoDB" description="One sec, teaching your project some manners..." testId="speleodb-publish-progress" />}
    </>
  );
}
