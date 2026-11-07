import { useCallback, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { DialogWrapper } from './DialogWrapper';
import { Button } from '../common/Button';
import { Select } from '../common/Select';
import { SpeleoUserAutocomplete } from '../common/SpeleoUserAutocomplete';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { expectedOpenProject } from '../../store/expectedOpenProject';
import { useSpeleoStore } from '../../store/speleoStore';
import { invokeIpc } from '../../utils/ipc';
import { humanizeSpeleoError } from '../../utils/speleoError';
import { errorCode } from '../../utils/commandError';
import { USER_LEVELS } from '../../utils/speleoPermission';
import { validateCoordinates } from '../../utils/gps';
import { reverseGeocodeCountry } from '../../utils/reverseGeocodeCountry';
import { COUNTRIES } from '../../data/countries';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { surfaceSaveWarnings } from '../../utils/saveFeedback';
import type {
  Permission,
  SpeleoPublishParams,
  SpeleoPublishReport,
  SpeleoPublishResult,
  ValidationWarning,
  Visibility,
} from '../../types/ipc.generated';

const inputClass = 'w-full px-2 py-1 bg-background border border-border rounded text-sm';

/**
 * [#576 NF-W4M-2] Publish answers its outcome together with the warnings of the two
 * saves it ran (the relocation into the SpeleoDB cache and the initial push's
 * pre-push save): what they could not keep. One-shot, like Done's and Push's, so
 * they are shown here like a normal save's. [#576 RV6a-6] Only that wire shape is
 * read: a bare outcome is a broken contract and fails the publish loudly.
 */
function readPublishReport(raw: SpeleoPublishReport): {
  result: SpeleoPublishResult;
  saveLosses: ValidationWarning[];
} {
  return { result: raw.result, saveLosses: raw.save_warnings ?? [] };
}

/** A collaborator queued in the dialog, granted only after the project is created. */
interface PendingCollaborator {
  email: string;
  permission: Permission;
}

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

  // Collaborators to grant AFTER the create succeeds - the project does not exist on
  // the server yet, so we queue (email, level) pairs here and add them once it is
  // linked. The email typeahead is session-backed, so it works before publish.
  const [collaborators, setCollaborators] = useState<PendingCollaborator[]>([]);
  const [collabEmail, setCollabEmail] = useState('');
  const [collabLevel, setCollabLevel] = useState<Permission>('READ_ONLY');

  const addCollaborator = useCallback(() => {
    const email = collabEmail.trim();
    if (!email) return;
    setCollaborators((prev) => {
      // De-dupe on a case-insensitive email; a re-add just updates the level.
      const lower = email.toLowerCase();
      const rest = prev.filter((c) => c.email.toLowerCase() !== lower);
      return [...rest, { email, permission: collabLevel }];
    });
    setCollabEmail('');
  }, [collabEmail, collabLevel]);

  const removeCollaborator = useCallback((email: string) => {
    setCollaborators((prev) => prev.filter((c) => c.email !== email));
  }, []);

  const setCollaboratorLevel = useCallback((email: string, permission: Permission) => {
    setCollaborators((prev) => prev.map((c) => (c.email === email ? { ...c, permission } : c)));
  }, []);

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
    !publishing;

  const handlePublish = useCallback(async () => {
    if (!canPublish) return;
    setPublishing(true);
    // Both-or-neither: only send numbers when both are present and valid.
    const bothCoords = latTrim !== '' && lonTrim !== '';
    const params: SpeleoPublishParams = {
      name: name.trim(),
      description,
      country,
      visibility,
      latitude: bothCoords ? latNum : null,
      longitude: bothCoords ? lonNum : null,
    };
    try {
      // [#576 RVF-c-C1] Names the project to publish: refused once another is open.
      const { result, saveLosses } = readPublishReport(
        await invokeIpc('speleodb_publish', { params, expectedProject: expectedOpenProject() }),
      );
      surfaceSaveWarnings(saveLosses);
      // The project now EXISTS on the server and is linked. Reload its cache copy so the
      // app converges on the now-remote project. loadProject reports failure via
      // {success:false} (it never throws - it may have opened a recovery dialog), so a
      // failed reload must NOT claim success or close over a stale/local view.
      const { success } = await useProjectStore.getState().loadProject(result.local_path);
      if (!success) {
        // Published, but the cache copy could not be reopened. Grants target the OPEN
        // link, which we no longer have, so any queued collaborators were NOT added -
        // say so rather than dropping them silently.
        showToast(
          collaborators.length > 0
            ? t('speleodb.publish.reloadFailedWithCollaborators')
            : t('speleodb.publish.reloadFailed'),
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

      // The project is now linked, so `speleodb_add_collaborator` (which targets the
      // open link) can grant each queued collaborator. Sequential + best-effort: a
      // failed grant must NOT undo a successful publish, so we collect the failures
      // and warn, leaving the rest to be finished from Project Settings.
      const failed: string[] = [];
      // The project the grants are for: the published one, reloaded just above.
      const grantsFor = expectedOpenProject();
      for (const c of collaborators) {
        try {
          await invokeIpc('speleodb_add_collaborator', {
            email: c.email,
            permission: c.permission,
            expectedProject: grantsFor,
          });
        } catch {
          failed.push(c.email);
        }
      }
      if (failed.length > 0) {
        showToast(
          t('speleodb.publish.collaboratorsPartial', { emails: failed.join(', ') }),
          TOAST_KEYS.SPELEODB_PUBLISH,
          'warning',
        );
      } else {
        showToast(t('speleodb.publish.success'), TOAST_KEYS.SPELEODB_PUBLISH, 'success');
      }
      closeDialog();
    } catch (e) {
      // [#576 NF-W9BE-1-D1] No `saves` context, though publish saves into the cache
      // first: a project a newer Stalagmight saved reads as the newer-version line,
      // like Start editing and Update, not as "Not saved: ...".
      showToast(humanizeSpeleoError(e, t('speleodb.publish.failed')), TOAST_KEYS.SPELEODB_PUBLISH, 'error');
      if (errorCode(e) === 'SPELEODB_RELOGIN') {
        useSpeleoStore.getState().clearSession();
      }
    } finally {
      setPublishing(false);
    }
  }, [
    canPublish,
    name,
    description,
    country,
    visibility,
    latTrim,
    lonTrim,
    latNum,
    lonNum,
    collaborators,
    showToast,
    closeDialog,
    t,
  ]);

  const isDirty =
    name.trim() !== (project?.name ?? '').trim() ||
    description.trim() !== '' ||
    country !== '' ||
    visibility !== 'PRIVATE' ||
    collaborators.length > 0 ||
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

        <div className="pt-2 border-t border-border">
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.publish.collaborators')}
          </label>
          <p className="text-text-secondary text-xs mb-2">
            {t('speleodb.publish.collaboratorsHint')}
          </p>

          {collaborators.length > 0 && (
            <ul className="space-y-1 mb-2" data-testid="publish-to-speleodb-dialog-collaborators">
              {/* Rows are keyed by position in their test ids: a test id is printed where a
                  run reports what it clicked, and must not carry an email (#576 RV19b-P1). */}
              {collaborators.map((c, i) => (
                <li
                  key={c.email}
                  className="flex items-center gap-2"
                  data-testid={`publish-to-speleodb-dialog-collab-${i}`}
                >
                  <span className="flex-1 min-w-0 truncate text-sm">{c.email}</span>
                  <Select
                    value={c.permission}
                    disabled={publishing}
                    onChange={(v) => setCollaboratorLevel(c.email, v as Permission)}
                    options={USER_LEVELS.map((p) => ({ value: p, label: t(`speleodb.collaborators.levels.${p}`) }))}
                    size="sm"
                    aria-label={t('speleodb.collaborators.levelFor', { name: c.email })}
                    data-testid={`publish-to-speleodb-dialog-collab-level-${i}`}
                  />
                  <button
                    type="button"
                    disabled={publishing}
                    onClick={() => removeCollaborator(c.email)}
                    className="text-error text-xs hover:underline disabled:opacity-60"
                    data-testid={`publish-to-speleodb-dialog-collab-remove-${i}`}
                  >
                    {t('speleodb.collaborators.remove')}
                  </button>
                </li>
              ))}
            </ul>
          )}

          <div className="flex items-center gap-2">
            <SpeleoUserAutocomplete
              value={collabEmail}
              onChange={setCollabEmail}
              disabled={publishing}
              enabled={!publishing}
              className={inputClass}
              placeholder={t('speleodb.collaborators.emailPlaceholder')}
              ariaLabel={t('speleodb.collaborators.addUser')}
              inputTestId="publish-to-speleodb-dialog-collab-email"
              listTestId="publish-to-speleodb-dialog-collab-suggestions"
              suggestionTestIdPrefix="publish-to-speleodb-dialog-collab-suggestion"
            />
            <Select
              value={collabLevel}
              disabled={publishing}
              onChange={(v) => setCollabLevel(v as Permission)}
              options={USER_LEVELS.map((p) => ({ value: p, label: t(`speleodb.collaborators.levels.${p}`) }))}
              aria-label={t('speleodb.collaborators.addLevel')}
              data-testid="publish-to-speleodb-dialog-collab-add-level"
            />
            <Button
              variant="secondary"
              disabled={publishing || collabEmail.trim() === ''}
              onClick={addCollaborator}
              data-testid="publish-to-speleodb-dialog-collab-add"
            >
              {t('speleodb.collaborators.add')}
            </Button>
          </div>
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
