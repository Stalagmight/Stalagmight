import type { Map as MaplibreMap } from 'maplibre-gl';
import type * as maplibregl from 'maplibre-gl';
import { GlobalCustomController } from './customRender/globalController';
import { onStyleRestored } from '../Map/surveyLayer/customRender/styleRestore';
import { generateSystemColor, type SystemGeometry } from './systemColor';
import { getMapThemeColors, type MapThemeColors } from '../../utils/themeColors';
import { watchDevicePixelRatio } from '../../utils/dprWatcher';
import { attachLongPressTouch } from '../../utils/longPressTouch';
import { useUIStore } from '../../store/uiStore';
import { hasLiveStyle } from '../Map/mapUtils';
import i18n from '../../i18n';
import { formatFixed, formatNumber } from '../../i18n/format';
import type { DecimalPrecision } from '../../types/config';
import { renderFatalMessage } from '../Map/surveyLayer/mapErrors';
import { TOAST_KEYS } from '../../utils/toastKeys';

// `SystemGeometry` + `generateSystemColor` moved to ./systemColor so the custom
// render buffer builder can reuse them without importing this MapLibre-bound
// layer (import-cycle avoidance). Re-exported here so existing consumers keep
// importing them from './GlobalSurveyLayer'.
export { generateSystemColor } from './systemColor';
export type { SystemGeometry } from './systemColor';

/**
 * Callbacks for GlobalSurveyLayer interactions.
 */
export interface GlobalSurveyLayerCallbacks {
  /** Called when a system is clicked (with ctrlKey flag for multi-select) */
  onSystemClick?: (filePath: string, event: maplibregl.MapMouseEvent, ctrlKey: boolean) => void;
  /** Called when a system is double-clicked (to open the project) */
  onSystemDoubleClick?: (filePath: string) => void;
  /** Called when a system is hovered */
  onSystemHover?: (filePath: string | null) => void;
  /** Called when mouse enters/leaves any feature */
  onFeatureHover?: (isHovering: boolean) => void;
  /** Called when a system is right-clicked (for context menu) */
  onSystemContextMenu?: (filePath: string, event: MouseEvent) => void;
}

/** Scalar tooltip fields carried by a resolved hit (name + length + station count). */
interface TooltipProps {
  name?: string | undefined;
  totalLength?: number | undefined;
  stationCount?: number | undefined;
}

/** Normalize a MapLibre `PointLike` (Point or `[x, y]`) to plain `{x, y}` for
 *  the geometry hit index. */
function pointLikeToXY(point: maplibregl.PointLike): { x: number; y: number } {
  if (Array.isArray(point)) return { x: point[0], y: point[1] };
  return { x: point.x, y: point.y };
}

/**
 * GlobalSurveyLayer renders survey geometry for multiple systems on the global map.
 *
 * This is a read-only visualization layer that shows shots as lines and stations
 * as circles. Each system has a distinct, stable color derived from its file path.
 *
 * The custom WebGL renderer ({@link GlobalCustomController}) is the SOLE renderer:
 * it draws the whole global survey from typed-array buffers and resolves every
 * interaction hit from an index-based geometry hit index (no stock MapLibre
 * layers, sources, or `queryRenderedFeatures`). GlobalSurveyLayer owns the event
 * wiring + tooltip and forwards hover / selection / search / reference /
 * visibility / theme changes to the controller. An unrecoverable GL failure
 * surfaces a user-facing error toast (there is no stock fallback).
 */
export default class GlobalSurveyLayer {
  private map: MaplibreMap;
  private callbacks: GlobalSurveyLayerCallbacks;

  // Visual state (mirrored to the custom renderer). The search term itself is
  // held by the controller (setSearch), not duplicated here.
  private hoveredSystemId: string | null = null;
  private selectedSystemIds: Set<string> = new Set();

  // Per-system colors, kept so getSystemColor / hasData work; the custom
  // renderer resolves its own colors from generateSystemColor.
  private systemColors: Map<string, string> = new Map();

  // We keep no copy of the input systems here: globalViewStore.geometryData
  // remains the single retained copy of inputs, and the custom renderer holds the
  // only layer-owned derived copies (its typed-array GPU buffers).

  // Tooltip element
  private tooltip: HTMLDivElement | null = null;

  // Track if context menu is open (suppress tooltip while open)
  private contextMenuOpen = false;

  // Theme colors for MapLibre paint properties
  // Seeded at construction so first paint uses current theme, not legacy fallbacks
  private themeColors: MapThemeColors = getMapThemeColors();

  // Precision settings for formatting
  private precision: DecimalPrecision | undefined;

  // Custom WebGL renderer (#customRender): the sole global-survey renderer. It
  // draws the whole global survey from typed-array buffers and resolves
  // interaction hits from its geometry index.
  private customRender: GlobalCustomController | null = null;
  /** Dispose for the device-pixel-ratio watcher (custom layer bakes dpr). */
  private dprWatcherDispose: (() => void) | undefined;
  /** Dispose for the re-add after a style rebuild (context / worker restore). */
  private styleRestoreDispose: (() => void) | undefined;

  // Bound event handlers for cleanup
  private boundMouseMove: ((e: maplibregl.MapMouseEvent) => void) | null = null;
  private boundMapLeave: (() => void) | null = null;
  private boundClick: ((e: maplibregl.MapMouseEvent) => void) | null = null;
  private boundDoubleClick: ((e: maplibregl.MapMouseEvent) => void) | null = null;
  private boundContextMenu: ((e: MouseEvent) => void) | null = null;

  // Touch long-press -> context menu. The state machine (500 ms hold, 10 px
  // cancel, swallowed touchend) is shared with SurveyLayer; see
  // src/utils/longPressTouch.ts.
  private detachLongPressTouch: (() => void) | null = null;

  constructor(map: MaplibreMap, callbacks: GlobalSurveyLayerCallbacks = {}) {
    this.map = map;
    this.callbacks = callbacks;
    this.initializeEventHandlers();
    this.initializeTooltip();
    this.customRender = new GlobalCustomController(map, undefined, {
      onFatal: (reason) => this.handleRenderFatal(reason),
    });
    // Rebuild the custom global layer when the DPR changes (window moved to a
    // different-scale monitor); it bakes dpr into device-pixel sizes.
    this.dprWatcherDispose = watchDevicePixelRatio(() => this.customRender?.resync());
    this.styleRestoreDispose = onStyleRestored(map, () => this.customRender?.reattach());
  }

  /**
   * Update the callbacks (e.g., when props change).
   */
  setCallbacks(callbacks: GlobalSurveyLayerCallbacks): void {
    this.callbacks = callbacks;
  }

  /**
   * The custom WebGL renderer hit an unrecoverable GL failure and removed its
   * dead layer. There is no stock fallback, so the global-survey geometry is now
   * blank. Log ungated and surface a user-facing toast (the store is read at call
   * time, so this works from a non-React class).
   */
  private handleRenderFatal(reason: string): void {
    console.error(`[globalCustomRender] survey rendering unavailable: ${reason}`);
    useUIStore.getState().showToast(renderFatalMessage(reason), TOAST_KEYS.RENDER_FATAL, 'error');
  }

  /**
   * Update precision settings for formatting.
   */
  setPrecision(precision: DecimalPrecision): void {
    this.precision = precision;
  }

  /**
   * Resolve the system under a screen point (station priority, then shot) via the
   * custom renderer's index-based geometry hit index. Returns the systemId plus
   * the tooltip scalars, or null for empty space (or when the renderer is
   * inactive / the survey is hidden).
   */
  private pickSystemAt(point: maplibregl.PointLike): { systemId: string; properties: TooltipProps } | null {
    if (!this.customRender) return null;
    const xy = pointLikeToXY(point);
    // Camera key so the hit index reprojects only when the camera moved, not on
    // every hover (a single large system would otherwise re-project all of its
    // endpoints per mousemove). Mirrors SurveyLayer.cameraSnapshot's key.
    const c = this.map.getCenter();
    const cv = this.map.getCanvas();
    const cameraKey = `${this.map.getZoom()}|${c.lng}|${c.lat}|${this.map.getBearing()}|${this.map.getPitch()}|${cv.width}|${cv.height}`;
    const hit = this.customRender.pick((c2) => this.map.project(c2), xy.x, xy.y, cameraKey);
    if (!hit) return null;
    const meta = this.customRender.getSystemMetadata(hit.systemId);
    return {
      systemId: hit.systemId,
      properties: {
        name: meta?.name,
        totalLength: meta?.total_length ?? undefined,
        stationCount: meta?.station_count ?? undefined,
      },
    };
  }

  private initializeEventHandlers(): void {
    // Unified click handler - passes ctrlKey for multi-select
    this.boundClick = (e: maplibregl.MapMouseEvent) => {
      const hit = this.pickSystemAt(e.point);
      if (hit) {
        const ctrlKey = e.originalEvent.ctrlKey || e.originalEvent.metaKey;
        this.callbacks.onSystemClick?.(hit.systemId, e, ctrlKey);
      }
    };
    this.map.on('click', this.boundClick);

    // Double-click handler to open project
    this.boundDoubleClick = (e: maplibregl.MapMouseEvent) => {
      const hit = this.pickSystemAt(e.point);
      if (hit) this.callbacks.onSystemDoubleClick?.(hit.systemId);
    };
    this.map.on('dblclick', this.boundDoubleClick);

    // Unified mousemove handler for hover detection and tooltip
    this.boundMouseMove = (e: maplibregl.MapMouseEvent) => {
      const hit = this.pickSystemAt(e.point);
      const newHoveredSystemId = hit?.systemId ?? null;

      // Update tooltip position and content
      this.updateTooltip(newHoveredSystemId ? e.originalEvent : null, hit?.properties ?? null);

      if (newHoveredSystemId !== this.hoveredSystemId) {
        this.hoveredSystemId = newHoveredSystemId;

        // Update cursor
        if (newHoveredSystemId) {
          this.map.getCanvas().style.cursor = 'pointer';
          this.callbacks.onFeatureHover?.(true);
        } else {
          this.map.getCanvas().style.cursor = '';
          this.callbacks.onFeatureHover?.(false);
        }

        this.callbacks.onSystemHover?.(newHoveredSystemId);
        // Hover is a cheap uniform update on the custom layer.
        this.customRender?.setHover(newHoveredSystemId);
      }
    };
    this.map.on('mousemove', this.boundMouseMove);

    // Clear hover on mouse leave
    this.boundMapLeave = () => {
      if (this.hoveredSystemId) {
        this.hoveredSystemId = null;
        this.map.getCanvas().style.cursor = '';
        this.callbacks.onFeatureHover?.(false);
        this.callbacks.onSystemHover?.(null);
        this.customRender?.setHover(null);
      }
      // Clear context menu flag (menu closes when mouse leaves)
      this.contextMenuOpen = false;
      this.hideTooltip();
    };
    this.map.getCanvas().addEventListener('mouseleave', this.boundMapLeave);

    // Context menu handler for right-click
    this.boundContextMenu = (e: MouseEvent) => {
      // Only handle if callback is set - allows default context menu when no callback
      if (!this.callbacks.onSystemContextMenu) return;

      // Get point from mouse event relative to map container
      const rect = this.map.getContainer().getBoundingClientRect();
      const point: [number, number] = [e.clientX - rect.left, e.clientY - rect.top];
      const hit = this.pickSystemAt(point);
      if (hit) {
        // Prevent default browser context menu when we handle the event
        e.preventDefault();
        // Hide tooltip while context menu is open
        this.contextMenuOpen = true;
        this.hideTooltip();
        this.callbacks.onSystemContextMenu(hit.systemId, e);
      }
    };
    this.map.getCanvas().addEventListener('contextmenu', this.boundContextMenu);

    // Touch long-press → context menu
    const canvas = this.map.getCanvas();
    this.detachLongPressTouch = attachLongPressTouch(canvas, {
      onLongPress: (pressPoint) => {
        if (!this.callbacks.onSystemContextMenu) return false;
        const rect = canvas.getBoundingClientRect();
        const point: [number, number] = [pressPoint.clientX - rect.left, pressPoint.clientY - rect.top];
        // Resolve the system at the touch position (custom hit index).
        const hit = this.pickSystemAt(point);
        // Empty map under the finger: no menu, so nothing to swallow the tap for.
        if (!hit) return false;
        this.contextMenuOpen = true;
        this.hideTooltip();
        const fakeEvent = new MouseEvent('contextmenu', {
          clientX: pressPoint.clientX,
          clientY: pressPoint.clientY,
        });
        this.callbacks.onSystemContextMenu(hit.systemId, fakeEvent);
        return true;
      },
    });
  }

  /**
   * Update the layer with new system geometry data. Feeds the custom renderer,
   * which draws every system and owns interaction hit-testing.
   */
  update(systems: SystemGeometry[]): void {
    // Rebuild the per-system color map (kept for getSystemColor / hasData); the
    // custom renderer resolves its own colors from generateSystemColor.
    this.systemColors.clear();
    for (const system of systems) {
      this.systemColors.set(system.metadata.file_path, generateSystemColor(system.metadata.file_path));
    }

    this.customRender?.sync(systems);
    // Re-resolve hover against the freshly rebuilt system-index map (selection is
    // baked into the rebuilt buffers by sync()).
    this.customRender?.setHover(this.hoveredSystemId);
  }

  /**
   * Set the selected system for highlighting (single selection).
   * For backwards compatibility - calls setSelectedSystems internally.
   */
  setSelectedSystem(filePath: string | null): void {
    const newSet = filePath ? new Set([filePath]) : new Set<string>();
    this.setSelectedSystems(newSet);
  }

  /**
   * Set the selected systems for highlighting (multi-select).
   */
  setSelectedSystems(systemIds: Set<string>): void {
    // Check if actually changed (compare set contents)
    if (
      this.selectedSystemIds.size === systemIds.size &&
      [...this.selectedSystemIds].every((id) => systemIds.has(id))
    ) {
      return;
    }
    this.selectedSystemIds = new Set(systemIds);
    // Mirror to the controller: it re-uploads the baked selection when active and
    // stores it for the next rebuild otherwise.
    this.customRender?.setSelected(this.selectedSystemIds);
  }

  /**
   * Hide reference shots (or show all). Mirrors to the custom layer, which hides
   * reference shots + all-reference stations via a shader uniform and gates them
   * out of the hit index so hidden geometry stays non-interactive.
   */
  setShowReferenceShots(visible: boolean): void {
    this.customRender?.setReferenceShotsVisible(visible);
  }

  /**
   * Set the search filter that dims non-matching systems (case-insensitive
   * substring match on system name). The custom renderer dims them via a
   * per-vertex flag; they remain interactive in the hit index.
   *
   * @param term - Search term to filter by, or null to clear filter
   */
  setSearchFilter(term: string | null): void {
    // Mirror to the custom layer (per-vertex dim re-upload when active; stores
    // the term for the next rebuild otherwise).
    this.customRender?.setSearch(term);
  }

  /**
   * Show or hide the whole global survey (shots + stations). Used by the view
   * mode toggle to switch between survey and markers-only view.
   *
   * @param visible - Whether the survey geometry should be visible
   */
  setVisible(visible: boolean): void {
    // Mirror to the custom layer (whole-survey draw toggle + hit gating).
    this.customRender?.setVisible(visible);
  }

  /**
   * Get the color assigned to a system.
   */
  getSystemColor(filePath: string): string {
    return this.systemColors.get(filePath) ?? generateSystemColor(filePath);
  }

  /**
   * Check if the layer has any data loaded.
   */
  hasData(): boolean {
    return this.systemColors.size > 0;
  }

  /**
   * Apply new theme colors to MapLibre paint properties.
   * Called from GlobalViewMap when resolvedTheme changes.
   */
  applyThemeColors(colors: MapThemeColors): void {
    this.themeColors = colors;

    if (!hasLiveStyle(this.map)) return;

    // Mirror the station stroke color into the custom layer (shader uniform).
    this.customRender?.setStrokeColor(colors.strokeColor);

    // Update tooltip colors
    if (this.tooltip) {
      this.tooltip.style.backgroundColor = colors.previewBg;
      this.tooltip.style.color = colors.labelColor;
    }
  }

  /**
   * Initialize the tooltip element.
   */
  private initializeTooltip(): void {
    this.tooltip = document.createElement('div');
    this.tooltip.className = 'globalview-survey-tooltip';
    const bg = this.themeColors?.previewBg ?? 'rgba(26, 26, 46, 0.95)';
    const fg = this.themeColors?.labelColor ?? 'white';
    this.tooltip.style.cssText = `
      position: absolute;
      pointer-events: none;
      background-color: ${bg};
      color: ${fg};
      padding: 8px 12px;
      border-radius: 4px;
      font-size: 12px;
      line-height: 1.4;
      z-index: 1000;
      white-space: nowrap;
      box-shadow: 0 2px 8px rgba(0,0,0,0.3);
      display: none;
    `;
    // Append to map container
    this.map.getContainer().appendChild(this.tooltip);
  }

  /**
   * Update tooltip position and content.
   */
  private updateTooltip(
    event: MouseEvent | null,
    properties: TooltipProps | null
  ): void {
    if (!this.tooltip) return;

    // Don't show tooltip if context menu is open
    if (this.contextMenuOpen) return;

    if (!event || !properties) {
      this.hideTooltip();
      return;
    }

    // Format total length
    const length = properties.totalLength ?? 0;
    const lengthPrec = this.precision?.length ?? 1;
    let lengthStr: string;
    if (length >= 1000) {
      lengthStr = `${formatFixed(length / 1000, undefined, lengthPrec)} ${i18n.t('common.units.kilometers')}`;
    } else {
      lengthStr = `${formatFixed(length, undefined, lengthPrec)} ${i18n.t('common.units.meters')}`;
    }

    // Build tooltip content using textContent to avoid XSS
    const name = properties.name ?? i18n.t('globalView.tooltip.unknown');
    const stationCount = properties.stationCount ?? 0;

    // Clear existing content
    this.tooltip.textContent = '';

    // Name row
    const nameDiv = document.createElement('div');
    nameDiv.style.cssText = 'font-weight: 600; margin-bottom: 4px;';
    nameDiv.textContent = name;
    this.tooltip.appendChild(nameDiv);

    // Length row
    const grayText = this.themeColors?.grayText ?? '#aaa';
    const lengthDiv = document.createElement('div');
    lengthDiv.style.color = grayText;
    lengthDiv.textContent = i18n.t('globalView.tooltip.length', { length: lengthStr });
    this.tooltip.appendChild(lengthDiv);

    // Station count row
    const stationDiv = document.createElement('div');
    stationDiv.style.color = grayText;
    stationDiv.textContent = i18n.t('globalView.tooltip.stations', {
      count: stationCount,
      countFormatted: formatNumber(stationCount),
    });
    this.tooltip.appendChild(stationDiv);

    // Position tooltip near cursor (offset by 15px)
    const container = this.map.getContainer();
    const rect = container.getBoundingClientRect();
    const x = event.clientX - rect.left + 15;
    const y = event.clientY - rect.top + 15;

    // Temporarily make visible but hidden for measurement
    this.tooltip.style.visibility = 'hidden';
    this.tooltip.style.display = 'block';

    // Measure tooltip dimensions
    const tooltipRect = this.tooltip.getBoundingClientRect();
    const maxX = container.clientWidth - tooltipRect.width - 10;
    const maxY = container.clientHeight - tooltipRect.height - 10;

    // Clamp position to keep tooltip within container bounds (min 0)
    this.tooltip.style.left = `${Math.max(0, Math.min(x, maxX))}px`;
    this.tooltip.style.top = `${Math.max(0, Math.min(y, maxY))}px`;
    this.tooltip.style.visibility = 'visible';
  }

  /**
   * Hide the tooltip.
   */
  private hideTooltip(): void {
    if (this.tooltip) {
      this.tooltip.style.display = 'none';
    }
  }

  /**
   * Destroy the layer, removing all sources and event handlers.
   */
  destroy(): void {
    // Tear down the custom layer (removes it) so it never outlives Global View
    // (#customRender).
    this.customRender?.destroy();
    this.customRender = null;
    this.dprWatcherDispose?.();
    this.dprWatcherDispose = undefined;
    this.styleRestoreDispose?.();
    this.styleRestoreDispose = undefined;

    // Drop the touch long-press listeners and any pending hold
    this.detachLongPressTouch?.();
    this.detachLongPressTouch = null;

    // Remove event handlers
    if (this.boundClick) {
      this.map.off('click', this.boundClick);
    }
    if (this.boundDoubleClick) {
      this.map.off('dblclick', this.boundDoubleClick);
    }
    if (this.boundMouseMove) {
      this.map.off('mousemove', this.boundMouseMove);
    }
    if (this.boundMapLeave) {
      this.map.getCanvas().removeEventListener('mouseleave', this.boundMapLeave);
    }
    if (this.boundContextMenu) {
      this.map.getCanvas().removeEventListener('contextmenu', this.boundContextMenu);
    }

    // Remove tooltip
    if (this.tooltip && this.tooltip.parentNode) {
      this.tooltip.parentNode.removeChild(this.tooltip);
    }
    this.tooltip = null;

    // Clear bound handlers
    this.boundClick = null;
    this.boundDoubleClick = null;
    this.boundMouseMove = null;
    this.boundMapLeave = null;
    this.boundContextMenu = null;
  }
}
