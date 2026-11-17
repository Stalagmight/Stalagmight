import { useTranslation } from 'react-i18next';
import { themeTokens } from '../../../theme/tokens';
import type { ColorTheme } from '../../../types/config';

const COLOR_THEME_NAME_KEYS: Record<ColorTheme, string> = {
  dark: 'settings.appearance.theme.darkCard.title',
  light: 'settings.appearance.theme.lightCard.title',
  cave: 'settings.appearance.theme.caveCard.title',
  dracula: 'settings.appearance.theme.draculaCard.title',
  one_dark: 'settings.appearance.theme.oneDarkCard.title',
  solarized_light: 'settings.appearance.theme.solarizedLightCard.title',
  catppuccin_latte: 'settings.appearance.theme.catppuccinLatteCard.title',
  nord: 'settings.appearance.theme.nordCard.title',
  catppuccin_mocha: 'settings.appearance.theme.catppuccinMochaCard.title',
  kaleido_eink: 'settings.appearance.theme.kaleidoEinkCard.title',
};

interface ThemePreviewCardProps {
  /** Which mode section this card represents */
  mode: 'day' | 'night';
  /** Which color theme is currently selected for this mode */
  colorTheme: ColorTheme;
  /** Whether this mode is currently active (applied) */
  isActive: boolean;
  /** Description shown in auto mode */
  description?: string | undefined;
}

/**
 * Visual preview card for a mode section (day/night), showing a mini app
 * mockup using the selected color theme's actual colors from themeTokens.
 */
export function ThemePreviewCard({
  mode,
  colorTheme,
  isActive,
  description,
}: ThemePreviewCardProps) {
  const { t } = useTranslation();
  const tokens = themeTokens[colorTheme];

  const bg = tokens['--cc-background'];
  const panel = tokens['--cc-panel'];
  const border = tokens['--cc-border'];
  const textSecondary = tokens['--cc-text-secondary'];
  const accent = tokens['--cc-accent'];
  const success = tokens['--cc-success'];
  const error = tokens['--cc-error'];

  const modeTitleKey =
    mode === 'day'
      ? 'settings.appearance.theme.dayTheme'
      : 'settings.appearance.theme.nightTheme';

  const colorThemeNameKey = COLOR_THEME_NAME_KEYS[colorTheme];

  return (
    <div
      data-testid={`theme-card-${mode}`}
      className={`
        flex-1 rounded-lg overflow-hidden text-left transition-all
        border-2
        ${isActive ? 'border-accent ring-1 ring-accent' : 'border-border'}
      `}
    >
      {/* Card header: mode title + active badge */}
      <div className="px-3 py-2 flex items-start justify-between gap-2 bg-panel border-b border-border">
        <div className="min-w-0">
          <div className="flex items-center gap-1.5">
            {/* Sun/Moon icon */}
            {mode === 'day' ? (
              <svg className="w-3.5 h-3.5 text-text-secondary flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth={2}>
                <circle cx="12" cy="12" r="5" />
                <path d="M12 1v2M12 21v2M4.22 4.22l1.42 1.42M18.36 18.36l1.42 1.42M1 12h2M21 12h2M4.22 19.78l1.42-1.42M18.36 5.64l1.42-1.42" />
              </svg>
            ) : (
              <svg className="w-3.5 h-3.5 text-text-secondary flex-shrink-0" fill="none" stroke="currentColor" viewBox="0 0 24 24" strokeWidth={2}>
                <path d="M21 12.79A9 9 0 1111.21 3 7 7 0 0021 12.79z" />
              </svg>
            )}
            <span className="text-sm font-medium text-text-primary">{t(modeTitleKey)}</span>
          </div>
          {description && (
            <p className="text-xs text-text-secondary mt-0.5">{description}</p>
          )}
        </div>
        {isActive && (
          <span
            className="text-xs font-medium px-1.5 py-0.5 rounded bg-accent text-accent-on flex-shrink-0"
            data-testid={`theme-active-badge-${mode}`}
          >
            {t('settings.appearance.theme.active')}
          </span>
        )}
      </div>

      {/* Mini app preview */}
      <div
        className="relative"
        style={{ backgroundColor: bg }}
      >
        {/* Title bar */}
        <div
          className="flex items-center gap-1.5 px-2.5 py-1.5"
          style={{ backgroundColor: panel, borderBottom: `1px solid ${border}` }}
        >
          {/* Window dots */}
          <div className="flex gap-1">
            <div className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: error }} />
            <div className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: tokens['--cc-warning'] }} />
            <div className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: success }} />
          </div>
          {/* Tab placeholders */}
          <div className="flex gap-1 ml-2">
            <div className="w-8 h-1.5 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.3 }} />
            <div className="w-6 h-1.5 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.2 }} />
          </div>
        </div>

        {/* Content area */}
        <div className="flex" style={{ minHeight: 88 }}>
          {/* Sidebar */}
          <div
            className="flex flex-col gap-2 p-2"
            style={{ backgroundColor: panel, width: 44, borderRight: `1px solid ${border}` }}
          >
            <div className="w-full h-2 rounded-sm" style={{ backgroundColor: accent, opacity: 0.7 }} />
            <div className="w-3/4 h-2 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.25 }} />
            <div className="w-full h-2 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.25 }} />
            <div className="w-2/3 h-2 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.25 }} />
            <div className="w-full h-2 rounded-sm" style={{ backgroundColor: textSecondary, opacity: 0.2 }} />
          </div>

          {/* Main content */}
          <div className="flex-1 p-2 flex flex-col gap-2">
            {/* Progress bar */}
            <div className="w-full h-2 rounded-full" style={{ backgroundColor: border }}>
              <div className="w-3/5 h-full rounded-full" style={{ backgroundColor: accent }} />
            </div>
            {/* Content blocks */}
            <div className="flex gap-1.5">
              <div className="flex-1 h-7 rounded" style={{ backgroundColor: panel }} />
              <div className="w-10 h-7 rounded" style={{ backgroundColor: panel }} />
            </div>
            {/* Extra content row */}
            <div className="flex gap-1.5">
              <div className="flex-1 h-4 rounded" style={{ backgroundColor: panel, opacity: 0.7 }} />
              <div className="flex-1 h-4 rounded" style={{ backgroundColor: panel, opacity: 0.5 }} />
            </div>
            {/* Status dots */}
            <div className="flex gap-1 mt-auto">
              <div className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: success }} />
              <div className="w-1.5 h-1.5 rounded-full" style={{ backgroundColor: accent }} />
            </div>
          </div>
        </div>
      </div>

      {/* Card footer: selected color theme name */}
      <div className="px-3 py-1.5 bg-panel border-t border-border">
        <span className="text-xs text-text-secondary">{t(colorThemeNameKey)}</span>
      </div>
    </div>
  );
}

/* ---- Color theme circle selector ---- */

interface ColorThemeCircleProps {
  colorTheme: ColorTheme;
  isSelected: boolean;
  onClick: () => void;
}

/**
 * Small circle showing representative colors from a color theme.
 * Three segments like GitHub's theme circles: left half background,
 * top-right quarter accent, bottom-right quarter panel.
 */
export function ColorThemeCircle({
  colorTheme,
  isSelected,
  onClick,
}: ColorThemeCircleProps) {
  const { t } = useTranslation();
  const tokens = themeTokens[colorTheme];
  const bg = tokens['--cc-background'];
  const accent = tokens['--cc-accent'];
  const panel = tokens['--cc-panel'];

  return (
    <button
      type="button"
      onClick={onClick}
      data-testid={`color-theme-circle-${colorTheme}`}
      className={`
        w-8 h-8 rounded-full overflow-hidden flex-shrink-0
        transition-all cursor-pointer
        ${isSelected
          ? 'ring-2 ring-accent ring-offset-2 ring-offset-panel'
          : 'ring-1 ring-border hover:ring-text-secondary'}
      `}
      aria-label={t(COLOR_THEME_NAME_KEYS[colorTheme])}
      aria-pressed={isSelected}
    >
      <svg viewBox="0 0 32 32" className="w-full h-full">
        {/* Top-left half: background */}
        <path d="M0 0 L32 0 L0 32 Z" fill={bg} />
        {/* Right quarter: accent */}
        <path d="M32 0 L32 32 L16 16 Z" fill={accent} />
        {/* Bottom quarter: panel */}
        <path d="M0 32 L32 32 L16 16 Z" fill={panel} />
      </svg>
    </button>
  );
}
