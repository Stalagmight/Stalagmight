import { useTranslation } from 'react-i18next';

interface ResetButtonProps {
  onReset: () => void;
  /** Already at the default: the button greys out and does nothing. */
  isDefault: boolean;
  /** Overrides the "Reset to default" tooltip / accessible name. */
  title?: string | undefined;
  'data-testid'?: string | undefined;
}

/**
 * The small circular-arrow "reset to default" button the Settings rows use,
 * shared so the map's colour-scale controls reset the same way.
 */
export function ResetButton({ onReset, isDefault, title, 'data-testid': testId }: ResetButtonProps) {
  const { t } = useTranslation();
  const label = title ?? t('settings.common.resetToDefault');
  return (
    <button
      type="button"
      onClick={onReset}
      disabled={isDefault}
      className={`p-1 rounded hover:bg-panel transition-colors ${
        !isDefault ? 'text-text-secondary hover:text-text-primary' : 'text-border cursor-not-allowed'
      }`}
      title={label}
      aria-label={label}
      data-testid={testId}
    >
      <svg className="w-3.5 h-3.5" fill="none" stroke="currentColor" viewBox="0 0 24 24" aria-hidden="true">
        <path
          strokeLinecap="round"
          strokeLinejoin="round"
          strokeWidth={2}
          d="M4 4v5h.582m15.356 2A8.001 8.001 0 004.582 9m0 0H9m11 11v-5h-.581m0 0a8.003 8.003 0 01-15.357-2m15.357 2H15"
        />
      </svg>
    </button>
  );
}
