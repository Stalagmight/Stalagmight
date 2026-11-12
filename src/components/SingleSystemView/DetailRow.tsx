import type { ReactNode } from 'react';

/** One `label: value` line in the Detail panel. */
export function DetailRow({ label, value, testId }: { label: string; value: ReactNode; testId?: string }) {
  return (
    <div className="flex justify-between text-sm" data-testid={testId}>
      <span className="text-text-secondary">{label}:</span>
      <span>{value}</span>
    </div>
  );
}
