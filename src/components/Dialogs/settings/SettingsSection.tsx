import { type ReactNode } from 'react';

interface SettingsSectionProps {
  /** Section title displayed as header */
  title: string;
  /** Child setting rows */
  children: ReactNode;
  /** Test ID for E2E tests */
  'data-testid'?: string;
}

/**
 * Groups related settings with a header.
 * Used to organize settings within a category.
 */
export function SettingsSection({
  title,
  children,
  'data-testid': dataTestId,
}: SettingsSectionProps) {
  return (
    <div className="mb-6" data-testid={dataTestId}>
      <h3 className="text-sm font-semibold text-text-secondary uppercase tracking-wide mb-3 pb-1 border-b border-border">
        {title}
      </h3>
      <div className="space-y-1">{children}</div>
    </div>
  );
}
