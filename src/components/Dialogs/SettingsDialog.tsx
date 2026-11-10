import { finishSpeleoSettings } from '../../utils/finishSpeleoSettings';
import { useState, useEffect, useCallback, useRef, useMemo, useId } from 'react';
import { useTranslation } from 'react-i18next';
import type { TFunction } from 'i18next';
import { useUIStore } from '../../store/uiStore';
import { useProjectStore } from '../../store/projectStore';
import { Button } from '../common/Button';
import { DialogWrapper, type FixedSize } from './DialogWrapper';
import { AlertDialog } from './AlertDialog';
import {
  SettingsSidebar,
  CATEGORIES,
  type SettingsCategory,
} from './SettingsSidebar';
import { GeneralSettings } from './settings/GeneralSettings';
import { UnitsSettings } from './settings/UnitsSettings';
import { GasSettings } from './settings/GasSettings';
import { DisplaySettings } from './settings/DisplaySettings';
import { MapSettings } from './settings/MapSettings';
import { ValidationSettings } from './settings/ValidationSettings';
import { ExportSettings } from './settings/ExportSettings';
import { ShortcutsSettings } from './settings/ShortcutsSettings';
import { CacheSettings } from './settings/CacheSettings';
import { AdvancedSettings } from './settings/AdvancedSettings';
import { AppearanceSettings } from './settings/AppearanceSettings';
import { SpeleoDbSettings } from './settings/SpeleoDbSettings';
import type { Config } from '../../types/config';
import {
  defaultConfig,
  getThemedDisplayDefaults,
  getThemedSectionColorDefaults,
} from '../../types/config';
import type { ResolvedTheme } from '../../store/uiStore';
import { trackDialogComplete } from '../../utils/telemetry';
import { isValidationConfigValid } from '../../utils/validation';
import { useSpeleoStore } from '../../store/speleoStore';
import { AboutMapImagery } from './AboutMapImagery';
import { runAppUpdateCheck } from '../../hooks/useAppUpdate';
import { buildDefaultUpdateTrack } from '../../utils/updateTrack';

/**
 * Deep comparison of two config objects to detect changed field paths.
 */
function getChangedFields(originalConfig: Config, currentConfig: Config): Set<string> {
  const changed = new Set<string>();

  function compare(original: unknown, current: unknown, path: string) {
    if (original === current) return;

    if (
      typeof original !== 'object' ||
      typeof current !== 'object' ||
      original === null ||
      current === null
    ) {
      changed.add(path);
      return;
    }

    if (Array.isArray(original) && Array.isArray(current)) {
      if (JSON.stringify(original) !== JSON.stringify(current)) {
        changed.add(path);
      }
      return;
    }

    const origObj = original as Record<string, unknown>;
    const currObj = current as Record<string, unknown>;
    const allKeys = new Set([...Object.keys(origObj), ...Object.keys(currObj)]);

    for (const key of allKeys) {
      compare(origObj[key], currObj[key], path ? `${path}.${key}` : key);
    }
  }

  compare(originalConfig, currentConfig, '');
  return changed;
}

/**
 * Get category-specific defaults from defaultConfig. Exported for its unit test.
 *
 * `current` supplies the fields a category reset must PRESERVE: the DPV rig lives on `gas` in
 * the data model but is planned in Plan mode's DPV tab (#317), not on the Gas settings
 * page - so resetting that page must not silently destroy equipment it no longer displays
 * (the working plan store would keep the rig, and the engine would keep planning a scootered
 * dive over a config that silently lost it).
 */
export function getCategoryDefaults(
  category: SettingsCategory,
  resolvedTheme: ResolvedTheme = 'dark',
  current?: Config
): Partial<Config> {
  switch (category) {
    case 'general':
      // `update_track` overridden, not taken from the snapshot: its default is
      // a property of the BUILD. See `buildDefaultUpdateTrack`.
      return {
        general: { ...defaultConfig.general, update_track: buildDefaultUpdateTrack() },
      };
    case 'units':
      return { units: { ...defaultConfig.units } };
    case 'gas':
      return { gas: { ...defaultConfig.gas, dpv: current ? current.gas.dpv : defaultConfig.gas.dpv } };
    case 'display':
      return {
        display: getThemedDisplayDefaults(resolvedTheme),
        section_colors: getThemedSectionColorDefaults(resolvedTheme),
      };
    case 'map':
      return {
        map: {
          ...defaultConfig.map,
          satellite_gps_origin: { ...defaultConfig.map.satellite_gps_origin },
          controls: { ...defaultConfig.map.controls },
          station: { ...defaultConfig.map.station },
          shot: { ...defaultConfig.map.shot },
          snap: { ...defaultConfig.map.snap },
          animation: { ...defaultConfig.map.animation },
        },
      };
    case 'validation':
      return { validation: { ...defaultConfig.validation } };
    case 'export':
      return { export: { ...defaultConfig.export } };
    case 'shortcuts':
      return { shortcuts: { ...defaultConfig.shortcuts } };
    case 'cache':
      return { cache: { ...defaultConfig.cache } };
    case 'speleodb':
      // Only the staged connection fields reset here; the live account session
      // (login/logout) is not part of the config and is unaffected by a reset.
      return { speleodb: { ...defaultConfig.speleodb } };
    case 'appearance':
      // Highlighting controls live in the Appearance tab, so resetting Appearance
      // also resets syntactic highlighting.
      return {
        appearance: { ...defaultConfig.appearance },
        syntactic_highlighting: {
          ...defaultConfig.syntactic_highlighting,
          overrides: {},
        },
      };
    case 'advanced':
      return {
        debug: { map: { ...defaultConfig.debug.map }, memory_probe: defaultConfig.debug.memory_probe },
        logging: { ...defaultConfig.logging },
        closure_adjustment: { ...defaultConfig.closure_adjustment },
        // ONLY the three panel sizes this tab actually shows. Same reasoning as
        // the DPV rig above: resetting the whole `panels` block destroyed the
        // data-table view state - column order, hidden columns, the two width
        // maps and the angle modes - none of which this page displays or reports
        // in its change summary (#357 item 15).
        //
        // It was also incoherent rather than merely wide. The open DataTable
        // never noticed (its seed effect is one-shot), so it kept rendering the
        // old layout while the defaults sat on disk, and the next unrelated view
        // change posted the whole in-memory map straight back - so the reset read
        // as a no-op until restart, or was actively undone. Pre-existing for
        // order/hidden columns; #334 enrolled the width maps in it.
        panels: current
          ? {
              ...current.panels,
              sections_panel_width: defaultConfig.panels.sections_panel_width,
              detail_panel_width: defaultConfig.panels.detail_panel_width,
              data_table_height: defaultConfig.panels.data_table_height,
            }
          : { ...defaultConfig.panels },
      };
    default:
      return {};
  }
}

/**
 * Get the label for a settings category.
 */
function getCategoryLabel(category: SettingsCategory, t: TFunction): string {
  const cat = CATEGORIES.find((c) => c.id === category);
  return cat ? t(cat.labelKey) : category;
}

/**
 * Centralized validation for all categories.
 * Returns a map of category -> hasError.
 * Defensive: guards against undefined nested props.
 */
function validateAllCategories(config: Config): Partial<Record<SettingsCategory, boolean>> {
  const errors: Partial<Record<SettingsCategory, boolean>> = {};

  try {
    // Map validation - with defensive access
    const map = config?.map;
    const mapHasError = !!(
      map &&
      (map.zoom_min >= map.zoom_max ||
        map.zoom_default < map.zoom_min ||
        map.zoom_default > map.zoom_max ||
        map.zoom_step < 0.1 ||
        map.zoom_step > 5.0 ||
        (map.satellite_gps_origin?.latitude != null &&
          (map.satellite_gps_origin.latitude < -90 || map.satellite_gps_origin.latitude > 90)) ||
        (map.satellite_gps_origin?.longitude != null &&
          (map.satellite_gps_origin.longitude < -180 || map.satellite_gps_origin.longitude > 180)) ||
        (map.station?.radius != null && (map.station.radius < 1 || map.station.radius > 50)) ||
        (map.station?.hit_area != null &&
          map.station?.radius != null &&
          map.station.hit_area < map.station.radius) ||
        (map.shot?.width != null && (map.shot.width < 0.5 || map.shot.width > 20)) ||
        (map.shot?.hit_area != null && map.shot?.width != null && map.shot.hit_area < map.shot.width) ||
        (map.snap?.threshold != null && (map.snap.threshold < 1 || map.snap.threshold > 50)) ||
        (map.animation?.duration != null && (map.animation.duration < 0 || map.animation.duration > 2000)))
    );
    errors.map = mapHasError;

    // Validation category (shared rules, see utils/validation).
    errors.validation = config?.validation ? !isValidationConfigValid(config.validation) : false;

    // Export category
    const exp = config?.export;
    const exportHasError = !!(exp && (exp.default_dpi < 72 || exp.default_dpi > 1200));
    errors.export = exportHasError;

    // Only set errors for categories we actually validate
    // Don't set false for categories without centralized validation
    // to avoid masking component-reported errors
  } catch {
    // If any error occurs, return empty to not mask component errors
    return {};
  }

  return errors;
}

/**
 * Deep merge two config objects.
 */
function mergeConfig(base: Config, updates: Partial<Config>): Config {
  // Spread to create a mutable copy
  const result: Config = {
    general: { ...base.general },
    units: { ...base.units },
    display: { ...base.display },
    section_colors: { palette: [...base.section_colors.palette] },
    map: {
      ...base.map,
      satellite_gps_origin: { ...base.map.satellite_gps_origin },
      controls: { ...base.map.controls },
      station: { ...base.map.station },
      shot: { ...base.map.shot },
      snap: { ...base.map.snap },
      animation: { ...base.map.animation },
    },
    validation: { ...base.validation },
    closure_adjustment: { ...base.closure_adjustment },
    export: { ...base.export },
    logging: { ...base.logging },
    shortcuts: { ...base.shortcuts },
    panels: { ...base.panels },
    cache: { ...base.cache },
    debug: { map: { ...base.debug.map }, memory_probe: base.debug.memory_probe },
    global_view: { ...base.global_view },
    appearance: { ...base.appearance },
    import: { ...base.import },
    placement_suggestion: { ...(base.placement_suggestion ?? defaultConfig.placement_suggestion) },
    syntactic_highlighting: {
      ...(base.syntactic_highlighting ?? defaultConfig.syntactic_highlighting),
      overrides: { ...(base.syntactic_highlighting ?? defaultConfig.syntactic_highlighting).overrides },
    },
    gas: { ...(base.gas ?? defaultConfig.gas) },
    speleodb: { ...(base.speleodb ?? defaultConfig.speleodb) },
    // Not a Settings field (the map legend cog and the export panel own it via
    // `update_map_palette`); carried through so a Settings save never drops it.
    map_palette: base.map_palette ?? defaultConfig.map_palette,
  };

  // Apply updates with deep merge for each key
  if (updates.general) {
    result.general = { ...result.general, ...updates.general };
  }
  if (updates.units) {
    result.units = { ...result.units, ...updates.units };
  }
  if (updates.display) {
    result.display = { ...result.display, ...updates.display };
  }
  if (updates.section_colors) {
    result.section_colors = { ...result.section_colors, ...updates.section_colors };
  }
  if (updates.map) {
    result.map = {
      ...result.map,
      ...updates.map,
      satellite_gps_origin: updates.map.satellite_gps_origin
        ? { ...result.map.satellite_gps_origin, ...updates.map.satellite_gps_origin }
        : result.map.satellite_gps_origin,
      controls: updates.map.controls
        ? { ...result.map.controls, ...updates.map.controls }
        : result.map.controls,
      station: updates.map.station
        ? { ...result.map.station, ...updates.map.station }
        : result.map.station,
      shot: updates.map.shot
        ? { ...result.map.shot, ...updates.map.shot }
        : result.map.shot,
      snap: updates.map.snap
        ? { ...result.map.snap, ...updates.map.snap }
        : result.map.snap,
      animation: updates.map.animation
        ? { ...result.map.animation, ...updates.map.animation }
        : result.map.animation,
    };
  }
  if (updates.validation) {
    result.validation = { ...result.validation, ...updates.validation };
  }
  if (updates.closure_adjustment) {
    result.closure_adjustment = { ...result.closure_adjustment, ...updates.closure_adjustment };
  }
  if (updates.export) {
    result.export = { ...result.export, ...updates.export };
  }
  if (updates.logging) {
    result.logging = { ...result.logging, ...updates.logging };
  }
  if (updates.shortcuts) {
    result.shortcuts = { ...result.shortcuts, ...updates.shortcuts };
  }
  if (updates.panels) {
    result.panels = { ...result.panels, ...updates.panels };
  }
  if (updates.cache) {
    result.cache = { ...result.cache, ...updates.cache };
  }
  if (updates.debug) {
    result.debug = {
      ...result.debug,
      map: updates.debug.map
        ? { ...result.debug.map, ...updates.debug.map }
        : result.debug.map,
    };
  }
  if (updates.global_view) {
    result.global_view = { ...result.global_view, ...updates.global_view };
  }
  if (updates.appearance) {
    result.appearance = { ...result.appearance, ...updates.appearance };
  }
  if (updates.placement_suggestion) {
    result.placement_suggestion = { ...result.placement_suggestion, ...updates.placement_suggestion };
  }
  if (updates.syntactic_highlighting) {
    result.syntactic_highlighting = { ...result.syntactic_highlighting, ...updates.syntactic_highlighting };
  }
  if (updates.gas) {
    result.gas = { ...result.gas, ...updates.gas };
  }
  if (updates.speleodb) {
    result.speleodb = { ...result.speleodb, ...updates.speleodb };
  }

  return result;
}

/** Account actions own these fields even while unrelated preferences are staged. */
function rebaseSpeleoOwnedFields(current: Config, saved: Config, previous: Config): Config {
  const session = useSpeleoStore.getState().session;
  return {
    ...current,
    general: {
      ...current.general,
      ...(JSON.stringify(saved.general.recent_projects) !== JSON.stringify(previous.general.recent_projects)
        ? { recent_projects: saved.general.recent_projects }
        : {}),
    },
    speleodb: {
      ...current.speleodb,
      email: saved.speleodb.email,
      ...(saved.speleodb.endpoint !== previous.speleodb.endpoint &&
          (current.speleodb.endpoint === previous.speleodb.endpoint || session?.loggedIn)
        ? { endpoint: saved.speleodb.endpoint }
        : {}),
    },
  };
}

export default function SettingsDialog() {
  const { t } = useTranslation();
  const closeDialog = finishSpeleoSettings;
  const resolvedTheme = useUIStore((s) => s.resolvedTheme);
  const setThemePreview = useUIStore((s) => s.setThemePreview);
  const setSyntacticHighlightPreview = useUIStore((s) => s.setSyntacticHighlightPreview);
  const { config, updateConfig } = useProjectStore();

  // Credential drafts are transient and are never applied by Settings Save.
  const clearPendingAuth = useSpeleoStore((s) => s.clearPendingAuth);

  // Panel ID base for ARIA associations
  const panelIdBase = useId();

  // Active (selected) category - initialCategory from dialogData (e.g. from "Customize shortcuts" link)
  const dialogData = useUIStore((s) => s.dialogData);
  const [activeCategory, setActiveCategory] = useState<SettingsCategory>(
    (dialogData?.initialCategory as SettingsCategory) || 'general'
  );
  // Focused category index for roving tabindex
  const [focusedIndex, setFocusedIndex] = useState(() =>
    Math.max(0, CATEGORIES.findIndex((category) => category.id === activeCategory))
  );

  // Local config copy for editing
  const [localConfig, setLocalConfig] = useState<Config | null>(config);
  const localConfigRef = useRef(localConfig);
  localConfigRef.current = localConfig;
  const backendConfigRef = useRef(config);
  const draftRevision = useRef(0);
  // Reference to original config for change detection
  const originalConfigRef = useRef<Config | null>(config);

  // Category error states
  const [categoryErrors, setCategoryErrors] = useState<Partial<Record<SettingsCategory, boolean>>>({});

  // Fixed size for settings dialog - constant from first render through all category switches
  const settingsFixedSize: FixedSize = useMemo(() => ({
    width: 'min(800px, 95vw)',
    height: 'clamp(500px, 80vh, 800px)',
  }), []);

  // Ref for scrollable content area - used to reset scroll on category change
  const contentRef = useRef<HTMLDivElement>(null);

  // Custom unsaved changes dialog state
  const [connectionBusy, setConnectionBusy] = useState(false);
  const savingSettings = useRef(false);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [showUnsavedDialog, setShowUnsavedDialog] = useState(false);
  // Reset confirmation dialog state
  const [showResetConfirm, setShowResetConfirm] = useState(false);
  // About dialog state
  const [showAbout, setShowAbout] = useState(false);
  // Manual "Check for updates" state (#579) - inline result text, no toast.
  //
  // The check itself goes through `runAppUpdateCheck`, the SAME path the launch
  // hook uses, so it drives the update store as well. Keeping a private result
  // here made the two disagree in both directions: this text promised "see the
  // update banner" for a banner the store never learned to show, and the
  // backend check (which discards any parked download) ran underneath a banner
  // still offering "Restart to update" - a restart that could then only fail.
  // What stays local is only the presentation: which sentence to show, and the
  // version pair to show in it.
  const [checkUpdatesState, setCheckUpdatesState] = useState<
    'idle' | 'checking' | 'up_to_date' | 'available' | 'failed'
  >('idle');
  const [checkUpdatesVersions, setCheckUpdatesVersions] = useState<{
    current: string;
    available: string | null;
  } | null>(null);
  const handleCheckUpdates = useCallback(async () => {
    const previous = checkUpdatesState;
    setCheckUpdatesState('checking');
    const outcome = await runAppUpdateCheck({ userInitiated: true });
    // A newer check (a track switch, a second press) already owns the store, so
    // this reply was never applied to it. Rendering it anyway would put a line
    // on screen that contradicts the banner - keep whatever was there before.
    if (outcome.kind === 'stale') {
      setCheckUpdatesState(previous === 'checking' ? 'idle' : previous);
      return;
    }
    if (outcome.kind === 'failed') {
      setCheckUpdatesState('failed');
      return;
    }
    setCheckUpdatesVersions({
      current: outcome.currentVersion,
      available: outcome.kind === 'available' ? outcome.version : null,
    });
    setCheckUpdatesState(outcome.kind);
  }, [checkUpdatesState]);

  // Initialize local config from store
  useEffect(() => {
    if (config && !localConfig) {
      setLocalConfig(config);
      originalConfigRef.current = config;
    }
  }, [config, localConfig]);

  useEffect(() => {
    const previous = backendConfigRef.current;
    backendConfigRef.current = config;
    if (!config || !previous || config === previous) return;
    if (originalConfigRef.current) {
      originalConfigRef.current = rebaseSpeleoOwnedFields(originalConfigRef.current, config, previous);
    }
    if (localConfigRef.current) {
      const rebased = rebaseSpeleoOwnedFields(localConfigRef.current, config, previous);
      localConfigRef.current = rebased;
      setLocalConfig(rebased);
    }
  }, [config]);

  // Live-preview theme changes: apply immediately on selection, revert on unmount/cancel
  useEffect(() => {
    if (localConfig) {
      setThemePreview(localConfig.appearance);
    }
    return () => setThemePreview(null);
  }, [localConfig?.appearance.theme, localConfig?.appearance.day_color_theme, localConfig?.appearance.night_color_theme, setThemePreview]);

  // Live-preview syntactic-highlighting changes (level / preset / per-color overrides)
  useEffect(() => {
    if (localConfig) {
      setSyntacticHighlightPreview(localConfig.syntactic_highlighting);
    }
    return () => setSyntacticHighlightPreview(null);
  }, [
    localConfig?.syntactic_highlighting.level,
    localConfig?.syntactic_highlighting.preset,
    localConfig?.syntactic_highlighting.overrides,
    setSyntacticHighlightPreview,
  ]);

  // Compute changed fields from original config
  const changedFields = useMemo(() => {
    if (!localConfig || !originalConfigRef.current) return new Set<string>();
    return getChangedFields(originalConfigRef.current, localConfig);
  }, [localConfig]);

  // Derive isDirty from changedFields
  const isDirty = changedFields.size > 0;

  // Handle config changes from category components
  const handleConfigChange = useCallback((updates: Partial<Config>) => {
    draftRevision.current += 1;
    setLocalConfig((prev) => {
      if (!prev) return prev;
      const next = mergeConfig(prev, updates);
      localConfigRef.current = next;
      return next;
    });
  }, []);

  // Handle validation error reports from categories
  const handleValidationError = useCallback((category: SettingsCategory, hasError: boolean) => {
    setCategoryErrors((prev) => {
      if (prev[category] === hasError) return prev;
      return { ...prev, [category]: hasError };
    });
  }, []);

  // Centralized validation for all categories - used for sidebar error dots
  const centralizedErrors = useMemo(() => {
    if (!localConfig) return {} as Partial<Record<SettingsCategory, boolean>>;
    return validateAllCategories(localConfig);
  }, [localConfig]);

  // Combined errors for sidebar display (centralized validation takes precedence)
  const combinedCategoryErrors = useMemo(() => {
    return { ...categoryErrors, ...centralizedErrors };
  }, [categoryErrors, centralizedErrors]);

  // Check if any category has errors
  const hasAnyError = useMemo(() => {
    return Object.values(combinedCategoryErrors).some((hasError) => hasError);
  }, [combinedCategoryErrors]);

  // Handle category change from sidebar
  const handleCategoryChange = useCallback((category: SettingsCategory) => {
    if (connectionBusy || savingSettings.current) return;
    setActiveCategory(category);
    // Also update focused index to match
    const index = CATEGORIES.findIndex((c) => c.id === category);
    if (index >= 0) {
      setFocusedIndex(index);
    }
    // Reset scroll position to top on category switch
    if (contentRef.current) {
      contentRef.current.scrollTop = 0;
    }
  }, [connectionBusy]);

  // Handle focus change from sidebar
  const handleFocusChange = useCallback((index: number) => {
    setFocusedIndex(index);
  }, []);

  // Never close over a replacement dialog or edits made while persistence waited.
  const commitSettings = useCallback(async (): Promise<boolean> => {
    const draft = localConfigRef.current;
    if (!draft || savingSettings.current) return false;
    const issuedRevision = draftRevision.current;
    const issuedDialog = useUIStore.getState().dialogData;
    const issuedGeneration = useSpeleoStore.getState().sessionGeneration;
    const ownsDialog = () => {
      const current = useUIStore.getState();
      return current.activeDialog === 'settings' && current.dialogData === issuedDialog &&
        useSpeleoStore.getState().sessionGeneration === issuedGeneration;
    };
    savingSettings.current = true;
    try {
      if (isDirty) {
        const saved = useProjectStore.getState().config;
        const previous = backendConfigRef.current;
        const next = saved && previous ? rebaseSpeleoOwnedFields(draft, saved, previous) : draft;
        const result = await updateConfig(next);
        if (!result.success) {
          if (ownsDialog()) setSaveError(result.error ?? t('settings.saveErrorTitle'));
          return false;
        }
      }
      return ownsDialog() && draftRevision.current === issuedRevision;
    } finally {
      savingSettings.current = false;
    }
  }, [isDirty, updateConfig, t]);

  // Save settings
  const handleSave = useCallback(async () => {
    if (!localConfig || !isDirty || hasAnyError) return;
    if (await commitSettings()) {
      trackDialogComplete('settings');
      closeDialog();
    }
  }, [localConfig, isDirty, hasAnyError, commitSettings, closeDialog]);

  // Request close - show custom dialog if dirty
  // Short-circuit if an overlay is already open to prevent Escape conflicts
  const handleRequestClose = useCallback(() => {
    if (connectionBusy) return;
    // If an overlay is open, close it instead of the main dialog
    if (showAbout) {
      setShowAbout(false);
      return;
    }
    if (showResetConfirm) {
      setShowResetConfirm(false);
      return;
    }
    if (showUnsavedDialog) {
      setShowUnsavedDialog(false);
      return;
    }
    if (activeCategory === 'speleodb') {
      if (hasAnyError) {
        const first = CATEGORIES.find((category) => combinedCategoryErrors[category.id]);
        if (first) setActiveCategory(first.id);
        return;
      }
      const issued = useUIStore.getState().dialogData;
      void commitSettings().then((ok) => {
        const current = useUIStore.getState();
        if (ok && current.activeDialog === 'settings' && current.dialogData === issued) closeDialog();
      });
      return;
    }
    // Other categories retain the explicit discard prompt.
    if (isDirty) {
      setShowUnsavedDialog(true);
    } else {
      trackDialogComplete('settings');
      closeDialog();
    }
  }, [connectionBusy, activeCategory, hasAnyError, combinedCategoryErrors, commitSettings, isDirty, closeDialog, showAbout, showResetConfirm, showUnsavedDialog]);

  // Handle custom unsaved dialog - Discard
  const handleDiscardAndClose = useCallback(() => {
    // Drop any staged login/logout along with the config edits.
    clearPendingAuth();
    setShowUnsavedDialog(false);
    trackDialogComplete('settings');
    closeDialog();
  }, [clearPendingAuth, closeDialog]);

  // Handle custom unsaved dialog - Cancel (stay in dialog)
  const handleCancelClose = useCallback(() => {
    setShowUnsavedDialog(false);
  }, []);

  // Reset to defaults for current category
  const handleResetToDefaults = useCallback(() => {
    if (connectionBusy || savingSettings.current) return;
    setShowResetConfirm(true);
  }, [connectionBusy]);

  // Confirm reset to defaults
  const handleConfirmReset = useCallback(() => {
    if (!localConfig) return;

    const defaults = getCategoryDefaults(activeCategory, resolvedTheme, localConfig);
    draftRevision.current += 1;
    const next = mergeConfig(localConfig, defaults);
    localConfigRef.current = next;
    setLocalConfig(next);
    setShowResetConfirm(false);
  }, [localConfig, activeCategory, resolvedTheme]);

  // Cancel reset
  const handleCancelReset = useCallback(() => {
    setShowResetConfirm(false);
  }, []);

  // Show loading state if config hasn't loaded yet
  if (!localConfig) {
    return (
      <DialogWrapper
        title={t('dialogs.settings.title')}
        onClose={closeDialog}
        size="4xl"
        closeOnBackdropClick={false}
        fixedSize={settingsFixedSize}
        overflow="hidden"
        bodyClassName="flex-1 flex items-center justify-center p-4"
        data-testid="settings-dialog"
      >
        <p className="text-text-secondary">{t('settings.loading')}</p>
      </DialogWrapper>
    );
  }

  // Render the active category content
  const renderCategoryContent = () => {
    const commonProps = {
      config: localConfig,
      onChange: handleConfigChange,
      changedFields,
      onValidationError: handleValidationError,
      panelIdBase,
    };

    switch (activeCategory) {
      case 'general':
        return <GeneralSettings {...commonProps} />;
      case 'units':
        return <UnitsSettings {...commonProps} />;
      case 'gas':
        return <GasSettings {...commonProps} />;
      case 'display':
        return <DisplaySettings {...commonProps} />;
      case 'map':
        return <MapSettings {...commonProps} />;
      case 'validation':
        return <ValidationSettings {...commonProps} />;
      case 'export':
        return <ExportSettings {...commonProps} />;
      case 'shortcuts':
        return <ShortcutsSettings {...commonProps} />;
      case 'cache':
        return <CacheSettings {...commonProps} />;
      case 'speleodb':
        return <SpeleoDbSettings {...commonProps} onConnectionBusyChange={setConnectionBusy} saveBeforeConnect={async () => { if (hasAnyError) { const first = CATEGORIES.find((category) => combinedCategoryErrors[category.id]); if (first) setActiveCategory(first.id); return false; } return commitSettings(); }} />;
      case 'appearance':
        return <AppearanceSettings {...commonProps} />;
      case 'advanced':
        return <AdvancedSettings {...commonProps} />;
      default:
        return null;
    }
  };

  return (
    <>
      <DialogWrapper
        title={t('dialogs.settings.title')}
        onClose={handleRequestClose}
        isDirty={false} // We handle dirty state with custom dialog
        size="4xl"
        closeOnBackdropClick={false}
        maxHeight="max-h-[90vh]"
        overflow="hidden"
        fixedSize={settingsFixedSize}
        bodyClassName="flex-1 flex flex-col overflow-hidden"
        data-testid="settings-dialog"
      >
        {({ requestClose }) => (
          <div className="flex flex-col min-w-[min(500px,100%)] flex-1 min-h-0">
            {/* Main content area with sidebar and content */}
            <div className="flex flex-1 overflow-hidden">
              {/* Sidebar */}
              <SettingsSidebar
                activeCategory={activeCategory}
                onCategoryChange={handleCategoryChange}
                focusedIndex={focusedIndex}
                onFocusChange={handleFocusChange}
                categoryErrors={combinedCategoryErrors}
                panelIdBase={panelIdBase}
              />

              {/* Content area */}
              <div className="flex-1 flex flex-col overflow-hidden">
                {/* Content header with Reset to Defaults */}
                <div className="flex items-center justify-between px-4 py-3 border-b border-border bg-panel-50">
                  <span className="text-sm text-text-secondary">
                    {t('settings.categoryHeader', { category: getCategoryLabel(activeCategory, t) })}
                  </span>
                  <Button
                    variant="secondary"
                    onClick={handleResetToDefaults}
                    disabled={connectionBusy || (activeCategory === 'speleodb' && useSpeleoStore.getState().session?.loggedIn === true)}
                    data-testid="reset-category-defaults"
                  >
                    {t('settings.resetToDefaults')}
                  </Button>
                </div>

                {/* Scrollable content - tabpanel semantics owned by category components */}
                <div ref={contentRef} className="flex-1 overflow-auto min-h-0 p-4">
                  {renderCategoryContent()}
                </div>
              </div>
            </div>

            {/* Footer with About on left, Save/Cancel on right */}
            <div className="flex justify-between gap-2 px-4 py-3 border-t border-border bg-panel">
              <Button
                variant="secondary"
                onClick={() => {
                  setCheckUpdatesState('idle');
                  setShowAbout(true);
                }}
                data-testid="settings-about-button"
              >
                {t('dialogs.about.title')}
              </Button>
              <div className="flex gap-2">
                {activeCategory === 'speleodb' ? <Button variant="primary" onClick={requestClose} disabled={connectionBusy} data-testid="settings-close-button">{t('common.actions.close')}</Button> : <>
                <Button onClick={requestClose} data-testid="settings-cancel-button">
                  {t('common.actions.cancel')}
                </Button>
                <Button
                  variant="primary"
                  onClick={handleSave}
                  disabled={!isDirty || hasAnyError}
                  data-testid="settings-save-button"
                >
                  {t('common.actions.save')}
                </Button>
                </>}
              </div>
            </div>
          </div>
        )}
      </DialogWrapper>

      <AlertDialog open={saveError !== null} title={t('settings.saveErrorTitle')} onClose={() => setSaveError(null)} closeOnBackdropClick={false} data-testid="settings-save-error" actions={<Button variant="primary" onClick={() => setSaveError(null)} data-testid="settings-save-error-close">{t('common.actions.close')}</Button>}><p className="text-error">{saveError}</p></AlertDialog>
      <AlertDialog
        open={showUnsavedDialog}
        title={t('dialogs.unsaved.title')}
        onClose={handleCancelClose}
        data-testid="unsaved-changes-dialog"
        actions={
          <>
            <Button variant="danger" onClick={handleCancelClose} data-testid="unsaved-cancel">
              {t('common.actions.cancel')}
            </Button>
            <Button
              variant="primary"
              onClick={handleDiscardAndClose}
              data-testid="unsaved-discard"
            >
              {t('dialogs.unsaved.discard')}
            </Button>

          </>
        }
      >
        <>
          <p>{t('dialogs.unsaved.message')}</p>
          <p className="text-sm text-text-secondary">
            {t('dialogs.unsaved.note')}
          </p>
        </>
      </AlertDialog>

      <AlertDialog
        open={showResetConfirm}
        title={t('dialogs.resetDefaults.title')}
        onClose={handleCancelReset}
        data-testid="reset-confirm-dialog"
        actions={
          <>
            <Button onClick={handleCancelReset} data-testid="reset-cancel">
              {t('common.actions.cancel')}
            </Button>
            <Button
              variant="danger"
              onClick={handleConfirmReset}
              data-testid="reset-confirm"
            >
              {t('common.actions.reset')}
            </Button>
          </>
        }
      >
        <>
          <p>
            {t('dialogs.resetDefaults.message', { category: getCategoryLabel(activeCategory, t) })}
          </p>
          <p className="text-sm text-text-secondary">
            {t('dialogs.resetDefaults.note', { action: t('common.actions.save') })}
          </p>
        </>
      </AlertDialog>

      <AlertDialog
        open={showAbout}
        title={t('dialogs.about.title')}
        onClose={() => setShowAbout(false)}
        data-testid="about-dialog"
        actions={
          <Button onClick={() => setShowAbout(false)} data-testid="about-close">
            {t('dialogs.about.close')}
          </Button>
        }
      >
        <div className="space-y-2">
          <p className="font-medium">{t('dialogs.about.version', { version: __APP_VERSION__ })}</p>
          {/* A released build's version is bare, so the commit is shown here:
              a bug report still has to identify which build it came from. */}
          <p className="text-xs text-text-secondary" data-testid="about-commit">
            {t('dialogs.about.build', { commit: __APP_COMMIT__ })}
          </p>
          {/* Only shown for a non-stable build - a stable release says nothing
              extra, matching the version line users already read. */}
          {__APP_CHANNEL__ !== 'stable' && (
            <p className="text-xs text-text-secondary" data-testid="about-channel">
              {t('dialogs.about.channel', { channel: __APP_CHANNEL__ })}
            </p>
          )}
          {/* The PERSISTED track, not the staged `localConfig` one. The check
              button below runs a backend command that reads the config from
              disk, so showing the staged value made the line disagree with the
              behaviour it sits next to: stage Beta, do not Save, press Check,
              and the label said Beta while the query went to Stable. A staged
              change is called out instead of silently displayed. */}
          <p className="text-xs text-text-secondary" data-testid="about-update-track">
            {t('dialogs.about.track', {
              track: t(`settings.general.updateTrack.options.${config?.general.update_track ?? 'stable'}`),
            })}
            {localConfig && config && localConfig.general.update_track !== config.general.update_track && (
              <span className="ml-1 text-warning" data-testid="about-update-track-pending">
                {t('dialogs.about.trackPending')}
              </span>
            )}
          </p>
          <p>
            <Button
              variant="secondary"
              onClick={handleCheckUpdates}
              disabled={checkUpdatesState === 'checking'}
              data-testid="about-check-updates"
            >
              {t('dialogs.about.checkUpdates')}
            </Button>
            {checkUpdatesState !== 'idle' && checkUpdatesState !== 'checking' && (
              <span
                className={`ml-2 text-sm ${checkUpdatesState === 'failed' ? 'text-warning' : 'text-text-secondary'}`}
                data-testid="about-check-updates-result"
              >
                {checkUpdatesState === 'up_to_date' &&
                  checkUpdatesVersions &&
                  t('dialogs.about.upToDate', { current: checkUpdatesVersions.current })}
                {checkUpdatesState === 'available' &&
                  checkUpdatesVersions?.available &&
                  t('dialogs.about.updateAvailable', {
                    current: checkUpdatesVersions.current,
                    version: checkUpdatesVersions.available,
                  })}
                {checkUpdatesState === 'failed' && t('dialogs.about.checkFailed')}
              </span>
            )}
          </p>
          <p className="text-sm text-text-secondary">{t('dialogs.about.description')}</p>
          <p>
            <a
              href="https://stalagmight.com"
              data-testid="about-website-link"
              target="_blank"
              rel="noopener noreferrer"
              className="text-sm text-accent hover:underline"
              onClick={(e) => {
                e.preventDefault();
                window.open('https://stalagmight.com', '_blank');
              }}
            >
              {t('dialogs.about.website')}
            </a>
          </p>
          <AboutMapImagery />
        </div>
      </AlertDialog>
    </>
  );
}
