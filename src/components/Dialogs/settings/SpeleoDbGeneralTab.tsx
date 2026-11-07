import { useTranslation } from 'react-i18next';
import { SettingsSection } from './SettingsSection';
import { Button } from '../../common/Button';
import { Select } from '../../common/Select';
import { SpeleoUndoModeControl } from './SpeleoUndoModeControl';
import { COUNTRIES } from '../../../data/countries';
import type { SpeleoProjectMetadataState } from '../../../hooks/useSpeleoProjectMetadata';
import type { Visibility } from '../../../types/ipc.generated';

const inputClass =
  'w-full px-2 py-1 bg-background border border-border rounded text-sm disabled:opacity-60';

/**
 * The General tab of the SpeleoDB project dialog (#206): the open project's
 * server-side details - name, description, country, visibility, coordinates - plus
 * the web-map rendering switch. Read-only users see the fields disabled with a badge.
 *
 * Pure presentation over {@link useSpeleoProjectMetadata}, which the DIALOG owns so
 * its footer Save reads the same diff this form writes.
 */
export function SpeleoDbGeneralTab({ metadata }: { metadata: SpeleoProjectMetadataState }) {
  const { t } = useTranslation();
  const {
    loading,
    loadError,
    reload,
    baseline,
    form,
    update,
    coordsValid,
    canEdit,
    enablingRendering,
    handleEnableRendering,
  } = metadata;

  const metadataContent = loading ? (
      <p className="text-text-secondary text-sm" data-testid="speleodb-settings-loading">
        {t('speleodb.projectSettings.loading')}
      </p>
    ) : loadError || !form || !baseline ? (
      <div className="space-y-3" data-testid="speleodb-project-settings-error">
        <p className="text-error text-sm">{loadError ?? t('speleodb.projectSettings.loadFailed')}</p>
        <Button variant="secondary" onClick={() => void reload()} data-testid="speleodb-settings-retry">
          {t('speleodb.projectSettings.retry')}
        </Button>
      </div>
    ) : (
      <SettingsSection title={t('speleodb.projectSettings.title')}>
        {!canEdit && (
          <div
            className="mb-3 inline-block text-xs px-1.5 py-0.5 bg-warning-20 text-warning rounded"
            data-testid="speleodb-settings-readonly-badge"
          >
            {t('speleodb.projectSettings.readOnlyBadge')}
          </div>
        )}

        <div className="py-2">
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.name')}
          </label>
          <input
            type="text"
            value={form.name}
            disabled={!canEdit}
            onChange={(e) => update('name', e.target.value)}
            className={inputClass}
            aria-label={t('speleodb.projectSettings.fields.name')}
            data-testid="speleodb-settings-name"
          />
        </div>

        <div className="py-2">
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.description')}
          </label>
          <textarea
            value={form.description}
            disabled={!canEdit}
            onChange={(e) => update('description', e.target.value)}
            rows={3}
            className={inputClass}
            aria-label={t('speleodb.projectSettings.fields.description')}
            data-testid="speleodb-settings-description"
          />
        </div>

        <div className="py-2">
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.country')}
          </label>
          <Select
            value={form.country}
            disabled={!canEdit}
            onChange={(v) => update('country', v)}
            options={[
              { value: '', label: t('speleodb.publish.countryPlaceholder'), disabled: true },
              ...COUNTRIES.map((c) => ({ value: c.code, label: c.name })),
            ]}
            aria-label={t('speleodb.projectSettings.fields.country')}
            data-testid="speleodb-settings-country"
          />
        </div>

        <div className="py-2">
          <label className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.fields.visibility')}
          </label>
          <Select
            value={form.visibility}
            disabled={!canEdit}
            onChange={(v) => update('visibility', v as Visibility)}
            options={[
              // [#576 B16] A visibility the server added after this build shipped:
              // shown as the current value, never offered (it is never sent back).
              ...(form.visibility === 'UNKNOWN'
                ? [
                    {
                      value: 'UNKNOWN',
                      label: t('speleodb.projectSettings.visibilityOptions.unknown'),
                      disabled: true,
                    },
                  ]
                : []),
              { value: 'PRIVATE', label: t('speleodb.projectSettings.visibilityOptions.private') },
              { value: 'PUBLIC', label: t('speleodb.projectSettings.visibilityOptions.public') },
            ]}
            aria-label={t('speleodb.projectSettings.fields.visibility')}
            data-testid="speleodb-settings-visibility"
          />
        </div>

        <div className="py-2 grid grid-cols-2 gap-3">
          <div>
            <label className="block text-sm font-medium text-text-primary mb-1">
              {t('speleodb.projectSettings.fields.latitude')}
            </label>
            <input
              type="number"
              value={form.latitude}
              disabled={!canEdit}
              onChange={(e) => update('latitude', e.target.value)}
              className={inputClass}
              aria-label={t('speleodb.projectSettings.fields.latitude')}
              data-testid="speleodb-settings-latitude"
            />
          </div>
          <div>
            <label className="block text-sm font-medium text-text-primary mb-1">
              {t('speleodb.projectSettings.fields.longitude')}
            </label>
            <input
              type="number"
              value={form.longitude}
              disabled={!canEdit}
              onChange={(e) => update('longitude', e.target.value)}
              className={inputClass}
              aria-label={t('speleodb.projectSettings.fields.longitude')}
              data-testid="speleodb-settings-longitude"
            />
          </div>
        </div>
        {!coordsValid && (
          <p className="text-error text-xs pb-2" data-testid="speleodb-settings-coords-error">
            {t('speleodb.projectSettings.coordinatesHint')}
          </p>
        )}

        <div className="py-2 border-t border-border">
          <span className="block text-sm font-medium text-text-primary mb-1">
            {t('speleodb.projectSettings.rendering.label')}
          </span>
          {baseline.exclude_geojson ? (
            <Button
              variant="secondary"
              disabled={!canEdit || enablingRendering}
              onClick={() => void handleEnableRendering()}
              data-testid="speleodb-settings-enable-rendering"
            >
              {enablingRendering
                ? t('speleodb.banner.enableRenderingBusy')
                : t('speleodb.projectSettings.rendering.enableButton')}
            </Button>
          ) : (
            <p className="text-text-secondary text-xs" data-testid="speleodb-settings-rendering-note">
              {t('speleodb.projectSettings.rendering.enabledNote')}
            </p>
          )}
        </div>

      </SettingsSection>
    );

  return (
    <div className="space-y-3" data-testid="speleodb-settings-tab">
      {metadataContent}
      {/* Local project-file state stays available when server metadata is offline. */}
      <SpeleoUndoModeControl />
    </div>
  );
}
