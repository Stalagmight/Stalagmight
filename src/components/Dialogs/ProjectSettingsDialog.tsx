import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useShallow } from 'zustand/react/shallow';
import { DialogWrapper } from './DialogWrapper';
import { ValidationSettings } from './settings/ValidationSettings';
import { SettingsSection } from './settings/SettingsSection';
import { SettingRow } from './settings/SettingRow';
import { DefaultNorthReferenceRow, useNorthReferenceDraft } from './settings/DefaultNorthReferenceRow';
import { ProjectSettingsSpeleoStub } from './settings/ProjectSettingsSpeleoStub';
import { getPanelId, getTabId, type SettingsCategory } from './SettingsSidebar';
import { Button } from '../common/Button';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { isSettingUnreadable, liftUnreadableSetting } from '../../store/unreadableSettings';
import { isMoveCarryRefusal } from '../../store/moveCarry';
import { useSpeleoStore } from '../../store/speleoStore';
import { defaultConfig, type Config } from '../../types/config';
import { isValidationConfigValid } from '../../utils/validation';
import { TOAST_KEYS } from '../../utils/toastKeys';
import { useEditBlockedReason, useEditingBlocked } from '../../hooks/useEditingBlocked';
import { isEditGateCode } from '../../utils/editErrors';
import type { CommandRefused } from '../../store/commandResult';
import { convertLength, convertLengthToMeters, clampPrecision } from '../../utils/formatStats';

// A compiler-checked subset of SettingsCategory, so these tabs can never drift
// from the shared id helpers they are paired with.
type ProjectSettingsTab = Extract<SettingsCategory, 'general' | 'validation' | 'speleodb'>;
// The local (always-shown) tabs; the remote-only 'speleodb' tab is appended below.
const TABS: readonly ProjectSettingsTab[] = ['general', 'validation'];
const EMPTY_CHANGED: ReadonlySet<string> = new Set();

/**
 * Per-project settings (#57). Mirrors the global Settings dialog's structure
 * (sidebar tabs + sections) but edits the values stored in the .stalx that
 * travel with the project: General = project name, entrance elevation and the
 * default north reference (#576 NF-W3-2), Validation = the same thresholds as
 * the global settings (which are the defaults for new projects).
 */
export function ProjectSettingsDialog() {
  const { t } = useTranslation();
  const closeDialog = useUIStore((s) => s.closeDialog);
  const showToast = useUIStore((s) => s.showToast);
  // The SpeleoDB tab is only meaningful for a remote (linked) project.
  // The BOOLEAN, not the link object: `seedFromStatus` hands out a fresh link on
  // every poll tick, so selecting the object re-rendered the whole settings dialog
  // on each one.
  const isRemote = useSpeleoStore((s) => s.link != null);
  const { project, globalConfig, updateProjectSettings, setSurfaceElevation } = useProjectStore(
    useShallow((s) => ({
      project: s.project,
      globalConfig: s.config,
      updateProjectSettings: s.updateProjectSettings,
      setSurfaceElevation: s.setSurfaceElevation,
    })),
  );
  // The elevation field disables itself in every read-only mode (docs/app-modes.md
  // guard-parity rule): the backend also rejects it via guard_mutation, but the UI
  // gate comes first.
  const editingBlocked = useEditingBlocked();
  // [#576 RV3-FE-14] The name and the thresholds stay editable in the read-only
  // workspaces (they are on the backend's allowlist), but not on a project whose
  // metadata the backend refuses to change at all: a file a newer version saved, or a
  // SpeleoDB project open without the lock (`guard_metadata_mutation`).
  const blockedReason = useEditBlockedReason();
  const metadataBlocked = blockedReason === 'newerFile' || blockedReason === 'lock';
  const panelIdBase = useId();

  const [tab, setTab] = useState<ProjectSettingsTab>('general');
  const renderedTabs = useMemo<readonly ProjectSettingsTab[]>(
    () => (isRemote ? [...TABS, 'speleodb'] : TABS),
    [isRemote],
  );
  // If the project unlinks from SpeleoDB while this dialog sits on the SpeleoDB tab
  // (e.g. a convert-to-local from another surface), that tab disappears from
  // renderedTabs and its panel is gated off - fall back to a valid tab so the body
  // doesn't render blank.
  useEffect(() => {
    if (!isRemote && tab === 'speleodb') setTab('general');
  }, [isRemote, tab]);
  const [name, setName] = useState(project?.name ?? '');
  // Keep the General-tab name field in step when the project name changes underneath
  // us - the SpeleoDB tab's Save folds the server-authoritative name into the store
  // (SpeleoDbSettingsTab), and the (staged, on-mount-once) `name` above would otherwise
  // go stale. A stale field makes the shared footer Save compute settingsChanged=true
  // and REVERT the folded name (re-dirtying the project). Re-seeding here keeps both
  // tabs consistent; a user's in-progress General-tab rename only changes local state
  // (never project.name until its own Save), so this never clobbers an active edit.
  useEffect(() => {
    setName(project?.name ?? '');
  }, [project?.name]);
  const [validation, setValidation] = useState<Config['validation']>(
    project?.validation ?? defaultConfig.validation,
  );
  const [hasValidationError, setHasValidationError] = useState(false);
  const [saving, setSaving] = useState(false);

  // Entrance elevation (#465): edited in the project's configured depth unit,
  // stored in meters. Empty string means "not recorded" (surface_elevation: null).
  const elevationUnit = globalConfig?.units.depth ?? defaultConfig.units.depth;
  const elevationPrecision = globalConfig?.units.decimal_precision.depth ?? defaultConfig.units.decimal_precision.depth;
  const [elevationDraft, setElevationDraft] = useState('');
  // The draft AS SEEDED from the project, so Save can compare strings rather
  // than re-parsing + re-converting the (possibly display-rounded) draft back
  // to meters: comparing against the stored meters value with a tolerance
  // made an UNTOUCHED Save rewrite the elevation whenever the display
  // precision was coarser than the stored value (e.g. 1234.56 at precision 1)
  // or the unit conversion round-trip didn't land exactly (any feet project).
  const [elevationInitialDraft, setElevationInitialDraft] = useState('');
  useEffect(() => {
    const meters = project?.surface_elevation ?? null;
    const seeded =
      meters === null || meters === undefined
        ? ''
        : convertLength(meters, elevationUnit).toFixed(clampPrecision(elevationPrecision));
    setElevationDraft(seeded);
    setElevationInitialDraft(seeded);
  }, [project?.surface_elevation, elevationUnit, elevationPrecision]);
  const elevationParsed = elevationDraft.trim() === '' ? null : Number(elevationDraft);
  // Empty clears to None; anything else must parse to a finite number - a
  // non-finite / unparsable draft blocks Save rather than silently rounding.
  const elevationValid = elevationParsed === null || Number.isFinite(elevationParsed);
  const elevationDraftChanged = elevationDraft !== elevationInitialDraft;

  // Default north reference (#576 NF-W3-2): saved with the name and thresholds, but a
  // survey edit (NF-W3-2-PLAN) - it turns every section that declares none - so, like
  // the elevation, it is disabled in every read-only mode, Plan and Export included,
  // and says why. The backend refuses a changed one there.
  const north = useNorthReferenceDraft(project?.default_azimuth_reference);

  // ValidationSettings is built against a full Config; wrap the project's
  // validation in a synthetic config and funnel its onChange back into local
  // state. It only ever mutates `.validation`.
  const syntheticConfig = useMemo<Config>(() => ({ ...defaultConfig, validation }), [validation]);
  const handleConfigChange = useCallback((updates: Partial<Config>) => {
    if (updates.validation) setValidation(updates.validation);
  }, []);
  const handleValidationError = useCallback((_cat: SettingsCategory, hasError: boolean) => {
    setHasValidationError(hasError);
  }, []);

  const trimmedName = name.trim();
  // Gate on the values directly (robust even if the Validation tab was never
  // opened), plus the live component check while it is mounted.
  const validationValid = isValidationConfigValid(validation);
  // Read-only mode blocks ONLY the elevation write (see the field's own
  // `disabled={editingBlocked}` below): folding it into `canSave` would block
  // the whole dialog - name + validation thresholds - in closure-adjusted/Plan/
  // Export views, where those saves are allowed and the backend accepts them.
  const canSave =
    !!project &&
    trimmedName.length > 0 &&
    validationValid &&
    !hasValidationError &&
    elevationValid &&
    north.valid &&
    !saving;

  // A refused write keeps the dialog open; say why - unless the refusal already said so
  // itself: a move carry (GHOST-2) and a read-only refusal (an edit-gate code, which the
  // store's edit path shows as the friendly read-only notice, #576 NF-W2RO-1) do.
  const reportSaveFailure = useCallback(
    (result: CommandRefused) => {
      setSaving(false);
      if (isMoveCarryRefusal(result) || isEditGateCode(result.code)) return;
      showToast(result.error ?? t('dialogs.projectSettings.saveFailed'), TOAST_KEYS.PROJECT_SETTINGS, 'error');
    },
    [showToast, t]
  );

  const handleSave = useCallback(async () => {
    if (
      !project ||
      trimmedName.length === 0 ||
      !isValidationConfigValid(validation) ||
      hasValidationError ||
      !elevationValid ||
      !north.valid ||
      saving
    )
      return;
    setSaving(true);
    // Skip the settings command when neither the name nor the validation changed:
    // it routes through executeCommand, which marks every success dirty, so even
    // the backend's no-op early-return would otherwise leave an unsaved-changes
    // prompt and a misleading history item for a Save that changed nothing (Codex).
    //
    // [#576 B15 follow-up] ...except while the stored thresholds could not be read:
    // then saving this dialog IS the user setting them (the ones shown are the
    // defaults standing in), which is what lets a project save write them again -
    // not where the backend refuses any metadata change (RV3-FE-14): there an
    // untouched Save would only earn a refusal, so it sends nothing and closes.
    // The default north reference rides the same command (#576 NF-W3-2), and is set
    // by it the same way while unread - but, as a survey edit, only where editing is
    // not blocked (NF-W3-2-PLAN), like the elevation below.
    const currentValidation = project.validation ?? defaultConfig.validation;
    const settingsUnread =
      (isSettingUnreadable('validation') && !metadataBlocked) ||
      (isSettingUnreadable('default_azimuth_reference') && !editingBlocked);
    const settingsChanged =
      trimmedName !== (project.name ?? '') ||
      JSON.stringify(validation) !== JSON.stringify(currentValidation) ||
      north.changed;
    if (settingsChanged || settingsUnread) {
      const result = await updateProjectSettings(trimmedName, validation, north.reference);
      if (!result.success) {
        reportSaveFailure(result);
        return;
      }
      liftUnreadableSetting('validation');
      // The backend sets an unread reference only outside Plan/Export (its workspace
      // check, NF-W3-2-PLAN); mirror that so the banner and the backend agree.
      if (useUIStore.getState().appMode === 'survey') liftUnreadableSetting('default_azimuth_reference');
    }
    // Compare the DRAFT STRING against what it was seeded with, not the
    // re-parsed/re-converted value against the stored meters: the draft is
    // rounded to display precision and unit-converted, so re-converting it
    // back to meters for an untouched field can land a hair off the stored
    // value (feet round-trip, or a stored value finer than display
    // precision) and would otherwise push a spurious write/undo entry.
    //
    // `editingBlocked` is NOT checked here: it only disables the input
    // against NEW edits (see `disabled={editingBlocked}` below). Gating the
    // SAVE on it too would let a flip from editable to read-only, landing
    // between typing and clicking Save, silently discard an already-staged
    // change (closeDialog() would still run). The backend is authoritative -
    // attempt the write whenever the draft changed, and if guard_mutation
    // rejects it (an edit-gate code), fall into the same failure path as a
    // rejected name/validation save: dialog stays open, error shown, the
    // user's staged draft is untouched.
    //
    // [#576 B15 follow-up] An unread stored elevation is set by saving the dialog
    // (as for the thresholds above) - unless editing is blocked, where the field
    // is disabled and the backend would refuse the write.
    const elevationUnread = isSettingUnreadable('surface_elevation') && !editingBlocked;
    if (elevationDraftChanged || elevationUnread) {
      const elevationMeters =
        elevationParsed === null ? null : convertLengthToMeters(elevationParsed, elevationUnit);
      const result = await setSurfaceElevation(elevationMeters);
      if (!result.success) {
        reportSaveFailure(result);
        return;
      }
      liftUnreadableSetting('surface_elevation');
    }
    setSaving(false);
    closeDialog();
  }, [
    trimmedName,
    hasValidationError,
    elevationValid,
    elevationDraftChanged,
    elevationParsed,
    elevationUnit,
    saving,
    updateProjectSettings,
    setSurfaceElevation,
    validation,
    closeDialog,
    reportSaveFailure,
    project,
    editingBlocked,
    metadataBlocked,
    north.valid,
    north.changed,
    north.reference,
  ]);

  return (
    <DialogWrapper
      title={t('dialogs.projectSettings.title')}
      onClose={closeDialog}
      // Fixed height (like the global Settings dialog) so the content pane
      // scrolls within the dialog instead of overflowing the screen.
      fixedSize={{ width: 'min(720px, 95vw)', height: 'clamp(420px, 75vh, 760px)' }}
      closeOnBackdropClick={false}
      overflow="hidden"
      bodyClassName="flex flex-col flex-1 min-h-0"
      data-testid="project-settings-dialog"
    >
      {!project ? (
        <p className="text-text-secondary p-4">{t('dialogs.projectSettings.noProject')}</p>
      ) : (
        <>
          <div className="flex flex-1 min-h-0">
            {/* Sidebar tabs (mirrors the global Settings structure). */}
            <div className="w-44 shrink-0 border-r border-border p-2 space-y-1" role="tablist">
              {renderedTabs.map((tk) => (
                <button
                  key={tk}
                  id={getTabId(panelIdBase, tk)}
                  role="tab"
                  aria-selected={tab === tk}
                  aria-controls={getPanelId(panelIdBase, tk)}
                  onClick={() => setTab(tk)}
                  className={`w-full text-left px-3 py-1.5 rounded text-sm ${tab === tk ? 'bg-accent-10 text-text-primary' : 'text-text-secondary hover:bg-background-50'}`}
                  data-testid={`project-settings-tab-${tk}`}
                >
                  {t(`dialogs.projectSettings.tabs.${tk}`)}
                </button>
              ))}
            </div>

            <div className="flex-1 min-w-0 overflow-auto p-4">
              {tab === 'general' && (
                <div role="tabpanel" id={getPanelId(panelIdBase, 'general')} aria-labelledby={getTabId(panelIdBase, 'general')}>
                  <SettingsSection title={t('dialogs.projectSettings.tabs.general')}>
                    <SettingRow
                      label={t('dialogs.projectSettings.name.label')}
                      helpText={
                        isRemote
                          ? t('dialogs.projectSettings.name.remoteHelp')
                          : t('dialogs.projectSettings.name.help')
                      }
                      data-testid="project-settings-name-row"
                    >
                      <input
                        type="text"
                        value={name}
                        // A remote project's name is authoritative on SpeleoDB: editing it
                        // here would be reverted by the pre-push name fold (#216), so route
                        // renames to the SpeleoDB tab and disable this field.
                        disabled={isRemote}
                        onChange={(e) => setName(e.target.value)}
                        className="w-64 px-2 py-1 bg-background border border-border rounded text-sm disabled:opacity-60"
                        aria-label={t('dialogs.projectSettings.name.label')}
                        data-testid="project-settings-name"
                      />
                    </SettingRow>
                    <SettingRow
                      label={t('dialogs.projectSettings.elevation.label')}
                      helpText={t('dialogs.projectSettings.elevation.help')}
                      data-testid="project-settings-elevation-row"
                    >
                      <div className="flex items-center gap-2">
                        <input
                          type="number"
                          step="any"
                          value={elevationDraft}
                          disabled={editingBlocked}
                          onChange={(e) => setElevationDraft(e.target.value)}
                          placeholder={t('dialogs.projectSettings.elevation.notRecorded')}
                          className="w-32 px-2 py-1 bg-background border border-border rounded text-sm disabled:opacity-60"
                          aria-label={t('dialogs.projectSettings.elevation.aria')}
                          title={editingBlocked ? t('dialogs.projectSettings.elevation.readOnlyReason') : undefined}
                          data-testid="project-settings-elevation"
                        />
                        <span className="text-xs text-text-secondary">
                          {elevationUnit === 'feet' ? t('common.units.feet') : t('common.units.meters')}
                        </span>
                      </div>
                    </SettingRow>
                    <DefaultNorthReferenceRow
                      draft={north}
                      disabledReason={
                        blockedReason !== null ? t(`dialogs.projectSettings.northReference.blocked.${blockedReason}`) : null
                      }
                    />
                  </SettingsSection>
                </div>
              )}
              {tab === 'validation' && (
                <ValidationSettings
                  config={syntheticConfig}
                  onChange={handleConfigChange}
                  changedFields={EMPTY_CHANGED as Set<string>}
                  onValidationError={handleValidationError}
                  panelIdBase={panelIdBase}
                  scope="project"
                  defaults={globalConfig?.validation ?? defaultConfig.validation}
                />
              )}
              {tab === 'speleodb' && isRemote && (
                <div role="tabpanel" id={getPanelId(panelIdBase, 'speleodb')} aria-labelledby={getTabId(panelIdBase, 'speleodb')}>
                  <ProjectSettingsSpeleoStub />
                </div>
              )}
            </div>
          </div>

          <div className="border-t border-border p-3 flex justify-end gap-2 shrink-0">
            <Button variant="secondary" onClick={closeDialog} data-testid="project-settings-cancel">
              {t('common.actions.cancel')}
            </Button>
            <Button variant="primary" onClick={handleSave} disabled={!canSave} data-testid="project-settings-save">
              {t('common.actions.save')}
            </Button>
          </div>
        </>
      )}
    </DialogWrapper>
  );
}
