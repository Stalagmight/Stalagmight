import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { useSpeleoStore } from '../../store/speleoStore';
import { invokeIpc } from '../../utils/ipc';
import { humanizeSpeleoError, speleoErrorAction } from '../../utils/speleoError';
import { validateCoordinates } from '../../utils/gps';
import { reverseGeocodeCountry } from '../../utils/reverseGeocodeCountry';
import { COUNTRIES } from '../../data/countries';
import { TOAST_KEYS } from '../../utils/toastKeys';
import type {
  SpeleoPublishParams,
  SpeleoPublishResult,
  Visibility,
} from '../../types/ipc.generated';

const inputClass = 'w-full px-2 py-1 bg-background border border-border rounded text-sm';

/**
 * Publish the OPEN local project to SpeleoDB (#207). Collects the required metadata
 * (name + country) plus optional description / visibility / coordinates, calls
 * `speleodb_publish`, then reloads the now-remote project so the link status
 * re-seeds. Reachable from the project menu and from New Project's follow-up
 * checkbox.
 */
export default function PublishToSpeleoDbDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const showToast = useUIStore((s) => s.showToast);
  const project = useProjectStore((s) => s.project);

  const [name, setName] = useState(project?.name ?? '');
  const [description, setDescription] = useState('');
  const [message, setMessage] = useState('');
  // Best-effort prefill from the first georeferenced zero datum, if any.
  const firstGps = useMemo(() => project?.zero_datums?.[0]?.gps ?? null, [project]);
  // Offline reverse-geocode the required country from that datum (#217). Lazy
  // initializer so it runs once at mount; empty when there is no GPS or the point
  // resolves to no country. Always user-editable via the <select> below.
  const [country, setCountry] = useState(() => {
    const gps = project?.zero_datums?.[0]?.gps ?? null;
    return gps ? (reverseGeocodeCountry(gps.latitude, gps.longitude) ?? '') : '';
  });
  const [visibility, setVisibility] = useState<Visibility>('PRIVATE');
  const [latitude, setLatitude] = useState(firstGps ? String(firstGps.latitude) : '');
  const [longitude, setLongitude] = useState(firstGps ? String(firstGps.longitude) : '');
  const [publishing, setPublishing] = useState(false);

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
    message.trim().length > 0 &&
    country !== '' &&
    coordsValid &&
    !publishing;

  const handlePublish = useCallback(async () => {
    if (!canPublish) return;
    setPublishing(true);
    // Both-or-neither: only send numbers when both are present and valid.
    const bothCoords = latTrim !== '' && lonTrim !== '';
    const params: SpeleoPublishParams = {
      name: name.trim(),
      description,
      message: message.trim(),
      country,
      visibility,
      latitude: bothCoords ? latNum : null,
      longitude: bothCoords ? lonNum : null,
    };
    try {
      const result = await invokeIpc<SpeleoPublishResult>('speleodb_publish', { params });
      // The project now EXISTS on the server and is linked. Reload its cache copy so the
      // app converges on the now-remote project. loadProject reports failure via
      // {success:false} (it never throws - it may have opened a recovery dialog), so a
      // failed reload must NOT claim success or close over a stale/local view.
      const { success } = await useProjectStore.getState().loadProject(result.local_path);
      if (!success) {
        showToast(
          t('speleodb.publish.reloadFailed'),
          TOAST_KEYS.SPELEODB_PUBLISH,
          'error',
        );
        return;
      }
      // UI-convergence only: a failure refreshing the link status must NOT flip a
      // succeeded publish into the error path (the link is already stamped on the
      // reloaded project), so swallow it.
      try {
        await useSpeleoStore.getState().refreshLinkStatus();
      } catch {
        // non-fatal; leave the status to the next refresh.
      }
      closeDialog();
    } catch (e) {
      showToast(humanizeSpeleoError(e, t('speleodb.publish.failed')), TOAST_KEYS.SPELEODB_PUBLISH, 'error');
      if (speleoErrorAction(e) === 'ReLogin') {
        useSpeleoStore.getState().clearSession();
      }
    } finally {
      setPublishing(false);
    }
  }, [
    canPublish,
    name,
    description,
    message,
    country,
    visibility,
    latTrim,
    lonTrim,
    latNum,
    lonNum,
    showToast,
    closeDialog,
    t,
  ]);

  const isDirty =
    name.trim() !== (project?.name ?? '').trim() ||
    description.trim() !== '' ||
    message.trim() !== '' ||
    country !== '' ||
    visibility !== 'PRIVATE' ||
    // Include the coordinate pins so entering ONLY a location still prompts on close.
    latTrim !== '' ||
    lonTrim !== '';

  return (
    <DialogWrapper
      title={t('speleodb.publish.title')}
      onClose={closeDialog}
      size="md"
      isDirty={isDirty}
      data-testid="publish-to-speleodb-dialog"
    >
      <div className="space-y-4">
        <p className="text-text-secondary text-sm">{t('speleodb.publish.description')}</p>

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.name')} <span className="text-error">*</span>
          </label>
          <input
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            className={inputClass}
            aria-label={t('speleodb.projectSettings.fields.name')}
            data-testid="publish-to-speleodb-dialog-name"
          />
        </div>

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.description')} <span className="text-error">*</span>
          </label>
          <textarea
            value={description}
            onChange={(e) => setDescription(e.target.value)}
            rows={3}
            className={inputClass}
            aria-label={t('speleodb.projectSettings.fields.description')}
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

        <div>
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.publish.visibility')}
          </label>
          <Select
            value={visibility}
            onChange={(v) => setVisibility(v as Visibility)}
            options={[
              { value: 'PRIVATE', label: t('speleodb.projectSettings.visibilityOptions.private') },
              { value: 'PUBLIC', label: t('speleodb.projectSettings.visibilityOptions.public') },
            ]}
            aria-label={t('speleodb.publish.visibility')}
            data-testid="publish-to-speleodb-dialog-visibility"
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
            {t('speleodb.projectSettings.coordinatesHint')}
          </p>
        )}

        <div>
          <label htmlFor="publish-commit-message" className="block text-sm text-text-secondary mb-1">
            {t('speleodb.done.messageLabel')}
          </label>
          <textarea
            id="publish-commit-message"
            data-testid="publish-to-speleodb-dialog-message"
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            disabled={publishing}
            required
            rows={3}
            className="w-full px-2 py-1 bg-background border border-border rounded text-sm"
          />
        </div>
        <div className="flex justify-end gap-2 pt-4 border-t border-border">
          <Button onClick={closeDialog} disabled={publishing} data-testid="publish-to-speleodb-dialog-cancel">
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
  );
}
