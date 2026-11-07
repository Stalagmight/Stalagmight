/**
 * Debug-only console helpers.
 *
 * Per-operation diagnostic prints scattered across the codebase used
 * to fire in production builds, where they cost format work and DOM
 * console buffer space on every hover, click, refresh, and import.
 * Route them through these helpers instead - they compile down to a
 * no-op when `import.meta.env.DEV` is false (Vite sets it to `true`
 * only during dev / test).
 *
 * Use `debugLog` for trace-style messages. Keep `console.error` (and
 * `console.warn` for genuinely unexpected runtime conditions) untouched
 * - those are signal users need to see.
 */

const isDev = typeof import.meta !== 'undefined'
  && (import.meta as { env?: { DEV?: boolean } }).env?.DEV === true;

type AnyArgs = readonly unknown[];

export const debugLog: (...args: AnyArgs) => void = isDev
  ? console.log.bind(console)
  : () => {};
