/**
 * The longest upload description SpeleoDB can store (sdk#2).
 *
 * SpeleoDB keeps each revision's description in a 1024-character database column,
 * but only AFTER it has committed the upload. A longer description is committed,
 * then fails to store, and every later upload of the project fails the same way.
 * So the upload dialogs stop it here, and the backend refuses it too
 * (`src-tauri/crates/speleodb/src/speleodb/commit_message.rs`; keep the two in step).
 *
 * The length that counts is the STORED one: the server strips the description and
 * turns every run of line breaks, with the whitespace around them, into ". ". A
 * single line break therefore costs two characters. Counted in characters (code
 * points), as the database counts them, not UTF-16 units.
 */

/** Characters (after the server's normalisation) an upload description may hold. */
export const COMMIT_MESSAGE_MAX_CHARS = 1000;

/**
 * Python's `str.isspace` set, which the server's `strip()` and `\s` use: JS `\s`
 * differs (it adds U+FEFF and lacks U+0085 and U+001C..U+001F).
 */
const SERVER_SPACE = '\\t\\n\\v\\f\\r\\u001c-\\u0020\\u0085\\u00a0\\u1680\\u2000-\\u200a\\u2028\\u2029\\u202f\\u205f\\u3000';
const EDGES = new RegExp(`^[${SERVER_SPACE}]+|[${SERVER_SPACE}]+$`, 'g');
const RUNS = new RegExp(`[${SERVER_SPACE}]+`, 'g');

/** The description exactly as the server will store it. */
export function normalizeCommitMessageLikeServer(message: string): string {
  return message.replace(EDGES, '').replace(RUNS, (run) => (run.includes('\n') ? '. ' : run));
}

/** The stored length of `message`, in characters. */
export function commitMessageLength(message: string): number {
  return Array.from(normalizeCommitMessageLikeServer(message)).length;
}

/** Longer than SpeleoDB can store. */
export function commitMessageTooLong(message: string): boolean {
  return commitMessageLength(message) > COMMIT_MESSAGE_MAX_CHARS;
}
