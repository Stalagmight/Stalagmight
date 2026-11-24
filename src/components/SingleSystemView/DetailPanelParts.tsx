import type React from 'react';

// Small presentational pieces of the Detail panel, split out of
// DetailPanel.tsx to keep it under the TS file-size budget (#157).

/** Center/locate glyph (small ring + edge spokes), filled so it stays crisp
 *  under the page-zoom UI scale. Shared by the per-entity "center on map"
 *  buttons (station/shot/line) below. */
export function CenterLocateIcon() {
  return (
    <svg className="w-4 h-4" fill="currentColor" viewBox="0 0 24 24" aria-hidden="true">
      <path
        fillRule="evenodd"
        d="M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0ZM11.2 12a0.8 0.8 0 1 0 1.6 0a0.8 0.8 0 1 0 -1.6 0Z"
      />
      <rect x="10.9" y="2" width="2.2" height="4" rx="1.1" />
      <rect x="10.9" y="18" width="2.2" height="4" rx="1.1" />
      <rect x="2" y="10.9" width="4" height="2.2" rx="1.1" />
      <rect x="18" y="10.9" width="4" height="2.2" rx="1.1" />
    </svg>
  );
}

export function StatCard({ label, value, hint, testId }: { label: string; value: string; hint?: string; testId?: string }) {
  return (
    <div className="text-sm" title={hint} data-testid={testId}>
      <div className="text-text-secondary text-xs">{label}</div>
      <div className="font-medium">{value}</div>
    </div>
  );
}

export function DetailRow({ label, value, testId }: { label: string; value: React.ReactNode; testId?: string }) {
  return (
    <div className="flex justify-between gap-2 text-sm" data-testid={testId}>
      <span className="shrink-0 text-text-secondary">{label}:</span>
      {/* min-w-0: a long pill (a line name) ellipsizes inside the panel
          instead of running past its edge (RS-OWNERSHIP). */}
      <span className="min-w-0 text-right">{value}</span>
    </div>
  );
}
