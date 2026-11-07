import type { CSSProperties } from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { extractFilename } from '../../utils/path';

/**
 * Busy indicator for an import whose file is still being read (#576 Q-W27-1-D1).
 *
 * `routeOpenedFile` parses a picked, dropped or OS-launched file before it opens
 * the flow's first dialog, and for that whole parse nothing else is on screen -
 * with no project open the Global View looked untouched, so a slow or oversized
 * file read as nothing happening. While `importParse` is set the router also
 * refuses a second file (one import at a time), and this says which one it is
 * busy with.
 *
 * Shown only while no dialog is up: a dialog owns the screen, and the one that
 * stays up during a parse (`ImportAsNewProjectDialog`) shows its own progress
 * line. Pointer events pass through, so it never blocks a control.
 *
 * It floats the usual `bottom-6` gap above the status bar both views end in
 * (#576 W62 item 5: anchored at `bottom-6` from the window edge, it straddled
 * the bar's top border). The bar's height is the clearance the bar registers
 * for the toast stack (`useToastBottomClearance`), not a restatement of its
 * classes, so a bar that grows (wraps to two lines) carries the indicator up
 * with it (#576 W71). Anything else it must not cover registers its own
 * clearance (`useParseIndicatorClearance`): in the project view, the map's
 * imagery attribution, which rides the map's bottom edge above the bar.
 */
export function ImportParseIndicator() {
  const { t } = useTranslation();
  const importParse = useUIStore((s) => s.importParse);
  const activeDialog = useUIStore((s) => s.activeDialog);
  // The highest edge registered along the window's bottom, in CSS px - the same
  // largest-wins reading the toast stack makes of `toastBottomOffsets`.
  const clearancePx = useUIStore((s) =>
    Math.max(0, ...Object.values(s.toastBottomOffsets), ...Object.values(s.parseIndicatorBottomOffsets)),
  );
  if (importParse === null || activeDialog !== null) return null;
  return (
    <div
      className="fixed bottom-[calc(var(--parse-indicator-clearance)+theme(spacing.6))] left-1/2 z-40 flex -translate-x-1/2 items-center gap-2 rounded-lg border border-border bg-panel px-3 py-2 text-sm text-text-secondary shadow-lg pointer-events-none"
      style={{ '--parse-indicator-clearance': `${clearancePx}px` } as CSSProperties}
      role="status"
      data-testid="import-parse-indicator"
    >
      <span
        className="h-4 w-4 shrink-0 animate-spin rounded-full border-2 border-accent border-t-transparent"
        aria-hidden="true"
      />
      {t('importGate.parsing', { fileName: extractFilename(importParse.path) })}
    </div>
  );
}
