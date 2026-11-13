/** True in the Android build (sideload-only; see docs/touch-support.md). */
export const isAndroid = typeof navigator !== 'undefined' && navigator.userAgent.includes('Android');
