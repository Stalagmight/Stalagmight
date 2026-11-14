import type { DmpImportWizardPhase, ViewType } from '../store/uiStore';

export type OnboardingPlacement = 'top' | 'bottom' | 'left' | 'right' | 'center';
export type OnboardingInteraction = 'locked' | 'target' | 'free';
export type OnboardingIllustration = 'tools-overview' | 'map-with-pan-zoom' | 'mouse-pan-zoom' | 'map-controls';

export interface OnboardingStepDefinition {
  id: string;
  titleKey: string;
  descriptionKey: string;
  /** Optional i18n key for a bullet-point list below the description */
  descriptionItemsKey?: string;
  targetSelector?: string;
  targetSelectors?: string[];
  mergeTargetSpotlights?: boolean;
  placement?: OnboardingPlacement;
  interaction?: OnboardingInteraction;
  tooltipMaxWidth?: number;
  tooltipOffsetX?: number;
  tooltipTargetSelector?: string;
  illustration?: OnboardingIllustration;
}

export interface OnboardingContext {
  view: ViewType;
  hasProject: boolean;
  activeDialog: string | null;
  dmpImportWizardPhase: DmpImportWizardPhase | null;
  placementModeActive: boolean;
  loadingProject: boolean;
  splashVisible: boolean;
  configLoaded: boolean;
  /**
   * The one-shot orientation tour has already had its chance. Scoped to that
   * tour only (#331): the contextual flows below are triggered by an action the
   * user may not take for months, so suppressing them globally would mean a
   * surveyor who imports their first DMP in week three never sees its guide.
   * Repeat-suppression for those is `onboarding_completed_flows`, per flow.
   */
  orientationConsumed: boolean;
}

export interface OnboardingFlowDefinition {
  id: string;
  titleKey: string;
  shouldStart: (context: OnboardingContext) => boolean;
  steps: OnboardingStepDefinition[];
}

const canShowOnboarding = (context: OnboardingContext): boolean =>
  context.configLoaded &&
  !context.splashVisible &&
  !context.loadingProject &&
  !context.placementModeActive &&
  context.activeDialog === null;

// The placement clause is DEFENSIVE, not load-bearing: every current path into
// placement mode nulls `activeDialog` and the wizard phase first (uiStore
// `closeDialog`), so no reachable state satisfies both today. It is kept so that
// a future path which starts placement with the wizard still registered cannot
// anchor a dialog flow to a control that is no longer on screen. What actually
// prevents that dead-end is the unresolvable-target fallback in
// OnboardingOverlay - not this clause (#331).
const canShowDialogOnboarding = (context: OnboardingContext): boolean =>
  context.configLoaded &&
  !context.splashVisible &&
  !context.loadingProject &&
  !context.placementModeActive;

const canShowPlacementOnboarding = (context: OnboardingContext): boolean =>
  context.configLoaded &&
  !context.splashVisible &&
  !context.loadingProject &&
  context.view === 'single' &&
  context.hasProject &&
  context.placementModeActive &&
  context.activeDialog === null;

/**
 * The one orientation flow. It covers what is specific to Stalagmight and not
 * inferable from a survey background: the workspace modes and their read-only
 * contract, the measured/closure-adjusted split, and where the tools and layers
 * live. Panels a surveyor can identify on sight get one merged step, not three.
 */
/** The one-shot orientation tour. See `orientationConsumed`. */
export const ORIENTATION_FLOW_ID = 'workspace-basics';

const workspaceFlow: OnboardingFlowDefinition = {
  id: ORIENTATION_FLOW_ID,
  titleKey: 'onboarding.flows.workspace.title',
  shouldStart: (context) =>
    canShowOnboarding(context) &&
    !context.orientationConsumed &&
    context.view === 'single' &&
    context.hasProject,
  steps: [
    {
      id: 'modes',
      titleKey: 'onboarding.flows.workspace.steps.modes.title',
      descriptionKey: 'onboarding.flows.workspace.steps.modes.description',
      descriptionItemsKey: 'onboarding.flows.workspace.steps.modes.descriptionItems',
      targetSelector: '[data-testid="mode-carousel"]',
      placement: 'bottom',
      interaction: 'locked',
    },
    {
      id: 'tools',
      titleKey: 'onboarding.flows.workspace.steps.tools.title',
      descriptionKey: 'onboarding.flows.workspace.steps.tools.description',
      targetSelector: '[data-testid="toolbar-tools-group"]',
      placement: 'bottom',
      interaction: 'locked',
      illustration: 'tools-overview',
      tooltipMaxWidth: 980,
    },
    // `display-options` and `geometry` are adjacent on purpose: both are anchored
    // inside the Display menu, which DisplayMenu auto-opens for exactly these two
    // step ids. A step between them would close and re-open the menu mid-tour.
    {
      id: 'display-options',
      titleKey: 'onboarding.flows.workspace.steps.displayOptions.title',
      descriptionKey: 'onboarding.flows.workspace.steps.displayOptions.description',
      targetSelectors: [
        '[data-testid="display-menu-button"]',
        '[data-testid="display-menu"]',
      ],
      mergeTargetSpotlights: true,
      placement: 'left',
      tooltipTargetSelector: '[data-testid="display-menu-button"]',
      tooltipOffsetX: -150,
      interaction: 'locked',
    },
    {
      // Editable-mode controls (DisplayMenu gates the Geometry group on
      // `isEditableMode(appMode)`, i.e. Survey, or Data with its map pane open).
      // Elsewhere this step has no anchor and falls back to a centered card
      // rather than blocking.
      id: 'geometry',
      titleKey: 'onboarding.flows.workspace.steps.geometry.title',
      descriptionKey: 'onboarding.flows.workspace.steps.geometry.description',
      targetSelectors: [
        '[data-testid="geometry-mode-measured"]',
        '[data-testid="geometry-mode-closureAdjusted"]',
      ],
      mergeTargetSpotlights: true,
      placement: 'left',
      tooltipTargetSelector: '[data-testid="geometry-mode-measured"]',
      tooltipOffsetX: -150,
      interaction: 'locked',
    },
    {
      id: 'docks',
      titleKey: 'onboarding.flows.workspace.steps.docks.title',
      descriptionKey: 'onboarding.flows.workspace.steps.docks.description',
      descriptionItemsKey: 'onboarding.flows.workspace.steps.docks.descriptionItems',
      targetSelectors: [
        '[data-testid="sections-panel"]',
        '[data-testid="detail-panel"]',
        '[data-testid="data-table"]',
      ],
      placement: 'center',
      interaction: 'locked',
    },
    {
      id: 'done',
      titleKey: 'onboarding.flows.workspace.steps.done.title',
      descriptionKey: 'onboarding.flows.workspace.steps.done.description',
      placement: 'center',
      interaction: 'locked',
    },
  ],
};

/**
 * One tooltip per DMP wizard screen. These cannot be a single flow: a flow is
 * torn down as soon as its `shouldStart` context goes false (useOnboarding),
 * and each wizard phase is a different dialog. They share a title so they read
 * as one thread of guidance.
 */
const dmpSelectionFlow: OnboardingFlowDefinition = {
  id: 'dmp-selection',
  titleKey: 'onboarding.flows.dmpImport.title',
  shouldStart: (context) =>
    canShowDialogOnboarding(context) &&
    context.activeDialog === 'dmp-selection',
  steps: [
    {
      id: 'selection',
      titleKey: 'onboarding.flows.dmpImport.steps.selection.title',
      descriptionKey: 'onboarding.flows.dmpImport.steps.selection.description',
      targetSelector: '[data-testid="dmp-section-table"]',
      placement: 'top',
      interaction: 'locked',
    },
  ],
};

const zeroDatumSelectionFlow: OnboardingFlowDefinition = {
  id: 'zero-datum-selection',
  titleKey: 'onboarding.flows.dmpImport.title',
  shouldStart: (context) =>
    canShowDialogOnboarding(context) &&
    context.activeDialog === 'zero-datum-selection',
  steps: [
    {
      id: 'zero-datum-selection',
      titleKey: 'onboarding.flows.dmpImport.steps.zeroDatumSelection.title',
      descriptionKey: 'onboarding.flows.dmpImport.steps.zeroDatumSelection.description',
      targetSelector: '[data-testid="zero-datum-selection-dialog"]',
      placement: 'bottom',
      interaction: 'locked',
    },
  ],
};

const zeroDatumConfigFlow: OnboardingFlowDefinition = {
  id: 'zero-datum-config',
  titleKey: 'onboarding.flows.dmpImport.title',
  shouldStart: (context) =>
    canShowDialogOnboarding(context) &&
    context.activeDialog === 'zero-datum-config',
  steps: [
    {
      id: 'zero-datum',
      titleKey: 'onboarding.flows.dmpImport.steps.zeroDatum.title',
      descriptionKey: 'onboarding.flows.dmpImport.steps.zeroDatum.description',
      descriptionItemsKey: 'onboarding.flows.dmpImport.steps.zeroDatum.descriptionItems',
      targetSelector: '[data-testid="zero-datum-gps-input"]',
      placement: 'bottom',
      interaction: 'locked',
    },
  ],
};

const dmpSuggestedConnectionFlow: OnboardingFlowDefinition = {
  id: 'dmp-suggested-connection',
  titleKey: 'onboarding.flows.dmpImport.title',
  shouldStart: (context) =>
    canShowDialogOnboarding(context) &&
    context.activeDialog === 'dmp-import-new' &&
    context.dmpImportWizardPhase === 'suggesting',
  steps: [
    {
      id: 'suggested-connection',
      titleKey: 'onboarding.flows.dmpImport.steps.suggestedConnection.title',
      descriptionKey: 'onboarding.flows.dmpImport.steps.suggestedConnection.description',
      descriptionItemsKey: 'onboarding.flows.dmpImport.steps.suggestedConnection.descriptionItems',
      targetSelectors: [
        '[data-testid="wizard-connection-description"]',
        '[data-testid="wizard-overlap-control"]',
      ],
      placement: 'bottom',
      interaction: 'locked',
    },
  ],
};

const dmpSegmentPlacementFlow: OnboardingFlowDefinition = {
  id: 'dmp-segment-placement',
  titleKey: 'onboarding.flows.dmpImport.title',
  shouldStart: (context) =>
    canShowDialogOnboarding(context) &&
    context.activeDialog === 'dmp-import-new' &&
    context.dmpImportWizardPhase === 'manual_placing',
  steps: [
    {
      id: 'segment-placement',
      titleKey: 'onboarding.flows.dmpImport.steps.segmentPlacement.title',
      descriptionKey: 'onboarding.flows.dmpImport.steps.segmentPlacement.description',
      descriptionItemsKey: 'onboarding.flows.dmpImport.steps.segmentPlacement.descriptionItems',
      targetSelector: '[data-testid="segment-placement-modes"]',
      placement: 'left',
      interaction: 'locked',
    },
  ],
};

/**
 * On-map placement stays: click-to-place with hover-snap-merge, and the fact
 * that Esc reopens the placement options, are not discoverable by looking.
 */
const placementModeFlow: OnboardingFlowDefinition = {
  id: 'placement-mode-map',
  titleKey: 'onboarding.flows.placementMode.title',
  shouldStart: (context) => canShowPlacementOnboarding(context),
  steps: [
    {
      id: 'place-on-map',
      titleKey: 'onboarding.flows.placementMode.steps.placeOnMap.title',
      descriptionKey: 'onboarding.flows.placementMode.steps.placeOnMap.description',
      descriptionItemsKey: 'onboarding.flows.placementMode.steps.placeOnMap.descriptionItems',
      targetSelector: '[data-testid="map-canvas"]',
      placement: 'top',
      interaction: 'locked',
      // Renders pan/zoom plus fit-to-view, zoom and scale-bar guidance - all of
      // which are what you actually need while positioning a section.
      illustration: 'map-with-pan-zoom',
    },
  ],
};

export const ONBOARDING_FLOWS: readonly OnboardingFlowDefinition[] = [
  placementModeFlow,
  workspaceFlow,
  dmpSelectionFlow,
  zeroDatumSelectionFlow,
  zeroDatumConfigFlow,
  dmpSuggestedConnectionFlow,
  dmpSegmentPlacementFlow,
];
