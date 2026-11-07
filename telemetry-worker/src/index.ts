/**
 * Stalagmight Telemetry Worker
 *
 * Receives anonymous usage event batches from the desktop app and stores
 * them in a D1 database. IP addresses are SHA-256 hashed for abuse
 * detection only - no raw IPs or other PII are stored.
 *
 * Wire format: self-describing JSON (event names sent verbatim), gzip-encoded
 * (`Content-Type: application/json`, `Content-Encoding: gzip`). Uncompressed
 * JSON is also accepted, e.g. for manual testing.
 */

const MAX_BODY_BYTES = 65_536; // 64 KB (compressed body)
const MAX_DECOMPRESSED_BYTES = 262_144; // 256 KB - gzip-bomb guard
const RATE_LIMIT_MAX_FLUSHES_PER_MIN = 10;

interface Env {
  DB: D1Database;
}

interface TelemetryPayload {
  v: number;
  app_version: string;
  os: string;
  session_id: string;
  duration_secs: number;
  project_seq?: number;
  events: Record<string, number>;
}

function isValidJsonPayload(body: unknown): body is TelemetryPayload {
  if (typeof body !== 'object' || body === null) return false;
  const p = body as Record<string, unknown>;
  if (
    typeof p.app_version !== 'string' ||
    typeof p.os !== 'string' ||
    typeof p.session_id !== 'string' ||
    typeof p.duration_secs !== 'number' ||
    (p.project_seq !== undefined && typeof p.project_seq !== 'number') ||
    typeof p.events !== 'object' ||
    p.events === null
  ) {
    return false;
  }
  // events must be a flat { name: count } map
  for (const count of Object.values(p.events as Record<string, unknown>)) {
    if (typeof count !== 'number') return false;
  }
  return true;
}

/** Read the request body, decompressing gzip with a hard size cap. */
async function readBody(request: Request): Promise<Uint8Array> {
  const compressed = new Uint8Array(await request.arrayBuffer());
  if (request.headers.get('content-encoding') !== 'gzip') {
    return compressed;
  }
  const stream = new Response(compressed).body!.pipeThrough(new DecompressionStream('gzip'));
  const reader = stream.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > MAX_DECOMPRESSED_BYTES) {
      await reader.cancel();
      throw new Error('decompressed payload too large');
    }
    chunks.push(value);
  }
  const out = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    out.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return out;
}

/** Parse the self-describing JSON payload (gzip-encoded or raw). */
async function parseRequest(request: Request): Promise<TelemetryPayload | null> {
  let bytes: Uint8Array;
  try {
    bytes = await readBody(request);
  } catch {
    return null;
  }
  let raw: unknown;
  try {
    raw = JSON.parse(new TextDecoder().decode(bytes));
  } catch {
    return null;
  }
  return isValidJsonPayload(raw) ? raw : null;
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    // Only accept POST to /v1/events
    const url = new URL(request.url);
    if (request.method !== 'POST' || url.pathname !== '/v1/events') {
      return new Response('Not Found', { status: 404 });
    }

    // Require Stalagmight user agent - blocks scanners and bots
    const ua = request.headers.get('User-Agent') ?? '';
    if (!ua.startsWith('Stalagmight/')) {
      return new Response('Not Found', { status: 404 });
    }

    // Reject oversized bodies before reading
    const contentLength = parseInt(request.headers.get('content-length') ?? '0', 10);
    if (contentLength > MAX_BODY_BYTES) {
      return new Response('Payload too large', { status: 413 });
    }

    const payload = await parseRequest(request);
    if (!payload) {
      return new Response('Invalid payload', { status: 400 });
    }

    // Basic sanity limits
    if (payload.session_id.length > 64 || payload.app_version.length > 32 || payload.os.length > 32) {
      return new Response('Field too long', { status: 400 });
    }

    const eventEntries = Object.entries(payload.events);
    if (eventEntries.length > 500) {
      return new Response('Too many events', { status: 400 });
    }

    // Hash the client IP for abuse detection (no raw IPs stored)
    const clientIp = request.headers.get('CF-Connecting-IP') ?? 'unknown';
    const ipBytes = new TextEncoder().encode(clientIp);
    const hashBuffer = await crypto.subtle.digest('SHA-256', ipBytes);
    const ipHash = Array.from(new Uint8Array(hashBuffer))
      .map((b) => b.toString(16).padStart(2, '0'))
      .join('');

    // Rate limit: count flushes from this IP in the last minute (read-only).
    // ip_hash is on event rows directly - no JOIN, no IP-flip exploit.
    const { results: rateRows } = await env.DB.prepare(
      `SELECT COUNT(DISTINCT received_at) as cnt FROM events
       WHERE ip_hash = ?1 AND received_at > datetime('now', '-1 minute')`
    ).bind(ipHash).all();
    const rate = rateRows[0];
    if (rate !== undefined && (rate.cnt as number) >= RATE_LIMIT_MAX_FLUSHES_PER_MIN) {
      return new Response('Too many requests', { status: 429 });
    }

    // Upsert session (update duration + ip_hash on repeated flushes from same session)
    const sessionStmt = env.DB.prepare(
      `INSERT INTO sessions (session_id, app_version, os, duration_secs, ip_hash)
       VALUES (?1, ?2, ?3, ?4, ?5)
       ON CONFLICT(session_id) DO UPDATE SET duration_secs = ?4, ip_hash = ?5, received_at = datetime('now')`
    ).bind(payload.session_id, payload.app_version, payload.os, payload.duration_secs, ipHash);

    // Insert events (project_seq defaults to 0 for backward compat)
    const projectSeq = payload.project_seq ?? 0;
    const eventStmts = eventEntries.map(([name, count]) =>
      env.DB.prepare(
        'INSERT INTO events (session_id, event_name, count, project_seq, ip_hash) VALUES (?1, ?2, ?3, ?4, ?5)'
      ).bind(payload.session_id, name.slice(0, 128), Math.min(count, 1_000_000), projectSeq, ipHash)
    );

    try {
      await env.DB.batch([sessionStmt, ...eventStmts]);
    } catch {
      return new Response('Database error', { status: 500 });
    }

    return new Response('OK', { status: 200 });
  },
};
