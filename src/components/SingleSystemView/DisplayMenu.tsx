import { useState, useRef, useEffect, type ReactNode, type CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore, type DisplayToggleKey, type ColorMode } from '../../store/uiStore';
import {
  selectEffectiveColorMode,
  selectEffectiveShowShotErrors,
  selectIssueDisplayAuto,
} from '../../store/displaySelectors';
import { useProjectStore } from '../../store/projectStore';
import { useDismissableLayer } from '../../hooks/useDismissableLayer';
import { useShallow } from 'zustand/react/shallow';
import { GRID_SPACING_OPTIONS } from '../Map/GridOverlay';
import { Select } from '../common/Select';
import { formatNumber } from '../../i18n/format';
import {
  ChevronDownIcon,
  ChevronUpIcon,
  EyeIcon,
  MapIcon,
  LayersIcon,
  LockIcon,
  LockOpenIcon,
  MessageIcon,
  RulerIcon,
  SlidersIcon,
} from '../common/icons';
import {
  EntitySymbol,
  EntityJumpSymbol,
  EntityReferenceKindSymbol,
  EntityDatumSymbol,
  EntityCommentSymbol,
  EntityDataSymbol,
  EntityLabelSymbol,
} from '../common/toolIcons';
import {
  syntacticLevelAtLeast,
  syntacticVarName,
  type BlobEntity,
} from '../../theme/syntacticPalette';
import { getThemeColor } from '../../utils/themeColors';
import { getContrastColor } from '../../utils/colors';
import { setSource } from '../../utils/telemetry';
import { hasAdjustmentInputs } from '../../utils/closures';
import { hasStationNames } from '../../types/survey-data';
import { useOnboardingStep } from '../../hooks/useOnboardingStep';

/** A multi-toggle display key shown as a pill, optionally gated on another toggle. */
interface ToggleItem {
  key: Extract<
    DisplayToggleKey,
    | 'showStations'
    | 'showShots'
    | 'showReferenceShots'
    | 'showJumps'
    | 'showZeroDatum'
    | 'showSectionNames'
    | 'showStationLabels'
    | 'showStationNames'
    | 'showStationComments'
    | 'showShotComments'
    | 'showShotData'
    | 'showShotErrors'
    | 'showGrid'
    | 'showSatellite'
  >;
  labelKey: string;
  descriptionKey?: string;
  disabledWhen?: 'showShots';
  /** Offered only when the project has station names (see renderTogglePill). */
  needsStationNames?: true;
  /** Element this pill refers to: drives the entity icon + the pill's tint
   *  (--sh-<entity>, the same color the data-table/legend chips use). */
  entity?: BlobEntity;
  /** Badged entity-symbol variant overlaid on the entity glyph. */
  symbol?: 'reference' | 'jump' | 'datum' | 'comment' | 'data' | 'label';
}

const MAP_ELEMENTS: ToggleItem[] = [
  { key: 'showStations', labelKey: 'displayMenu.items.stations', descriptionKey: 'displayMenu.descriptions.stations', entity: 'station' },
  { key: 'showShots', labelKey: 'displayMenu.items.shots', descriptionKey: 'displayMenu.descriptions.shots', entity: 'shot' },
  { key: 'showJumps', labelKey: 'displayMenu.items.jumps', descriptionKey: 'displayMenu.descriptions.jumps', disabledWhen: 'showShots', entity: 'shot', symbol: 'jump' },
  { key: 'showReferenceShots', labelKey: 'displayMenu.items.referenceShots', descriptionKey: 'displayMenu.descriptions.referenceShots', disabledWhen: 'showShots', entity: 'shot', symbol: 'reference' },
  { key: 'showZeroDatum', labelKey: 'displayMenu.items.zeroDatum', descriptionKey: 'displayMenu.descriptions.zeroDatum', entity: 'station', symbol: 'datum' },
];

const OVERLAYS: ToggleItem[] = [
  { key: 'showGrid', labelKey: 'displayMenu.items.grid', descriptionKey: 'displayMenu.descriptions.grid' },
  { key: 'showSatellite', labelKey: 'displayMenu.items.satellite', descriptionKey: 'displayMenu.descriptions.satellite' },
];

// Each pill is tinted/iconned by the element it annotates (the category header
// already carries the comment glyph), so the comment/label pills inherit their
// PARENT entity's color - station comments read as station-colored, etc. -
// rather than the generic 'comment' role color.
const COMMENTS_LABELS: ToggleItem[] = [
  { key: 'showSectionNames', labelKey: 'displayMenu.items.sectionNames', descriptionKey: 'displayMenu.descriptions.sectionNames', entity: 'section', symbol: 'label' },
  { key: 'showStationLabels', labelKey: 'displayMenu.items.stationLabels', descriptionKey: 'displayMenu.descriptions.stationLabels', entity: 'station', symbol: 'label' },
  // Exclusive with Station IDs (toggleDisplay turns the other one off).
  { key: 'showStationNames', labelKey: 'displayMenu.items.stationNames', descriptionKey: 'displayMenu.descriptions.stationNames', entity: 'station', symbol: 'label', needsStationNames: true },
  { key: 'showStationComments', labelKey: 'displayMenu.items.stationComments', descriptionKey: 'displayMenu.descriptions.stationComments', entity: 'station', symbol: 'comment' },
  { key: 'showShotComments', labelKey: 'displayMenu.items.shotComments', descriptionKey: 'displayMenu.descriptions.shotComments', disabledWhen: 'showShots', entity: 'shot', symbol: 'comment' },
  { key: 'showShotData', labelKey: 'displayMenu.items.shotData', descriptionKey: 'displayMenu.descriptions.shotData', disabledWhen: 'showShots', entity: 'shot', symbol: 'data' },
  // Not gated on Shots: the loop-closure error labels follow Stations instead.
  { key: 'showShotErrors', labelKey: 'displayMenu.items.shotErrors', descriptionKey: 'displayMenu.descriptions.shotErrors', entity: 'shot', symbol: 'label' },
];

/**
 * A pill toggle. When `entityColored` is set, the pill carries its element's
 * color (`--o` = `var(--sh-<entity>)`, the same setting-wired palette the
 * data-table/legend chips use): off shows just a faint entity-tinted outline,
 * on fills with the full element color (with `onColor` - a contrast color - for
 * the label + glyph). Otherwise it fills with the theme accent. Dimmed when
 * disabled.
 */
function Pill({
  label,
  active,
  disabled,
  onClick,
  testId,
  title,
  entity,
  entityColored,
  onColor,
  icon,
  auto,
}: {
  label: string;
  active: boolean;
  disabled?: boolean;
  onClick: () => void;
  testId: string;
  title?: string;
  entity?: BlobEntity | undefined;
  /** Apply the element-colored outline/fill (true only once colors are enabled). */
  entityColored?: boolean;
  /** Contrast color for the filled (on) state's label + glyph. */
  onColor?: string | undefined;
  icon?: ReactNode;
  /** On only because the Warnings tab is open (the visit default). */
  auto?: boolean;
}) {
  const base = 'px-3 py-1 text-xs rounded-full inline-flex items-center gap-1.5 transition-colors';
  const className = entity && entityColored
    ? `${base} sh-epill ${active ? 'sh-epill--on' : ''} ${disabled ? 'opacity-50 cursor-not-allowed' : ''}`
    : `${base} border ${
        disabled
          ? 'opacity-50 cursor-not-allowed bg-background text-text-secondary sh-apill'
          : active
            ? 'bg-accent text-accent-on border-accent'
            : 'bg-background text-text-primary hover:bg-background-50 sh-apill'
      }`;
  const style = entity && entityColored
    ? ({ '--o': `var(${syntacticVarName(entity)})`, '--pill-oncolor': onColor ?? '#ffffff' } as CSSProperties)
    : undefined;
  return (
    <button
      type="button"
      onClick={() => { if (!disabled) onClick(); }}
      disabled={disabled}
      aria-pressed={active}
      title={title}
      data-testid={testId}
      data-auto={auto ? 'true' : undefined}
      className={className}
      style={style}
    >
      {icon}
      {label}
    </button>
  );
}

/** A labelled category block (optional leading icon) with its pills wrapping below. */
function Category({ label, icon, children }: { label: string; icon?: ReactNode; children: ReactNode }) {
  return (
    <div>
      <div className="px-0.5 pb-1.5 text-xs font-medium text-text-secondary uppercase tracking-wide flex items-center gap-1.5">
        {icon}
        {label}
      </div>
      <div className="flex flex-wrap gap-1.5">{children}</div>
    </div>
  );
}

export default function DisplayMenu({
  condensed = false,
  tight = false,
}: { condensed?: boolean; tight?: boolean } = {}) {
  const [isOpen, setIsOpen] = useState(false);
  const menuRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);
  const openedByOnboardingRef = useRef(false);
  const onboardingStep = useOnboardingStep();
  const { t } = useTranslation();

  // Single-select coloring mode, derived from the three mutually-exclusive overlay
  // flags (or a Warnings-tab visit). Reading + setting both go through the store
  // (selectEffectiveColorMode / setColorMode) so the select-one logic isn't
  // duplicated here. Folded into this shallow selector as plain values.
  const {
    gridSpacing,
    geometryMode,
    appMode,
    syntacticLevel,
    toggleDisplay,
    setColorMode,
    setGridSpacing,
    setGeometryMode,
    colorMode,
    visitActive,
    autoColor,
    autoErrors,
  } = useUIStore(useShallow((s) => ({
    gridSpacing: s.gridSpacing,
    geometryMode: s.geometryMode,
    appMode: s.appMode,
    syntacticLevel: s.syntacticLevel,
    toggleDisplay: s.toggleDisplay,
    setColorMode: s.setColorMode,
    setGridSpacing: s.setGridSpacing,
    setGeometryMode: s.setGeometryMode,
    // The PAINTED mode: during a Warnings-tab visit that is the visit's, not the
    // saved flags' (displaySelectors).
    colorMode: selectEffectiveColorMode(s),
    // Flattened (plain booleans) so the shallow compare stays stable.
    visitActive: selectIssueDisplayAuto(s).visit,
    autoColor: selectIssueDisplayAuto(s).color,
    autoErrors: selectIssueDisplayAuto(s).errors,
  })));

  // The element-toggle booleans, read as a keyed map so a pill can look itself
  // up by `item.key` (and its `disabledWhen` dependency) - this IS the source,
  // not a hand-maintained copy of separately destructured values.
  const toggleValues = useUIStore(
    useShallow(
      (s): Record<ToggleItem['key'], boolean> => ({
        showStations: s.showStations,
        showShots: s.showShots,
        showReferenceShots: s.showReferenceShots,
        showJumps: s.showJumps,
        showZeroDatum: s.showZeroDatum,
        showSectionNames: s.showSectionNames,
        showStationLabels: s.showStationLabels,
        showStationNames: s.showStationNames,
        showStationComments: s.showStationComments,
        showShotComments: s.showShotComments,
        showShotData: s.showShotData,
        showShotErrors: selectEffectiveShowShotErrors(s),
        showGrid: s.showGrid,
        showSatellite: s.showSatellite,
      }),
    ),
  );

  // The closure-adjusted view only differs from measured when the project has at
  // least one closure eligible for the solve (Batch / LeastSquaresWeighted) or a
  // GPS fix (#741); otherwise the option is disabled (identical geometry).
  const hasClosures = useProjectStore((s) => (s.project ? hasAdjustmentInputs(s.project) : false));
  const hasNames = useProjectStore((s) => hasStationNames(s.surveyData));

  const selectColorMode = (mode: ColorMode) => {
    if (mode === colorMode) return;
    setSource('display-menu');
    setColorMode(mode);
  };

  // Element icons appear at the 'icons' level; the element-color outline/fill
  // kicks in as soon as colors are on ('toolbar_only'+), like the rest of the chrome.
  const showIcons = syntacticLevelAtLeast(syntacticLevel, 'icons');
  const colorsOn = syntacticLevelAtLeast(syntacticLevel, 'toolbar_only');

  const entitySymbolFor = (item: ToggleItem): ReactNode => {
    if (!item.entity || !showIcons) return undefined;
    switch (item.symbol) {
      // #27: the same letter badges the data table and context menu use, so
      // the two virtual kinds are not two identical crossed-eye icons here.
      case 'jump': return <EntityJumpSymbol entity={item.entity} size={14} />;
      case 'reference': return <EntityReferenceKindSymbol entity={item.entity} size={14} />;
      case 'datum': return <EntityDatumSymbol entity={item.entity} size={14} />;
      case 'comment': return <EntityCommentSymbol entity={item.entity} size={14} />;
      case 'data': return <EntityDataSymbol entity={item.entity} size={14} />;
      case 'label': return <EntityLabelSymbol entity={item.entity} size={14} />;
      default: return <EntitySymbol entity={item.entity} size={14} />;
    }
  };

  const renderTogglePill = (item: ToggleItem) => {
    const active = toggleValues[item.key];
    // Station names replace the IDs, so with no names in the project the pill
    // would only switch the IDs off and label nothing. Offer it only when there
    // are names - or when it is already on, so it can always be turned off.
    const noNames = !!item.needsStationNames && !hasNames && !active;
    const disabled = noNames || (item.disabledWhen ? !toggleValues[item.disabledWhen] : false);
    const entityColored = !!item.entity && colorsOn;
    // Contrast color for the filled (on) state, derived from the resolved
    // (possibly user-overridden) element color so the label + glyph stay legible.
    // Read live (not cached): the --sh-<entity> vars change on a preset / per-
    // element color override without changing syntacticLevel, so a cache keyed on
    // the level would serve a stale (possibly illegible) color. The cost is one
    // getComputedStyle per on-pill on an already-open menu - negligible.
    const onColor = entityColored && active && item.entity
      ? getContrastColor(getThemeColor(syntacticVarName(item.entity)))
      : undefined;
    const auto = item.key === 'showShotErrors' && autoErrors;
    const description = item.descriptionKey ? t(item.descriptionKey) : '';
    return (
      <Pill
        key={item.key}
        label={t(item.labelKey)}
        title={noNames ? t('displayMenu.descriptions.stationNamesUnavailable') : auto ? `${description}. ${t('displayMenu.warningsVisit.pillHint')}` : description}
        auto={auto}
        active={active}
        disabled={disabled}
        onClick={() => { setSource('display-menu'); toggleDisplay(item.key); }}
        testId={`display-toggle-${item.key}`}
        entity={item.entity}
        entityColored={entityColored}
        onColor={onColor}
        icon={entitySymbolFor(item)}
      />
    );
  };

  // Close on outside mousedown / external scroll / resize (shared hook, same as
  // Select/AppMenu). Escape is handled separately below - the hook doesn't own it.
  useDismissableLayer({
    open: isOpen,
    onDismiss: () => setIsOpen(false),
    contains: [menuRef, buttonRef],
  });

  // Close on escape
  useEffect(() => {
    const handleEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setIsOpen(false);
      }
    };

    if (isOpen) {
      document.addEventListener('keydown', handleEscape);
    }

    return () => {
      document.removeEventListener('keydown', handleEscape);
    };
  }, [isOpen]);

  // Auto-open the display menu during onboarding "display options" steps. The
  // active step is observed once (shared hook); this effect just reacts to it
  // changing, instead of running a full DOM re-check on every app-wide mutation.
  // The geometry step (#331) spotlights the measured/closure-adjusted controls,
  // which only exist inside the open menu - so it opens the menu the same way.
  const isDisplayOptionsStep =
    onboardingStep.flowId === 'workspace-basics' &&
    (onboardingStep.stepId === 'display-options' || onboardingStep.stepId === 'geometry');
  useEffect(() => {
    if (isDisplayOptionsStep) {
      setIsOpen((current) => {
        if (!current) {
          openedByOnboardingRef.current = true;
          return true;
        }
        return current;
      });
      return;
    }
    if (openedByOnboardingRef.current) {
      openedByOnboardingRef.current = false;
      setIsOpen(false);
    }
  }, [isDisplayOptionsStep]);

  return (
    <div className="relative">
      <button
        ref={buttonRef}
        onClick={() => setIsOpen(!isOpen)}
        // When tight, keep the LEFT padding (so the eye isn't jammed against the tool
        // to its left) but squeeze the right padding; the eye<->caret gap tightens too.
        className={`
          ${tight ? 'pl-3 pr-1.5 py-1.5' : 'px-3 py-1.5'} text-sm rounded transition-colors
          ${isOpen
            ? 'bg-accent text-accent-on'
            : 'bg-background hover:bg-background-80 text-text-primary'
          }
        `}
        title={t('displayMenu.title')}
        data-testid="display-menu-button"
        aria-expanded={isOpen}
        // When condensed the "Display" text is dropped to save toolbar width, so the
        // button keeps an accessible name via aria-label (eye + caret stay visible).
        aria-label={condensed ? t('displayMenu.button') : undefined}
      >
        <span className={`flex items-center ${tight ? 'gap-0.5' : 'gap-1'}`}>
          <EyeIcon size={18} />
          {!condensed && <span>{t('displayMenu.button')}</span>}
          <span className="text-xs">
            {isOpen ? <ChevronUpIcon size={10} /> : <ChevronDownIcon size={10} />}
          </span>
        </span>
      </button>

      {isOpen && (
        <div
          ref={menuRef}
          className="absolute right-0 top-full mt-1 w-[30rem] overflow-y-auto overscroll-contain bg-panel border border-border rounded-lg shadow-lg z-50 p-3"
          style={{ maxHeight: 'calc(100vh - 6rem)' }}
          data-testid="display-menu"
        >
          <div className="grid grid-cols-2 gap-x-5 gap-y-4">
            {/* Row 1: Color | Overlays. The Color overlays are hidden in Plan - the gas
                reachability gradient owns the station/shot coloring there, so a color pill would
                be a confusing no-op (it still shows in Survey + Export). */}
            {appMode !== 'plan' && (
              <Category label={t('displayMenu.groups.color')} icon={showIcons ? <SlidersIcon size={14} /> : undefined}>
                <Pill label={t('displayMenu.items.section')} active={colorMode === 'section'} onClick={() => selectColorMode('section')} testId="display-color-section" />
                <Pill label={t('displayMenu.items.depthGradient')} title={t('displayMenu.descriptions.depthGradient')} active={colorMode === 'depth'} onClick={() => selectColorMode('depth')} testId="display-toggle-showDepthGradient" />
                <Pill label={t('displayMenu.items.distanceGradient')} title={t('displayMenu.descriptions.distanceGradient')} active={colorMode === 'distance'} onClick={() => selectColorMode('distance')} testId="display-toggle-showDistanceGradient" />
                <Pill label={t('displayMenu.items.issueSeverity')} title={autoColor ? `${t('displayMenu.descriptions.issueSeverity')}. ${t('displayMenu.warningsVisit.pillHint')}` : t('displayMenu.descriptions.issueSeverity')} auto={autoColor} active={colorMode === 'issues'} onClick={() => selectColorMode('issues')} testId="display-toggle-showIssueSeverity" />
                {visitActive && (
                  <p className="basis-full pt-1 text-xs text-text-secondary" data-testid="display-warnings-visit-note">
                    {t('displayMenu.warningsVisit.note')}
                  </p>
                )}
              </Category>
            )}

            <Category label={t('displayMenu.groups.overlays')} icon={showIcons ? <LayersIcon size={14} /> : undefined}>
              {OVERLAYS.map(renderTogglePill)}
              {toggleValues.showGrid && (
                <div className="basis-full flex items-center gap-2 pt-1">
                  <span className="text-xs text-text-secondary">{t('displayMenu.spacing.label')}:</span>
                  <Select
                    value={String(gridSpacing)}
                    onChange={(v) => setGridSpacing(Number(v))}
                    options={GRID_SPACING_OPTIONS.map((spacing) => ({
                      value: String(spacing),
                      label: `${formatNumber(spacing)}${t('common.units.meters')}`,
                    }))}
                    size="sm"
                    aria-label={t('displayMenu.spacing.label')}
                    data-testid="grid-spacing-select"
                  />
                </div>
              )}
            </Category>

            {/* Row 2: Map Elements (full width) */}
            <div className="col-span-2">
              <Category label={t('displayMenu.groups.mapElements')} icon={showIcons ? <MapIcon size={14} /> : undefined}>
                {MAP_ELEMENTS.map(renderTogglePill)}
              </Category>
            </div>

            {/* Row 3: Comments & Labels (full width) */}
            <div className="col-span-2">
              <Category label={t('displayMenu.groups.commentsLabels')} icon={showIcons ? <MessageIcon size={14} /> : undefined}>
                {COMMENTS_LABELS.map(renderTogglePill)}
              </Category>
            </div>

            {/* Row 4: Geometry (single-select), full width. Survey-only - Plan/Export force the
                read-only closure map, so switching between the Raw Data View and the Closures
                Computed View there would fight the mode. */}
            {appMode === 'survey' && (
            <div className="col-span-2 border-t border-border pt-3">
              <Category label={t('displayMenu.geometry.groupLabel')} icon={showIcons ? <RulerIcon size={14} /> : undefined}>
                <Pill
                  label={t('displayMenu.geometry.measured')}
                  active={geometryMode === 'measured'}
                  onClick={() => setGeometryMode('measured', 'display-menu')}
                  testId="geometry-mode-measured"
                  icon={showIcons ? <LockOpenIcon size={14} /> : undefined}
                />
                <Pill
                  label={t('displayMenu.geometry.closureAdjusted')}
                  active={geometryMode === 'closureAdjusted'}
                  disabled={!hasClosures}
                  title={!hasClosures ? t('displayMenu.geometry.noClosures') : ''}
                  onClick={() => setGeometryMode('closureAdjusted', 'display-menu')}
                  testId="geometry-mode-closureAdjusted"
                  icon={showIcons ? <LockIcon size={14} /> : undefined}
                />
              </Category>
              <p className="mt-1.5 text-xs text-text-secondary">
                {geometryMode === 'closureAdjusted'
                  ? t('displayMenu.geometry.closureAdjustedDescription')
                  : t('displayMenu.geometry.measuredDescription')}
              </p>
            </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
