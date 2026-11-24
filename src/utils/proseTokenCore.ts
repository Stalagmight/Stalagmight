/**
 * The prose token's form and the one sanitiser for values put into a prose
 * sentence (RS-PROSE fix 1). No imports, so `i18n/index.ts` can use
 * {@link proseValue} as its interpolation escape without a cycle.
 *
 * A token is U+0001, this page load's random nonce, a colon, the part as
 * URI-encoded JSON, and U+0002. Text a user or a file controls (a comment, a
 * name, a file name) cannot know the nonce, so it can never forge a token;
 * and every value interpolated into a sentence goes through
 * {@link proseValue}, which drops U+0001, U+0002 and the other C0 controls,
 * so a value cannot even hold a marker.
 */

export const OPEN = '\u0001';
export const CLOSE = '\u0002';

function randomNonce(): string {
  const bytes = new Uint8Array(8);
  const c = (globalThis as { crypto?: { getRandomValues?: (a: Uint8Array) => Uint8Array } }).crypto;
  if (c?.getRandomValues) c.getRandomValues(bytes);
  else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256);
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** This page load's nonce: only tokens carrying it are the app's own. */
export const NONCE = randomNonce();

/** Anything between the markers (the nonce is checked by the caller). */
export const TOKEN = new RegExp(`${OPEN}([^${OPEN}${CLOSE}]*)${CLOSE}`, 'g');

/** C0 controls (bar tab, line feed, carriage return) and DEL: never shown, and U+0001/U+0002 mark tokens. */
const CONTROL_CLASS = '[\\u0000-\\u0008\\u000b\\u000c\\u000e-\\u001f\\u007f]';
const CONTROLS = new RegExp(CONTROL_CLASS, 'g');
const HAS_CONTROL = new RegExp(CONTROL_CLASS);

/** `payload` is one of this page's tokens (its nonce prefix). */
export function ownPayload(payload: string): string | null {
  return payload.startsWith(`${NONCE}:`) ? payload.slice(NONCE.length + 1) : null;
}

/**
 * A raw value made safe for a prose sentence: the app's own tokens (this
 * page's nonce) are kept, every other C0 control (bar tab and line breaks)
 * goes, so a value can carry no marker. The i18n escape for every `t()`
 * interpolation, and what a sentence built by hand runs its raw values
 * through (comments, typed or imported names, file names, backend words).
 */
export function proseValue(value: string): string {
  if (!HAS_CONTROL.test(value)) return value;
  let out = '';
  let last = 0;
  for (const match of value.matchAll(TOKEN)) {
    out += value.slice(last, match.index).replace(CONTROLS, '');
    out += ownPayload(match[1] ?? '') !== null ? match[0] : (match[1] ?? '').replace(CONTROLS, '');
    last = match.index + match[0].length;
  }
  return out + value.slice(last).replace(CONTROLS, '');
}

/** Text with every C0 control (bar tab and line breaks) removed: words between tokens. */
export function withoutControls(text: string): string {
  return text.replace(CONTROLS, '');
}
