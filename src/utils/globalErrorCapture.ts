/**
 * Capture otherwise-unhandled window errors and promise rejections and route
 * them through console.error, which installConsoleCapture() bridges into the
 * Rust ring buffer. This gives the in-app debug dialog (and the agent test
 * harness's crash oracle) visibility into frontend crashes that would
 * otherwise vanish. The '[unhandled-error]' / '[unhandled-rejection]' prefixes
 * are a stable contract the harness greps for.
 *
 * Always-on: it only calls console.error (no cost outside the error path), so
 * real users get crash context in the debug dialog for free. Coexists with the
 * VITE_DEBUG_MACOS_LOCKUP handlers in debugLockup.ts (addEventListener stacks).
 */

// Rate limit so a render-loop error storm cannot flood the ring buffer / IPC.
const MAX_REPORTS = 30;
const WINDOW_MS = 60_000;

// Latched (#576 D27-32(g)): a second call would add a second pair of listeners
// and report every crash twice.
let installed = false;

export function installGlobalErrorCapture(): void {
  if (typeof window === 'undefined' || installed) return;
  installed = true;

  let stamps: number[] = [];
  const allow = (): boolean => {
    const now = Date.now();
    stamps = stamps.filter((t) => now - t < WINDOW_MS);
    if (stamps.length >= MAX_REPORTS) return false;
    stamps.push(now);
    return true;
  };

  window.addEventListener('error', (event) => {
    if (!allow()) return;
    const src = event.filename ? ` at ${event.filename}:${event.lineno}:${event.colno}` : '';
    const stack = event.error instanceof Error && event.error.stack ? `\n${event.error.stack}` : '';
    console.error(`[unhandled-error] ${event.message}${src}${stack}`);
  });

  window.addEventListener('unhandledrejection', (event) => {
    if (!allow()) return;
    const reason: unknown = event.reason;
    const msg =
      reason instanceof Error ? `${reason.name}: ${reason.message}\n${reason.stack ?? ''}` : String(reason);
    console.error(`[unhandled-rejection] ${msg}`);
  });
}
