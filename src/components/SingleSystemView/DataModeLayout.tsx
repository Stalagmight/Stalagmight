/**
 * The map area's layout in every mode, and the Data workspace's place in it.
 *
 * In Survey, Plan and Export this is just the map. In Data the workspace sits to
 * the LEFT of the map: with the map pane closed it covers the whole area (opaque)
 * while the map stays mounted, hidden, beneath it; with the pane open the map
 * shrinks to a right-hand pane of `mapPaneWidth`, a draggable splitter between
 * the two, and `DataMapPaneHeader` sits on top of the pane.
 *
 * The one rule this component exists to keep: **the map is never unmounted or
 * remounted** by a mode or pane change. MapLibre owns a WebGL context, sources,
 * the camera and a pile of map-scoped state; a remount would throw all of it away
 * on every Data <-> Survey switch. So every child sits in a fixed slot (each
 * conditional is a `{cond && ...}` that holds its index when false), and the map
 * column is always the third slot, whatever the mode.
 *
 * Width: the pane is clamped to `MAP_PANE_MIN_WIDTH` .. (area width minus
 * `DATA_WORKSPACE_MIN_WIDTH`), so the table always keeps a usable column. The
 * clamp is display-only (the stored width is the user's, and a wider window
 * gives it back), the same rule as the side docks' squeeze. A drag lives in
 * local state and reaches the store once, on release; the keyboard writes on a
 * short trailing debounce - the WizardMapPanel precedent (#567).
 */
import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent,
  type ReactNode,
} from 'react';
import { useTranslation } from 'react-i18next';
import { useUIStore } from '../../store/uiStore';
import { clampRound } from '../../utils/clamp';
import { ErrorBoundary } from '../common/ErrorBoundary';
import { DataWorkspace } from '../DataWorkspace/DataWorkspace';
import { DataMapPaneHeader } from '../DataWorkspace/DataMapPaneHeader';
import {
  MAP_PANE_DEFAULT_WIDTH,
  MAP_PANE_MIN_WIDTH,
  useDataWorkspaceStore,
} from '../DataWorkspace/store';
import { useResizeHandle } from './useResizeHandle';

/** The narrowest the Data table may be squeezed by the map pane (px). */
export const DATA_WORKSPACE_MIN_WIDTH = 480;

/** Arrow-key resize step (px), and the Shift+Arrow step. Same as WizardMapPanel. */
const KEY_STEP = 16;
const KEY_STEP_COARSE = 64;
/** Quiet period (ms) after the last resize key before the width reaches the store. */
const KEY_PERSIST_DELAY_MS = 300;

/**
 * The widest the map pane can render in an area `areaWidth` px wide: whatever
 * leaves the table `DATA_WORKSPACE_MIN_WIDTH`, never below the pane's own floor.
 * `areaWidth <= 0` means "not laid out yet" (first paint, jsdom): no cap then
 * beyond the stored width itself, so nothing is clamped on a guess.
 */
export function mapPaneMaxWidth(areaWidth: number, storedWidth: number): number {
  if (areaWidth <= 0) return Math.max(MAP_PANE_MIN_WIDTH, storedWidth);
  return Math.max(MAP_PANE_MIN_WIDTH, Math.floor(areaWidth - DATA_WORKSPACE_MIN_WIDTH));
}

/** `children` is the map and its overlays (MapCanvas first). */
export function DataModeLayout({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  const isData = useUIStore((s) => s.appMode === 'data');
  const paneOpen = useDataWorkspaceStore((s) => s.mapPaneOpen);
  const storedWidth = useDataWorkspaceStore((s) => s.mapPaneWidth);
  const setMapPaneWidth = useDataWorkspaceStore((s) => s.setMapPaneWidth);
  const split = isData && paneOpen;
  const covered = isData && !paneOpen;

  // The area's width, for the clamp. Observed rather than read per render.
  const rootRef = useRef<HTMLDivElement>(null);
  const paneRef = useRef<HTMLDivElement>(null);
  const [areaWidth, setAreaWidth] = useState(0);
  useLayoutEffect(() => {
    const root = rootRef.current;
    if (!root) return undefined;
    setAreaWidth(root.clientWidth);
    if (typeof ResizeObserver === 'undefined') return undefined;
    const ro = new ResizeObserver(() => setAreaWidth(root.clientWidth));
    ro.observe(root);
    return () => ro.disconnect();
  }, []);

  // Local width while a drag or a burst of keys is in flight; null = the store's.
  const [liveWidth, setLiveWidth] = useState<number | null>(null);
  const maxWidth = mapPaneMaxWidth(areaWidth, liveWidth ?? storedWidth);
  const width = clampRound(liveWidth ?? storedWidth, MAP_PANE_MIN_WIDTH, maxWidth);

  // Keyboard writes go through a trailing debounce, flushed on unmount (leaving
  // Data inside the window must not drop the width just chosen).
  const keyTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const pendingRef = useRef<number | null>(null);
  const commit = useCallback(
    (next: number) => {
      if (keyTimerRef.current !== null) clearTimeout(keyTimerRef.current);
      keyTimerRef.current = null;
      pendingRef.current = null;
      setMapPaneWidth(next);
      setLiveWidth(null);
    },
    [setMapPaneWidth],
  );
  const commitRef = useRef(commit);
  commitRef.current = commit;
  useEffect(
    () => () => {
      if (keyTimerRef.current !== null) clearTimeout(keyTimerRef.current);
      const pending = pendingRef.current;
      if (pending !== null) useDataWorkspaceStore.getState().setMapPaneWidth(pending);
    },
    [],
  );

  const resize = useResizeHandle({
    direction: 'horizontal',
    // The handle is on the pane's LEFT edge: width = pane right - pointer x.
    handlePosition: 'start',
    minSize: MAP_PANE_MIN_WIDTH,
    maxSize: maxWidth,
    measureRef: paneRef,
    onResize: setLiveWidth,
    onResizeEnd: commit,
  });

  const onKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>) => {
      const step = e.shiftKey ? KEY_STEP_COARSE : KEY_STEP;
      let next: number;
      switch (e.key) {
        // The splitter is the pane's LEFT edge: moving it left widens the map.
        case 'ArrowLeft':
          next = width + step;
          break;
        case 'ArrowRight':
          next = width - step;
          break;
        case 'Home':
          next = MAP_PANE_MIN_WIDTH;
          break;
        case 'End':
          next = maxWidth;
          break;
        default:
          return;
      }
      e.preventDefault();
      next = clampRound(next, MAP_PANE_MIN_WIDTH, maxWidth);
      if (next === width) return;
      setLiveWidth(next);
      pendingRef.current = next;
      if (keyTimerRef.current !== null) clearTimeout(keyTimerRef.current);
      keyTimerRef.current = setTimeout(() => {
        const pending = pendingRef.current;
        if (pending !== null) commitRef.current(pending);
      }, KEY_PERSIST_DELAY_MS);
    },
    [width, maxWidth],
  );

  const resetWidth = useCallback(() => commit(MAP_PANE_DEFAULT_WIDTH), [commit]);

  // One element for the life of the layout: a splitter drag re-renders this
  // component every frame, and an identical element lets React skip the table.
  const workspace = useMemo(
    () => (
      <ErrorBoundary panelName="Data">
        <DataWorkspace />
      </ErrorBoundary>
    ),
    [],
  );

  return (
    <div ref={rootRef} className="absolute inset-0 flex" data-testid="map-area-layout">
      {/* Slot 1: the Data workspace. One wrapper element whatever the pane does, so
          opening or closing the pane restyles it and never remounts the table. */}
      {isData && (
        <div
          className={covered ? 'absolute inset-0 z-30 bg-background' : 'relative h-full min-w-0 flex-1 bg-background'}
          data-testid="data-workspace-slot"
        >
          {workspace}
        </div>
      )}
      {/* Slot 2: the splitter, only between the table and an open pane. The 1px
          rule is the border between them; the handle is a wider transparent hit
          area on top of it, the same shape and highlight as WizardMapPanel's. */}
      {split && (
        <div className="relative shrink-0 w-px h-full bg-border">
          <div
            role="separator"
            aria-orientation="vertical"
            aria-label={t('common.a11y.dataMapPaneResize')}
            aria-valuenow={width}
            aria-valuemin={MAP_PANE_MIN_WIDTH}
            aria-valuemax={maxWidth}
            tabIndex={0}
            onKeyDown={onKeyDown}
            onDoubleClick={resetWidth}
            className={`resize-handle-h absolute top-0 left-1/2 z-40 h-full w-1 -translate-x-1/2 cursor-ew-resize px-1 transition-colors hover:bg-accent-50 focus:outline-none focus-visible:bg-accent-50 focus-visible:ring-1 focus-visible:ring-accent ${
              resize.isResizing ? 'bg-accent-50' : ''
            }`}
            style={{ touchAction: 'none' }}
            data-testid="data-map-pane-splitter"
            {...resize.handleProps}
          />
        </div>
      )}
      {/* Slot 3: the map column - always here, always mounted. In Data with the pane
          closed it keeps its full size under the table (no resize churn) but is
          hidden, so nothing under the table can be focused or painted through. */}
      <div
        ref={paneRef}
        className={`relative h-full min-w-0 flex flex-col ${split ? 'shrink-0' : 'flex-1'} ${covered ? 'invisible' : ''}`}
        style={split ? { width } : undefined}
        aria-hidden={covered || undefined}
        data-testid="map-pane"
      >
        {split && <DataMapPaneHeader />}
        <div className="relative flex-1 min-h-0">{children}</div>
      </div>
    </div>
  );
}
