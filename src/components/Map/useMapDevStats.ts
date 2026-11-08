import { useEffect, useState, useRef, useCallback } from 'react';
import type { TFunction } from 'i18next';
import { invokeIpc } from '../../utils/ipc';
import * as maplibregl from 'maplibre-gl';
import { getTileFetchStats, type TileFetchStats } from '../../utils/tileProtocol';
import { formatFixed, formatNumber } from '../../i18n/format';
import { useNetworkStore } from '../../store/networkStore';

export interface CacheStats {
  total_tiles: bigint;
  total_bytes: bigint;
  providers: Record<string, { tiles: bigint; bytes: bigint }>;
}

export interface RenderProfile {
  drawCalls: number;
  triangles: number;
  renderMs: number;
  layers: { total: number; visible: number; byType: Record<string, number> };
  sources: Array<{ name: string; features: number }>;
}

export interface WebGLInfo {
  renderer: string;
  vendor: string;
  webglVersion: string;
  // GPU limits (static, cached on mount)
  maxTextureSize: number;
  maxTextureUnits: number;
  maxVertexAttribs: number;
  maxViewportDims: [number, number];
  // Color depth
  colorBits: { r: number; g: number; b: number; a: number };
  depthBits: number;
  stencilBits: number;
  // Anti-aliasing
  samples: number;
}

export interface MapDevStatsData {
  // Performance
  fps: number;
  frameTime: number;
  isZooming: boolean;
  renderProfile: RenderProfile | null;

  // Zoom performance (last completed zoom gesture)
  zoomFps: number | null;
  lastZoomDuration: number | null;
  lastZoomLevels: number | null;
  worstFrameTime: number | null;

  // Map / tile state
  zoom: number;
  center: maplibregl.LngLat;
  canvasSize: { width: number; height: number };
  mapLoaded: boolean;
  tilesLoaded: boolean;

  // WebGL, network, cache
  webglInfo: WebGLInfo | null;
  networkStatus: boolean;
  tileFetchStats: TileFetchStats | null;
  cacheStats: CacheStats | null;

  // Debug toggles (MapLibre built-in)
  showTileBoundaries: boolean;
  showCollisionBoxes: boolean;
  showOverdraw: boolean;
  hideSymbols: boolean;
  hideAllLayers: boolean;
  toggleTileBoundaries: () => void;
  toggleCollisionBoxes: () => void;
  toggleOverdraw: () => void;
  toggleSymbols: () => void;
  toggleAllLayers: () => void;
}

/**
 * Instruments WebGL draw calls by wrapping drawArrays/drawElements on the context.
 * Returns a controller to read per-frame counters and clean up.
 */
function createDrawCallCounter(canvas: HTMLCanvasElement) {
  const gl = (canvas.getContext('webgl2') || canvas.getContext('webgl')) as WebGL2RenderingContext | null;
  if (!gl) return null;

  let drawCalls = 0;
  let triangles = 0;
  let lines = 0;
  let points = 0;

  // Save originals
  const origDrawArrays = gl.drawArrays.bind(gl);
  const origDrawElements = gl.drawElements.bind(gl);
  const origDrawArraysInstanced = gl.drawArraysInstanced?.bind(gl);
  const origDrawElementsInstanced = gl.drawElementsInstanced?.bind(gl);

  const countPrimitives = (mode: number, count: number, instances = 1) => {
    drawCalls++;
    const total = count * instances;
    if (mode === gl.TRIANGLES) triangles += total / 3;
    else if (mode === gl.TRIANGLE_STRIP || mode === gl.TRIANGLE_FAN) triangles += Math.max(0, total - 2);
    else if (mode === gl.LINES) lines += total / 2;
    else if (mode === gl.LINE_STRIP) lines += Math.max(0, total - 1);
    else if (mode === gl.POINTS) points += total;
  };

  gl.drawArrays = function (mode: number, first: number, count: number) {
    countPrimitives(mode, count);
    return origDrawArrays(mode, first, count);
  };

  gl.drawElements = function (mode: number, count: number, type: number, offset: number) {
    countPrimitives(mode, count);
    return origDrawElements(mode, count, type, offset);
  };

  if (origDrawArraysInstanced) {
    gl.drawArraysInstanced = function (mode: number, first: number, count: number, instanceCount: number) {
      countPrimitives(mode, count, instanceCount);
      return origDrawArraysInstanced(mode, first, count, instanceCount);
    };
  }

  if (origDrawElementsInstanced) {
    gl.drawElementsInstanced = function (mode: number, count: number, type: number, offset: number, instanceCount: number) {
      countPrimitives(mode, count, instanceCount);
      return origDrawElementsInstanced(mode, count, type, offset, instanceCount);
    };
  }

  return {
    /** Read and reset counters (call once per frame) */
    flush: () => {
      const result = { drawCalls, triangles: Math.round(triangles), lines: Math.round(lines), points: Math.round(points) };
      drawCalls = 0;
      triangles = 0;
      lines = 0;
      points = 0;
      return result;
    },
    /** Restore original WebGL methods */
    dispose: () => {
      gl.drawArrays = origDrawArrays;
      gl.drawElements = origDrawElements;
      if (origDrawArraysInstanced) gl.drawArraysInstanced = origDrawArraysInstanced;
      if (origDrawElementsInstanced) gl.drawElementsInstanced = origDrawElementsInstanced;
    },
  };
}

/**
 * Collects the data behind the Dev Stats / Map Stats / Zoom Stats debug
 * panels (see DevStatsPanel/MapStatsPanel/ZoomStatsPanel). Installs a WebGL
 * draw-call counter and polls cache/tile-fetch stats over IPC, so it should
 * only be mounted while at least one of those panels is visible.
 *
 * References:
 * - https://maplibre.org/maplibre-gl-js/docs/API/classes/Map/
 * - https://deepwiki.com/maplibre/maplibre-gl-js/5-build-system-and-performance
 */
export function useMapDevStats(map: maplibregl.Map): MapDevStatsData {
  // FPS tracking
  const [fps, setFps] = useState(0);
  const [frameTime, setFrameTime] = useState(0);
  const frameTimesRef = useRef<number[]>([]);
  const lastFrameTimeRef = useRef(performance.now());

  // Cache and network stats
  const [cacheStats, setCacheStats] = useState<CacheStats | null>(null);
  const networkStatus = useNetworkStore((s) => s.isOnline);
  const [tileFetchStats, setTileFetchStats] = useState<TileFetchStats | null>(null);

  // Map state - zoom/center stored in refs and flushed to React state periodically
  // to avoid triggering expensive re-renders on every pan frame
  const [zoom, setZoom] = useState(map.getZoom());
  const [center, setCenter] = useState(map.getCenter());
  const zoomRef = useRef(map.getZoom());
  const centerRef = useRef(map.getCenter());
  const [mapLoaded, setMapLoaded] = useState(map.loaded());
  const [tilesLoaded, setTilesLoaded] = useState(map.areTilesLoaded());

  // WebGL info
  const [webglInfo, setWebglInfo] = useState<WebGLInfo | null>(null);
  const [canvasSize, setCanvasSize] = useState({ width: 0, height: 0 });

  // Debug toggles (MapLibre built-in)
  const [showTileBoundaries, setShowTileBoundaries] = useState(map.showTileBoundaries);
  const [showCollisionBoxes, setShowCollisionBoxes] = useState(map.showCollisionBoxes);
  const [showOverdraw, setShowOverdraw] = useState(map.showOverdrawInspector);
  const [hideSymbols, setHideSymbols] = useState(false);

  // Zoom performance tracking
  const [isZooming, setIsZooming] = useState(false);
  const [zoomFps, setZoomFps] = useState<number | null>(null);
  const [lastZoomDuration, setLastZoomDuration] = useState<number | null>(null);
  const [lastZoomLevels, setLastZoomLevels] = useState<number | null>(null);
  const [worstFrameTime, setWorstFrameTime] = useState<number | null>(null);
  const zoomStartTimeRef = useRef<number | null>(null);
  const zoomStartLevelRef = useRef<number | null>(null);
  const zoomFrameTimesRef = useRef<number[]>([]);
  const worstFrameRef = useRef<number>(0);

  // Render profiling
  const [renderProfile, setRenderProfile] = useState<RenderProfile | null>(null);
  const drawCounterRef = useRef<ReturnType<typeof createDrawCallCounter>>(null);
  // Accumulate draw stats across sub-frames; flush to React state periodically
  const pendingDrawsRef = useRef({ drawCalls: 0, triangles: 0 });

  // Install WebGL draw call counter on mount
  useEffect(() => {
    const counter = createDrawCallCounter(map.getCanvas());
    drawCounterRef.current = counter;
    return () => {
      drawCounterRef.current?.dispose();
      drawCounterRef.current = null;
    };
  }, [map]);

  // Collect draw calls and render stats; update React state at ~2 Hz to avoid churn
  // PERF NOTE: querySourceFeatures is expensive (20-40ms) - only run on idle, not on render.
  // The render event handler only flushes draw call counters (cheap).
  useEffect(() => {
    let lastProfileUpdate = 0;
    const PROFILE_INTERVAL = 500; // ms - match the FPS update cadence

    const handleRender = () => {
      // Flush per-frame draw call counters (accumulated since last flush)
      const counter = drawCounterRef.current;
      if (counter) {
        const draws = counter.flush();
        pendingDrawsRef.current = draws; // Overwrite with latest frame's counts
      }
    };

    // Collect stats on idle only (NOT on every render - avoids feedback loop)
    let idleRafId = 0;
    const scheduleIdleUpdate = () => {
      cancelAnimationFrame(idleRafId);
      idleRafId = requestAnimationFrame(() => {
        const now = performance.now();
        if (now - lastProfileUpdate < PROFILE_INTERVAL) return;
        lastProfileUpdate = now;

        const draws = pendingDrawsRef.current;

        // Collect layer stats
        const style = map.getStyle();
        const allLayers = style?.layers ?? [];
        let visible = 0;
        const byType: Record<string, number> = {};
        for (const layer of allLayers) {
          const vis = map.getLayoutProperty(layer.id, 'visibility');
          if (vis !== 'none') visible++;
          byType[layer.type] = (byType[layer.type] ?? 0) + 1;
        }

        // Collect source feature counts via querySourceFeatures (official API).
        // This can be expensive (~20-40ms) but we only run on idle with 500ms throttle.
        const sources: Array<{ name: string; features: number }> = [];
        if (style?.sources) {
          for (const [name, spec] of Object.entries(style.sources)) {
            const s = spec as { type: string };
            if (s.type === 'geojson') {
              try {
                const features = map.querySourceFeatures(name);
                sources.push({ name: name.replace('survey-', ''), features: features.length });
              } catch {
                // Source may not be loaded yet
              }
            }
          }
        }

        setRenderProfile({
          ...draws,
          renderMs: frameTimesRef.current.length > 0
            ? Math.round((frameTimesRef.current.reduce((a, b) => a + b, 0) / frameTimesRef.current.length) * 100) / 100
            : 0,
          layers: { total: allLayers.length, visible, byType },
          sources,
        });
      });
    };

    map.on('render', handleRender);
    map.on('idle', scheduleIdleUpdate);

    return () => {
      map.off('render', handleRender);
      map.off('idle', scheduleIdleUpdate);
      cancelAnimationFrame(idleRafId);
    };
  }, [map]);

  // Get WebGL info on mount (cached - these don't change)
  useEffect(() => {
    const canvas = map.getCanvas();
    const gl = canvas.getContext('webgl2') || canvas.getContext('webgl');

    if (gl) {
      const debugInfo = gl.getExtension('WEBGL_debug_renderer_info');
      const renderer = debugInfo
        ? gl.getParameter(debugInfo.UNMASKED_RENDERER_WEBGL)
        : gl.getParameter(gl.RENDERER);
      const vendor = debugInfo
        ? gl.getParameter(debugInfo.UNMASKED_VENDOR_WEBGL)
        : gl.getParameter(gl.VENDOR);
      const webglVersion = gl instanceof WebGL2RenderingContext ? 'WebGL2' : 'WebGL1';

      // GPU limits (static values)
      const maxTextureSize = gl.getParameter(gl.MAX_TEXTURE_SIZE) as number;
      const maxTextureUnits = gl.getParameter(gl.MAX_COMBINED_TEXTURE_IMAGE_UNITS) as number;
      const maxVertexAttribs = gl.getParameter(gl.MAX_VERTEX_ATTRIBS) as number;
      const viewportDims = gl.getParameter(gl.MAX_VIEWPORT_DIMS) as Int32Array;

      // Color/depth bits
      const colorBits = {
        r: gl.getParameter(gl.RED_BITS) as number,
        g: gl.getParameter(gl.GREEN_BITS) as number,
        b: gl.getParameter(gl.BLUE_BITS) as number,
        a: gl.getParameter(gl.ALPHA_BITS) as number,
      };
      const depthBits = gl.getParameter(gl.DEPTH_BITS) as number;
      const stencilBits = gl.getParameter(gl.STENCIL_BITS) as number;

      // Anti-aliasing samples
      const samples = gl.getParameter(gl.SAMPLES) as number;

      setWebglInfo({
        renderer,
        vendor,
        webglVersion,
        maxTextureSize,
        maxTextureUnits,
        maxVertexAttribs,
        maxViewportDims: [viewportDims[0] ?? 0, viewportDims[1] ?? 0],
        colorBits,
        depthBits,
        stencilBits,
        samples,
      });
    }

    // Get initial canvas size
    setCanvasSize({ width: canvas.width, height: canvas.height });
  }, [map]);

  // FPS measurement using requestAnimationFrame
  // Only update React state every 500ms to avoid triggering re-renders every frame
  const lastStateUpdateRef = useRef(performance.now());
  const STATE_UPDATE_INTERVAL = 500; // ms
  const isZoomingRef = useRef(false); // Non-reactive ref for frame loop
  const animationFrameRef = useRef<number | null>(null);

  useEffect(() => {
    const measureFrame = () => {
      const now = performance.now();
      const delta = now - lastFrameTimeRef.current;
      lastFrameTimeRef.current = now;

      // Store frame times for averaging
      if (delta < 2000) {
        frameTimesRef.current.push(delta);
        if (frameTimesRef.current.length > 60) {
          frameTimesRef.current.shift();
        }
      }

      // Track frame times during zoom
      if (isZoomingRef.current) {
        zoomFrameTimesRef.current.push(delta);
        if (delta > worstFrameRef.current) {
          worstFrameRef.current = delta;
        }
      }

      // Only update React state periodically to avoid re-render every frame
      if (now - lastStateUpdateRef.current >= STATE_UPDATE_INTERVAL) {
        lastStateUpdateRef.current = now;
        if (frameTimesRef.current.length > 0) {
          const avgFrameTime =
            frameTimesRef.current.reduce((a, b) => a + b, 0) / frameTimesRef.current.length;
          setFrameTime(Math.round(avgFrameTime * 100) / 100);
          setFps(Math.round(1000 / avgFrameTime));
        }
        // Flush zoom/center from refs (written by move handler every frame)
        setZoom(zoomRef.current);
        setCenter(centerRef.current);
      }

      animationFrameRef.current = requestAnimationFrame(measureFrame);
    };

    animationFrameRef.current = requestAnimationFrame(measureFrame);

    return () => {
      if (animationFrameRef.current) {
        cancelAnimationFrame(animationFrameRef.current);
      }
    };
  }, []);

  // Fetch cache stats periodically
  useEffect(() => {
    const fetchStats = async () => {
      try {
        const stats = await invokeIpc<CacheStats>('get_cache_stats', undefined, { purpose: 'fetch tile cache stats' });
        setCacheStats(stats);
      } catch (e) {
        console.debug('[useMapDevStats] Failed to fetch cache stats:', e);
      }
    };

    // Initial fetch
    fetchStats();

    // Refresh every 2 seconds
    const interval = setInterval(fetchStats, 2000);

    return () => clearInterval(interval);
  }, []);

  // Track map state changes
  useEffect(() => {
    const handleMove = () => {
      // Write to refs only - flushed to React state in the periodic update loop
      zoomRef.current = map.getZoom();
      centerRef.current = map.getCenter();
    };

    const handleLoad = () => {
      setMapLoaded(true);
    };

    // Throttle sourcedata handler - it fires very frequently during tile loading
    let sourcedataTimeout: ReturnType<typeof setTimeout> | null = null;
    const handleSourceData = () => {
      if (sourcedataTimeout) return; // Skip if already scheduled
      sourcedataTimeout = setTimeout(() => {
        sourcedataTimeout = null;
        setTilesLoaded(map.areTilesLoaded());
      }, 250);
    };

    // Idle event fires when map finishes rendering - ensures we catch final state
    const handleIdle = () => {
      setMapLoaded(map.loaded());
      setTilesLoaded(map.areTilesLoaded());
    };

    const handleResize = () => {
      const canvas = map.getCanvas();
      setCanvasSize({ width: canvas.width, height: canvas.height });
    };

    // Zoom performance tracking
    const handleZoomStart = () => {
      zoomStartTimeRef.current = performance.now();
      zoomStartLevelRef.current = map.getZoom();
      zoomFrameTimesRef.current = [];
      worstFrameRef.current = 0;
      isZoomingRef.current = true;
      setIsZooming(true);
    };

    const handleZoomEnd = () => {
      isZoomingRef.current = false;
      setIsZooming(false);

      if (zoomStartTimeRef.current !== null) {
        const duration = performance.now() - zoomStartTimeRef.current;
        setLastZoomDuration(Math.round(duration));
      }

      if (zoomStartLevelRef.current !== null) {
        const levels = Math.abs(map.getZoom() - zoomStartLevelRef.current);
        setLastZoomLevels(Math.round(levels * 100) / 100);
      }

      // Calculate average FPS during zoom
      if (zoomFrameTimesRef.current.length > 0) {
        const avgFrameTime =
          zoomFrameTimesRef.current.reduce((a, b) => a + b, 0) / zoomFrameTimesRef.current.length;
        setZoomFps(Math.round(1000 / avgFrameTime));
      }

      // Record worst frame
      if (worstFrameRef.current > 0) {
        setWorstFrameTime(Math.round(worstFrameRef.current));
      }

      zoomStartTimeRef.current = null;
      zoomStartLevelRef.current = null;
    };

    map.on('move', handleMove);
    map.on('load', handleLoad);
    map.on('sourcedata', handleSourceData);
    map.on('resize', handleResize);
    map.on('zoomstart', handleZoomStart);
    map.on('zoomend', handleZoomEnd);
    map.on('idle', handleIdle);

    return () => {
      map.off('move', handleMove);
      map.off('load', handleLoad);
      map.off('sourcedata', handleSourceData);
      map.off('resize', handleResize);
      map.off('zoomstart', handleZoomStart);
      map.off('zoomend', handleZoomEnd);
      map.off('idle', handleIdle);
      if (sourcedataTimeout) clearTimeout(sourcedataTimeout);
    };
  }, [map]);

  // Poll tile fetch stats periodically via IPC
  useEffect(() => {
    const fetchTileStats = async () => {
      try {
        const stats = await getTileFetchStats();
        setTileFetchStats(stats);
      } catch (e) {
        console.debug('[useMapDevStats] Failed to fetch tile stats:', e);
      }
    };

    // Initial fetch
    fetchTileStats();

    // Refresh every 2 seconds (matches cache stats cadence, avoids IPC overhead)
    const interval = setInterval(fetchTileStats, 2000);

    return () => clearInterval(interval);
  }, []);

  // Toggle handlers for debug options
  const toggleTileBoundaries = useCallback(() => {
    const newValue = !map.showTileBoundaries;
    map.showTileBoundaries = newValue;
    setShowTileBoundaries(newValue);
  }, [map]);

  const toggleCollisionBoxes = useCallback(() => {
    const newValue = !map.showCollisionBoxes;
    map.showCollisionBoxes = newValue;
    setShowCollisionBoxes(newValue);
  }, [map]);

  const toggleOverdraw = useCallback(() => {
    const newValue = !map.showOverdrawInspector;
    map.showOverdrawInspector = newValue;
    setShowOverdraw(newValue);
  }, [map]);

  // Toggle visibility of all symbol layers (labels) for debugging
  const toggleSymbols = useCallback(() => {
    const newValue = !hideSymbols;
    const style = map.getStyle();
    if (style?.layers) {
      for (const layer of style.layers) {
        if (layer.type === 'symbol') {
          map.setLayoutProperty(layer.id, 'visibility', newValue ? 'none' : 'visible');
        }
      }
    }
    setHideSymbols(newValue);
  }, [map, hideSymbols]);

  // Hide ALL layers except background for ultimate performance test
  const [hideAllLayers, setHideAllLayers] = useState(false);
  const toggleAllLayers = useCallback(() => {
    const newValue = !hideAllLayers;
    const style = map.getStyle();
    if (style?.layers) {
      for (const layer of style.layers) {
        if (layer.id !== 'background') {
          map.setLayoutProperty(layer.id, 'visibility', newValue ? 'none' : 'visible');
        }
      }
    }
    setHideAllLayers(newValue);
  }, [map, hideAllLayers]);

  return {
    fps,
    frameTime,
    isZooming,
    renderProfile,
    zoomFps,
    lastZoomDuration,
    lastZoomLevels,
    worstFrameTime,
    zoom,
    center,
    canvasSize,
    mapLoaded,
    tilesLoaded,
    webglInfo,
    networkStatus,
    tileFetchStats,
    cacheStats,
    showTileBoundaries,
    showCollisionBoxes,
    showOverdraw,
    hideSymbols,
    hideAllLayers,
    toggleTileBoundaries,
    toggleCollisionBoxes,
    toggleOverdraw,
    toggleSymbols,
    toggleAllLayers,
  };
}

/** Format a byte count as a human-readable string (B/KB/MB/GB), i18n-aware. */
export function formatBytes(t: TFunction, bytes: number | bigint): string {
  const value = typeof bytes === 'bigint' ? Number(bytes) : bytes;
  const sizes = [
    t('map.devStats.units.bytes'),
    t('map.devStats.units.kilobytes'),
    t('map.devStats.units.megabytes'),
    t('map.devStats.units.gigabytes'),
  ];
  if (value === 0) {
    return t('map.devStats.bytesValue', {
      value: formatNumber(0),
      unit: sizes[0],
    });
  }
  const k = 1024;
  const i = Math.floor(Math.log(value) / Math.log(k));
  return t('map.devStats.bytesValue', {
    value: formatFixed(value / Math.pow(k, i), undefined, 1),
    unit: sizes[i] ?? sizes[sizes.length - 1],
  });
}

/** Color-code an FPS value: green >= 55, amber >= 30, red below. */
export function getFpsColor(currentFps: number): string {
  if (currentFps >= 55) return 'text-success';
  if (currentFps >= 30) return 'text-warning';
  return 'text-error';
}

/** Network status indicator (color + label), i18n-aware. */
export function getNetworkIndicator(t: TFunction, online: boolean): { color: string; text: string } {
  if (online) {
    return { color: 'text-success', text: t('map.devStats.network.online') };
  }
  return { color: 'text-error', text: t('map.devStats.network.offline') };
}
