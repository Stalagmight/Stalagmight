/**
 * "Are these two spellings the same SpeleoDB deployment?" - the frontend half of
 * the identity the backend keys credentials by.
 *
 * `src-tauri/src/speleodb/auth.rs::normalize_endpoint` collapses a trailing
 * slash, host case and a default port so that one server entered several ways
 * resolves to one server identity. Anywhere the UI compares an endpoint it has to
 * collapse the same forms, or it reports a mismatch between two names for the
 * one server the user is already signed in to - and, in the managed-file prompt,
 * refuses to open a canonical copy that is sitting right there.
 *
 * Mirrors the Rust rather than sharing it: this comparison runs before any IPC
 * (that is the point of it), and it is small enough that a round trip to ask the
 * backend would cost more than the duplication. Keep the two in step - the same
 * bargain `emailsMatch` makes with `lock_held_by_me`.
 */
function normalizeEndpoint(endpoint: string): string {
  const trimmed = endpoint.trim();
  try {
    const url = new URL(trimmed);
    // `URL` already drops a port that is its scheme's default, so `:443` on
    // https and the implicit form arrive here identical.
    const scheme = url.protocol.replace(/:$/, '').toLowerCase();
    const port = url.port ? `:${url.port}` : '';
    return `${scheme}://${url.hostname.toLowerCase()}${port}`;
  } catch {
    // A bare host, or anything `URL` will not parse: the same lightweight
    // fallback the Rust takes, rather than an error the caller cannot act on.
    return trimmed.replace(/\/+$/, '').toLowerCase();
  }
}

/** Whether `a` and `b` name the same deployment. */
export function endpointsMatch(
  a: string | null | undefined,
  b: string | null | undefined
): boolean {
  return !!a && !!b && normalizeEndpoint(a) === normalizeEndpoint(b);
}
