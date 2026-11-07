import { useCallback, useEffect, useRef, useState } from 'react';
import { useTranslation } from 'react-i18next';
import i18n from '../i18n';
import { useUIStore } from '../store/uiStore';
import { useProjectStore } from '../store/projectStore';
import { expectedOpenProject } from '../store/expectedOpenProject';
import { useSpeleoStore } from '../store/speleoStore';
import { invokeIpc } from '../utils/ipc';
import { humanizeSpeleoError } from '../utils/speleoError';
import { toastProjectChanged } from '../utils/projectChangedNotice';
import { isProjectChangedRefusal } from '../store/commandResult';
import { permissionAtLeast } from '../utils/speleoPermission';
import { validateCoordinates } from '../utils/gps';
import { TOAST_KEYS } from '../utils/toastKeys';
import type {
  SpeleoProjectMetadata,
  SpeleoProjectMetadataPatch,
  Visibility,
} from '../types/ipc.generated';

/** Editable copy of the server metadata that the form binds to. Lat/long are held
 *  as strings so an in-progress edit ("-" / "12.") doesn't fight the number input. */
export interface SpeleoMetadataForm {
  name: string;
  description: string;
  country: string;
  visibility: Visibility;
  latitude: string;
  longitude: string;
}

export interface SpeleoProjectMetadataState {
  loading: boolean;
  loadError: string | null;
  reload: () => Promise<void>;
  /** The last server truth we loaded / saved; also carries `exclude_geojson`. */
  baseline: SpeleoProjectMetadata | null;
  form: SpeleoMetadataForm | null;
  update: <K extends keyof SpeleoMetadataForm>(key: K, value: SpeleoMetadataForm[K]) => void;
  coordsValid: boolean;
  canEdit: boolean;
  canSave: boolean;
  saving: boolean;
  handleSave: () => Promise<void>;
  enablingRendering: boolean;
  handleEnableRendering: () => Promise<void>;
}

/** Turn the server metadata into the string-backed form shape. */
function toForm(m: SpeleoProjectMetadata): SpeleoMetadataForm {
  return {
    name: m.name,
    description: m.description,
    country: m.country,
    visibility: m.visibility,
    latitude: m.latitude === null ? '' : String(m.latitude),
    longitude: m.longitude === null ? '' : String(m.longitude),
  };
}

function sameForm(a: SpeleoMetadataForm, b: SpeleoMetadataForm): boolean {
  return (
    a.name === b.name &&
    a.description === b.description &&
    a.country === b.country &&
    a.visibility === b.visibility &&
    a.latitude === b.latitude &&
    a.longitude === b.longitude
  );
}

/** The baseline and the form move together, so one update can decide both. */
interface MetadataState {
  baseline: SpeleoProjectMetadata | null;
  form: SpeleoMetadataForm | null;
}

/**
 * [#576 D26] Fold a freshly loaded server truth in: the baseline always moves, and
 * the form takes the new truth for every field except the ones the user has typed
 * (those that differ from the previous baseline's form), which keep what was typed.
 * A reload used to replace both, discarding what was being typed.
 *
 * Field by field, not the whole form (#576 RV4-FE-SP2): keeping the entire previous
 * form left every UNTOUCHED field on its old value, which then differed from the new
 * baseline, so the next Save PATCHed it and silently reverted what another client had
 * changed. Latitude and longitude travel as one pair, as the PATCH sends them.
 */
function foldLoaded(prev: MetadataState, loaded: SpeleoProjectMetadata): MetadataState {
  const fresh = toForm(loaded);
  const typed = prev.form;
  if (typed === null || prev.baseline === null) return { baseline: loaded, form: fresh };
  const was = toForm(prev.baseline);
  const keep = <K extends keyof SpeleoMetadataForm>(key: K): SpeleoMetadataForm[K] =>
    typed[key] !== was[key] ? typed[key] : fresh[key];
  const coordinatesTyped = typed.latitude !== was.latitude || typed.longitude !== was.longitude;
  const form: SpeleoMetadataForm = {
    name: keep('name'),
    description: keep('description'),
    country: keep('country'),
    visibility: keep('visibility'),
    latitude: coordinatesTyped ? typed.latitude : fresh.latitude,
    longitude: coordinatesTyped ? typed.longitude : fresh.longitude,
  };
  return { baseline: loaded, form: sameForm(form, fresh) ? fresh : form };
}

/**
 * The open remote project's server metadata: load, edit, and PATCH back only what
 * changed (#206). Editing needs READ_AND_WRITE or higher; a read-only user gets the
 * loaded values with `canEdit` false.
 *
 * This lives in a hook rather than in the tab body because the SpeleoDB project
 * dialog splits the two halves apart: the FOOTER owns Save (`canSave`/`saving`/
 * `handleSave`) while the General tab BODY owns the form, and both must read one
 * source of truth or the button would gate on a stale diff.
 */
export function useSpeleoProjectMetadata(): SpeleoProjectMetadataState {
  const { t } = useTranslation();
  const showToast = useUIStore((s) => s.showToast);
  // Select the two primitives rather than the link object: the status poll hands
  // out a fresh object on every tick, which would re-run the load effect below.
  const isLinked = useSpeleoStore((s) => s.link !== null);
  const permission = useSpeleoStore((s) => s.link?.permission ?? null);
  const refreshLinkStatus = useSpeleoStore((s) => s.refreshLinkStatus);

  const canEdit = permissionAtLeast(permission, 'READ_AND_WRITE');

  const [loading, setLoading] = useState(true);
  const [loadError, setLoadError] = useState<string | null>(null);
  // `baseline` is the last server truth we loaded / saved - what changed fields diff
  // against; `form` is the user's editable copy.
  const [{ baseline, form }, setMetadata] = useState<MetadataState>({
    baseline: null,
    form: null,
  });
  const [saving, setSaving] = useState(false);
  const [enablingRendering, setEnablingRendering] = useState(false);
  // [#576 D26] The sequence number of the newest load. A reply applies only while
  // it is still the newest one issued (an older reply resolving later must not
  // overwrite a newer one), and the effect's cleanup bumps it so nothing lands after
  // unmount or after the effect re-ran. The neighbouring loaders guard the same way
  // (SpeleoRevisionHistory, SpeleoCollaboratorsTab).
  const loadSeq = useRef(0);

  const reload = useCallback(async () => {
    loadSeq.current += 1;
    const seq = loadSeq.current;
    const current = () => seq === loadSeq.current;
    // A project with no link has no server metadata; the dialog shows its
    // not-linked message instead of the form, so skip the doomed round trip.
    if (!isLinked) {
      setLoading(false);
      return;
    }
    setLoading(true);
    setLoadError(null);
    try {
      const meta = await invokeIpc('speleodb_get_project_metadata');
      if (!current()) return;
      setMetadata((prev) => foldLoaded(prev, meta));
    } catch (e) {
      if (!current()) return;
      // Translated when the error happens rather than closing over `t`: `t` changes
      // identity on a language switch, which re-ran this load over the open form.
      setLoadError(humanizeSpeleoError(e, i18n.t('speleodb.projectSettings.loadFailed')));
    } finally {
      if (current()) setLoading(false);
    }
  }, [isLinked]);

  useEffect(() => {
    void reload();
    return () => {
      loadSeq.current += 1;
    };
  }, [reload]);

  const update = useCallback(
    <K extends keyof SpeleoMetadataForm>(key: K, value: SpeleoMetadataForm[K]) => {
      setMetadata((prev) =>
        prev.form ? { ...prev, form: { ...prev.form, [key]: value } } : prev,
      );
    },
    [],
  );

  // Both-or-neither coordinate validity: OK when both blank or both filled with a
  // finite number. A single filled field is the only invalid state.
  const latTrim = form?.latitude.trim() ?? '';
  const lonTrim = form?.longitude.trim() ?? '';
  const latNum = latTrim === '' ? null : Number(latTrim);
  const lonNum = lonTrim === '' ? null : Number(lonTrim);
  const coordsPairValid = (latTrim === '') === (lonTrim === '');
  // When both are set they must be finite AND in range - reuse the shared validator
  // (finiteness alone would let an out-of-range latitude reach the server).
  const bothProvided = latTrim !== '' && lonTrim !== '';
  const coordsRangeValid =
    !bothProvided || (latNum !== null && lonNum !== null && validateCoordinates(latNum, lonNum).valid);
  const coordsValid = coordsPairValid && coordsRangeValid;

  // Which fields differ from the loaded baseline (drives the minimal PATCH + Save gate).
  const nameChanged = !!baseline && !!form && form.name !== baseline.name;
  const descriptionChanged = !!baseline && !!form && form.description !== baseline.description;
  const countryChanged = !!baseline && !!form && form.country !== baseline.country;
  const visibilityChanged = !!baseline && !!form && form.visibility !== baseline.visibility;
  const baselineLat = baseline?.latitude ?? null;
  const baselineLon = baseline?.longitude ?? null;
  const coordinatesChanged =
    !!baseline && !!form && coordsValid && (latNum !== baselineLat || lonNum !== baselineLon);

  const anyChanged =
    nameChanged || descriptionChanged || countryChanged || visibilityChanged || coordinatesChanged;
  const nameEmpty = (form?.name.trim() ?? '') === '';
  const canSave = canEdit && !!form && anyChanged && coordsValid && !nameEmpty && !saving;

  const handleSave = useCallback(async () => {
    if (!form || !baseline || !canSave) return;
    setSaving(true);
    const patch: SpeleoProjectMetadataPatch = {
      name: nameChanged ? form.name.trim() : null,
      description: descriptionChanged ? form.description : null,
      country: countryChanged ? form.country : null,
      visibility: visibilityChanged ? form.visibility : null,
      coordinates_edited: coordinatesChanged,
      latitude: coordinatesChanged ? latNum : null,
      longitude: coordinatesChanged ? lonNum : null,
    };
    try {
      // [#576 RVF-c-C1] Names the project: refused once another is open.
      const updated = await invokeIpc('speleodb_patch_project', {
        patch,
        expectedProject: expectedOpenProject(),
      });
      setMetadata({ baseline: updated, form: toForm(updated) });
      // The backend folds a name change into the local .stalx; mirror it in the UI.
      // A lightweight in-memory set (not set_project_name_for_path) avoids marking the
      // project dirty for a rename that is already persisted server- and file-side.
      if (nameChanged) {
        const project = useProjectStore.getState().project;
        if (project) {
          useProjectStore.setState({ project: { ...project, name: updated.name } });
        }
      }
      // Re-seed the link so any server-side follow-on (e.g. permission) stays fresh.
      void refreshLinkStatus();
      showToast(t('speleodb.projectSettings.saved'), TOAST_KEYS.SPELEODB_PROJECT_SETTINGS, 'success');
    } catch (e) {
      // [#576 Q-W44-7-D1] Refused because the project changed: the short line instead.
      if (!toastProjectChanged(e)) {
        showToast(
          humanizeSpeleoError(e, t('speleodb.projectSettings.saveFailed')),
          TOAST_KEYS.SPELEODB_PROJECT_SETTINGS,
          'error',
        );
      }
    } finally {
      setSaving(false);
    }
  }, [
    form,
    baseline,
    canSave,
    nameChanged,
    descriptionChanged,
    countryChanged,
    visibilityChanged,
    coordinatesChanged,
    latNum,
    lonNum,
    refreshLinkStatus,
    showToast,
    t,
  ]);

  const handleEnableRendering = useCallback(async () => {
    setEnablingRendering(true);
    try {
      // Reuse the shared store action (single source of truth for the IPC + the
      // rendering-disabled banner drop + link re-seed), then mirror the flag into this
      // hook's local baseline and toast. It throws on failure.
      await useSpeleoStore.getState().enableRendering();
      setMetadata((prev) =>
        prev.baseline ? { ...prev, baseline: { ...prev.baseline, exclude_geojson: false } } : prev,
      );
      showToast(t('speleodb.projectSettings.rendering.enabled'), TOAST_KEYS.SPELEODB_PROJECT_SETTINGS, 'success');
    } catch (e) {
      // [#576 Q-W44-7-D1] The web-rendering switch is a background verb: a refusal
      // because the project changed is dropped, nothing shown.
      if (isProjectChangedRefusal(e)) return;
      showToast(
        humanizeSpeleoError(e, t('speleodb.banner.enableRenderingFailed')),
        TOAST_KEYS.SPELEODB_ENABLE_RENDERING_ERROR,
        'error',
      );
    } finally {
      setEnablingRendering(false);
    }
  }, [showToast, t]);

  return {
    loading,
    loadError,
    reload,
    baseline,
    form,
    update,
    coordsValid,
    canEdit,
    canSave,
    saving,
    handleSave,
    enablingRendering,
    handleEnableRendering,
  };
}
