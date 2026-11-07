// Optional debug instrumentation. Inert unless VITE_DEBUG_MACOS_LOCKUP=true
// at build time. Imported first so its side-effects can wire up before any
// other module runs. See docs/debugging-macos-pan-zoom.md.
import './utils/debugLockup';
import React from 'react';
import ReactDOM from 'react-dom/client';
import App from './App';
import './i18n';
import './index.css';
import { installConsoleCapture } from './utils/consoleCapture';
import { installGlobalErrorCapture } from './utils/globalErrorCapture';
import { initInputModalityTracking } from './utils/inputModality';
import { useUIStore } from './store/uiStore';
import { useProjectStore, type LoadingState } from './store/projectStore';
import { useGlobalViewStore } from './store/globalViewStore';
import { useGasPlanStore } from './store/gasPlanStore';
import { useSpeleoStore } from './store/speleoStore';
import { useExportStore } from './store/exportStore';
import { useNetworkStore } from './store/networkStore';
import { useUpdateStore } from './store/updateStore';
import { shotMaterialization } from './types/survey-data';
import { getPerfTimings } from './utils/memoryProbe';
import { routeOpenedFile } from './utils/openOrImport';
import type { TestFileDialogQueueEntry } from './utils/fileDialogs';

// Pipe frontend console.* calls into the Rust ring buffer for the debug dialog
installConsoleCapture();

// Dev-only agent bridge: mirror the console and accept eval requests over the
// Vite dev server so an outside process can drive the webview. Dynamically
// imported behind the DEV constant so it tree-shakes out of production builds,
// and installed AFTER installConsoleCapture() so its console patch wraps that
// one (both fire). See docs/debug-gates.md and scripts/lib/vite-agent-bridge.mjs.
// NOT under an automation harness (#417): a Playwright page never
// authenticates to the bridge, so its token probe just 403s (up to 8 retries)
// - and specs that assert a clean console red on those resource errors, but
// only on dev-server runs (CI serves the built bundle, where this DEV block
// tree-shakes away entirely). That local-red/CI-green split was #417.
// `navigator.webdriver` is the automation signal because the local Playwright
// path runs plain `vite` WITHOUT VITE_E2E (the store hooks ride on DEV there);
// the real Tauri webview never sets it, so manual dev keeps the bridge.
if (import.meta.env.DEV && navigator.webdriver !== true) {
  void import('./dev/agentBridge')
    .then((m) => m.installAgentBridge())
    .catch(() => {});
}

// Route unhandled window errors / promise rejections into the same ring buffer
installGlobalErrorCapture();

// Keyboard-only focus rings: tag <html data-input-modality> from the first
// interaction (set before render so there's no initial ring flash).
initInputModalityTracking();

// The frame-gap freeze detector is no longer started automatically - it
// was producing a stream of `freeze #N` entries in the memory probe for
// every minor stall, drowning out the signal. Open the probe panel
// (Display ▸ Dev stats) - it starts the detector on demand from its own effect.

// Expose stores for E2E testing
declare global {
  interface Window {
    __E2E_TEST__?: {
      uiStore?: typeof useUIStore;
      projectStore?: typeof useProjectStore;
      globalViewStore?: typeof useGlobalViewStore;
      gasPlanStore?: typeof useGasPlanStore;
      speleoStore?: typeof useSpeleoStore;
      exportStore?: typeof useExportStore;
      networkStore?: typeof useNetworkStore;
      updateStore?: typeof useUpdateStore;
      // Loading progress E2E hooks
      getLoadingId?: () => number;
      getBackendLoadingId?: () => bigint;
      simulateLoadingProgress?: (payload: Partial<LoadingState>) => void;
      simulateLoadingError?: (message: string, details?: string) => void;
      resetLoading?: () => void;
      fileDialogAnswers?: {
        open: TestFileDialogQueueEntry[];
        save: TestFileDialogQueueEntry[];
      };
      fileDialogConsumptions?: Record<string, 'open' | 'save'>;
      activeFileDialogTriggerTestId?: string;
      /**
       * The open/import ROUTER, by path (`src/utils/openOrImport.ts`).
       *
       * Every production entry point - toolbar, Ctrl+I/Ctrl+O, drag-drop,
       * launch-with-file - funnels into it, but only two of them are reachable
       * from a driver: the pickers (which need an armed file-dialog answer AND a
       * clickable trigger) and a real OS drag-drop. Neither works while an
       * import already owns the screen, which is exactly the state
       * `routeOpenedFile`'s own `importFlowOwnsScreen` guard exists to refuse -
       * so the guard had no test path at all. This exposes the router itself,
       * under the same E2E gate as the stores below (which already expose
       * `loadProject`/`saveProject`, a strictly larger capability).
       */
      routeOpenedFile?: (path: string, errorTitle?: string) => Promise<void>;
    };
  }
}

// Expose stores for E2E testing (dev/test mode or explicit E2E build flag)
// Guarded to prevent shipping internal store access to production builds
// Uses additive pattern to avoid clobbering other test helpers
if (import.meta.env.DEV || import.meta.env.MODE === 'test' || import.meta.env.VITE_E2E === '1') {
  const testAPI = window.__E2E_TEST__ || {};
  testAPI.uiStore = useUIStore;
  testAPI.projectStore = useProjectStore;
  testAPI.globalViewStore = useGlobalViewStore;
  testAPI.gasPlanStore = useGasPlanStore;
  testAPI.speleoStore = useSpeleoStore;
  testAPI.exportStore = useExportStore;
  testAPI.networkStore = useNetworkStore;
  testAPI.updateStore = useUpdateStore;
  testAPI.fileDialogAnswers ??= { open: [], save: [] };
  testAPI.fileDialogConsumptions ??= {};
  // See the declaration above for why the router needs its own hook.
  testAPI.routeOpenedFile = (path: string, errorTitle?: string) =>
    routeOpenedFile(path, errorTitle ?? 'Import Failed');
  window.__E2E_TEST__ = testAPI;

  // Lazy-materialization guard for the load path: the harness reads this after
  // a settled load and asserts it stayed 0.
  // A live getter so `window.__shotMaterializations` always reflects the
  // current count; `window.__resetShotMaterializations()` zeroes it.
  Object.defineProperty(window, '__shotMaterializations', {
    configurable: true,
    get: () => shotMaterialization.count,
  });
  (window as unknown as { __resetShotMaterializations?: () => void }).__resetShotMaterializations =
    () => {
      shotMaterialization.count = 0;
    };

  // Perf-timings snapshot for the harness (#mem verification): lets a test read
  // the most recent recordPerfTiming() entries (e.g. feature-build and
  // per-shot-mode granularity-swap costs) without opening the memory probe.
  (window as unknown as { __getPerfTimings?: () => Record<string, { ms: number; ts: number }> }).__getPerfTimings =
    () => getPerfTimings();
}

// Expose loading hooks for E2E tests (dev/test mode or explicit E2E build flag)
// IMPORTANT: Extend existing hooks, don't overwrite
if (import.meta.env.DEV || import.meta.env.MODE === 'test' || import.meta.env.VITE_E2E === '1') {
  const existingHooks = window.__E2E_TEST__ || {};
  window.__E2E_TEST__ = {
    ...existingHooks, // PRESERVE existing uiStore, projectStore, etc.
    // Use clientOpId for reliable tracking - loadingId may be 0 until first event
    getLoadingId: () => useProjectStore.getState().loading.clientOpId,
    getBackendLoadingId: () => useProjectStore.getState().loading.loadingId,
    simulateLoadingProgress: (payload: Partial<LoadingState>) => {
      useProjectStore.getState().setLoadingProgress(payload);
    },
    simulateLoadingError: (message: string, details?: string) => {
      useProjectStore.getState().setLoadingError(message, details);
    },
    resetLoading: () => useProjectStore.getState().resetLoading(),
  };
}

const rootElement = document.getElementById('root');
if (!rootElement) {
  throw new Error('Root element not found');
}

ReactDOM.createRoot(rootElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
);
