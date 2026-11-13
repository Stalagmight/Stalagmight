import { useTranslation } from 'react-i18next';
import { useUIStore, type AppMode } from '../../store/uiStore';
import { ListIcon, MapIcon, SlidersIcon, type IconProps } from '../common/icons';
import type { ComponentType } from 'react';

/** The modes the carousel switches between. Export is not one of them: it is entered
 *  from the project menu (or Ctrl+E) and left with its own Close export button. */
type CarouselMode = Exclude<AppMode, 'export'>;

const MODES: readonly CarouselMode[] = ['survey', 'data', 'plan'];

/** Icon stand-ins for the mode labels in the compact (icon-only) variant. */
const MODE_ICONS: Record<CarouselMode, ComponentType<IconProps>> = {
  survey: MapIcon,
  data: ListIcon,
  plan: SlidersIcon,
};

/**
 * Top-middle workspace switcher (#38, the workspace-mode reframe). Selects the top-level mode
 * for an open project: Survey (the editable map workspace), Data (the editable full-project
 * table) or Plan (the gas planner over a read-only closure map). While Export is open no
 * button is pressed, and clicking one leaves Export for that mode.
 * Lives in the Toolbar's center slot - the active *tool* is already shown by the
 * highlighted toolbar button, so the center is free for the mode switch.
 *
 * No discard-on-switch guard: the gas store now persists across mode switches (#244),
 * so leaving Plan mode loses nothing (the overlay is appMode-gated and a loaded plan
 * stays loaded). Unsaved-plan edits are still guarded at app close (App) and when
 * loading a different plan (DivePlanBar).
 */
export function ModeCarousel({
  disabled = false,
  compact = false,
}: {
  disabled?: boolean;
  /** Icon-only variant for the Toolbar's final condense stage (#260): the labels
   *  are replaced by their icons and kept as the accessible name + tooltip, so the
   *  carousel asks for roughly a third of the width and can share a narrow header
   *  with both clusters instead of being pushed on top of one. */
  compact?: boolean;
}) {
  const { t } = useTranslation();
  const appMode = useUIStore((s) => s.appMode);
  const setAppMode = useUIStore((s) => s.setAppMode);

  return (
    // `max-w-full` is load-bearing, not cosmetic (#260): in the Toolbar's final
    // condense stage this sits in a shrinkable flex slot, and a flex item's parent
    // does NOT bound its box - without the cap the carousel kept its full intrinsic
    // width in a few-px slot and was laid out right across the tool cluster
    // (measured: a persistent overlap at 540 CSS px and below).
    //
    // The overflow is the carousel's OWN, and it scrolls rather than clips: this is
    // the only way to switch between Survey, Data and Plan (no menu item, no
    // shortcut), so a clipped mode is an unreachable workspace, not a cramped one.
    // Every mode stays reachable by wheel/trackpad and by Tab, since focusing a
    // scrolled-out button brings it into view. Y stays hidden so a horizontal
    // scrollbar can never introduce a vertical one.
    <div
      className={`inline-flex max-w-full overflow-x-auto overflow-y-hidden rounded border border-border ${
        disabled ? 'opacity-45 grayscale' : ''
      }`}
      role="group"
      aria-label={t('modeCarousel.aria')}
      data-testid="mode-carousel"
    >
      {MODES.map((mode, i) => {
        const active = appMode === mode;
        const label = t(`modeCarousel.${mode}`);
        const Icon = MODE_ICONS[mode];
        return (
          <button
            key={mode}
            type="button"
            aria-pressed={active}
            // Disabled while a survey placement is in progress: switching to the
            // read-only Plan map, or to Data's table over the map, would otherwise
            // leave that mutation live.
            disabled={disabled}
            data-testid={`mode-${mode}`}
            onClick={() => setAppMode(mode)}
            // Compact drops the visible text, so the label has to come back as the
            // accessible name (and a tooltip) or the button becomes unnamed.
            {...(compact ? { 'aria-label': label, title: label } : {})}
            // `shrink-0`: the group is a scroll container now, so a too-narrow slot
            // must scroll the buttons, never squash them into unreadable slivers.
            className={`shrink-0 ${compact ? 'px-2 py-1.5 flex items-center justify-center' : 'px-3 py-1.5'} text-sm font-medium transition-colors disabled:cursor-not-allowed ${
              i > 0 ? 'border-l border-border' : ''
            } ${
              active ? 'bg-accent text-accent-on' : 'text-text-secondary hover:text-text hover:bg-panel'
            }`}
          >
            {compact ? <Icon size={16} /> : label}
          </button>
        );
      })}
    </div>
  );
}
