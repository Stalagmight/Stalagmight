//! Anonymous telemetry for tool usage counting.
//!
//! Always compiled, but the real implementation is behind the `telemetry` Cargo
//! feature flag. When disabled, all public methods are empty no-ops that the
//! compiler eliminates entirely - zero runtime cost. The event-name
//! [`registry`] is plain data and compiles either way, so its coverage tests run
//! in a plain `cargo test` too (#576 B7).
//!
//! Design:
//! - Session-level anonymous counters only - no PII
//! - Opt-in via config (`general.telemetry_enabled`)
//! - Buffers to disk when offline, retries on next flush
//! - Periodic flush every 60s (low-frequency-only batches every 10 min)
//! - On app close the pending counts go to the disk buffer, never the network,
//!   so quitting cannot wait on the server; the next launch sends them
//! - The buffer is drained on the flush thread (at its start, then after
//!   failures), within [`DRAIN_BUDGET`] per pass, so startup never waits either
//! - On network failure, retries every 60s until success

use std::collections::HashMap;

pub mod registry;

// ── Feature-gated imports ────────────────────────────────────────────

#[cfg(feature = "telemetry")]
use stalagmight_models::poison_policy::RecoveringMutex;
#[cfg(feature = "telemetry")]
use std::fs;
#[cfg(feature = "telemetry")]
use std::io::{Read, Write};
#[cfg(feature = "telemetry")]
use std::path::{Path, PathBuf};
#[cfg(feature = "telemetry")]
use std::sync::Arc;
#[cfg(feature = "telemetry")]
use std::time::Instant;

#[cfg(feature = "telemetry")]
use serde::{Deserialize, Serialize};

// ── Constants (feature-gated) ────────────────────────────────────────

/// Self-describing JSON wire-format version (event names sent verbatim, gzipped).
#[cfg(feature = "telemetry")]
const PAYLOAD_VERSION: u32 = 3;

#[cfg(feature = "telemetry")]
const ENDPOINT_URL: &str = env!("TELEMETRY_ENDPOINT");

#[cfg(feature = "telemetry")]
const HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Wall-clock budget for one pass over the disk buffer (#576 B17).
///
/// Stopping at the first retryable failure bounds an UNREACHABLE endpoint to
/// one `HTTP_TIMEOUT`, but a slow endpoint that still answers fails nothing:
/// up to [`MAX_BUFFERED_FILES`] sends of up to `HTTP_TIMEOUT` each, 500 s. The
/// pass runs on the flush thread, so this no longer holds up startup; it keeps
/// that thread from being tied up for minutes, and what is left waits for the
/// next pass.
#[cfg(feature = "telemetry")]
const DRAIN_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);

/// Upper bound on a buffered payload file `send_buffered` will read back. Real
/// payloads are a few KB of gzipped-JSON source; anything near this is foreign.
#[cfg(feature = "telemetry")]
const MAX_BUFFERED_FILE_BYTES: u64 = 1024 * 1024;

/// Upper bound on buffered payload files. One file lands per `project_seq`
/// bucket per failed flush (60s apart), so this covers many hours offline;
/// beyond it the OLDEST payloads are dropped - recent usage data is worth more
/// than a complete archive, and the alternative is unbounded growth (#520).
#[cfg(feature = "telemetry")]
const MAX_BUFFERED_FILES: usize = 100;

/// How many same-millisecond names to try before giving the payload up. Two or
/// three is the realistic ceiling (one file per `project_seq` bucket per failed
/// flush); the headroom is for a directory an attacker pre-seeded with links.
#[cfg(feature = "telemetry")]
const MAX_NAME_ATTEMPTS: u32 = 32;

// ── Payload type ─────────────────────────────────────────────────────

/// Self-describing wire payload (v3): event names are sent verbatim as a
/// `{name: count}` map. Serialized as JSON and gzipped on the wire - and also
/// the on-disk buffer format for offline retry, so the two are identical.
#[cfg(feature = "telemetry")]
#[derive(Debug, Serialize, Deserialize)]
struct TelemetryPayload {
    v: u32,
    app_version: String,
    os: String,
    session_id: String,
    duration_secs: u64,
    project_seq: u32,
    events: HashMap<String, u64>,
}

// ── Inner state ──────────────────────────────────────────────────────

#[cfg(feature = "telemetry")]
#[derive(Debug)]
struct TelemetryInner {
    /// Events bucketed by project_seq (0 = global/Rust-originated).
    events: HashMap<u32, HashMap<String, u64>>,
    session_id: String,
    session_start: Instant,
    enabled: bool,
}

// ══════════════════════════════════════════════════════════════════════
// TelemetryCollector - real implementation (feature = "telemetry")
// ══════════════════════════════════════════════════════════════════════

#[cfg(feature = "telemetry")]
#[derive(Debug, Clone)]
pub struct TelemetryCollector {
    inner: Arc<RecoveringMutex<TelemetryInner>>,
}

#[cfg(feature = "telemetry")]
impl TelemetryCollector {
    /// Create a new collector. If `enabled` is false, all tracking is a no-op.
    ///
    /// Dev affordance: `STALAGMIGHT_TELEMETRY_ENABLED=1` forces telemetry on at
    /// startup regardless of the saved opt-in, so `scripts/dev.sh` can dogfood
    /// telemetry without persisting the toggle. The Settings switch can still turn
    /// it off afterward, and production builds never set this var.
    pub fn new(enabled: bool) -> Self {
        let enabled = enabled
            || matches!(
                std::env::var("STALAGMIGHT_TELEMETRY_ENABLED").as_deref(),
                Ok("1") | Ok("true")
            );
        Self {
            inner: Arc::new(RecoveringMutex::new(
                "telemetry",
                TelemetryInner {
                    events: HashMap::new(),
                    session_id: generate_session_id(),
                    session_start: Instant::now(),
                    enabled,
                },
            )),
        }
    }

    /// Track a single event by name (Rust-originated, always project_seq 0).
    pub fn track(&self, event: &str) {
        {
            let mut inner = self.inner.lock();
            if !inner.enabled {
                return;
            }
            *inner
                .events
                .entry(0)
                .or_default()
                .entry(event.to_string())
                .or_insert(0) += 1;
        }
    }

    /// Track multiple events at once (batch from frontend), bucketed by project_seq.
    pub fn track_batch(&self, events: &HashMap<String, u64>, project_seq: u32) {
        {
            let mut inner = self.inner.lock();
            if !inner.enabled {
                return;
            }
            let bucket = inner.events.entry(project_seq).or_default();
            for (name, count) in events {
                *bucket.entry(name.clone()).or_insert(0) += count;
            }
        }
    }

    /// Set whether telemetry is enabled (user toggled in settings).
    pub fn set_enabled(&self, enabled: bool) {
        self.inner.lock().enabled = enabled;
    }

    /// Check if telemetry is currently enabled.
    pub fn is_enabled(&self) -> bool {
        self.inner.lock().enabled
    }

    /// Take every pending bucket as a payload, leaving the collector empty.
    /// Nothing when disabled or idle.
    fn take_payloads(&self) -> Vec<TelemetryPayload> {
        let mut inner = self.inner.lock();
        if !inner.enabled || inner.events.is_empty() {
            return Vec::new();
        }
        let app_version = env!("APP_VERSION").to_string();
        let os = std::env::consts::OS.to_string();
        let session_id = inner.session_id.clone();
        let duration_secs = inner.session_start.elapsed().as_secs();

        inner
            .events
            .drain()
            .map(|(project_seq, events)| TelemetryPayload {
                v: PAYLOAD_VERSION,
                app_version: app_version.clone(),
                os: os.clone(),
                session_id: session_id.clone(),
                duration_secs,
                project_seq,
                events,
            })
            .collect()
    }

    /// Flush current events to the telemetry backend as self-describing JSON +
    /// gzip. On network failure, buffers the same payload to disk for retry.
    pub fn flush(&self) {
        for payload in &self.take_payloads() {
            match send_payload(payload, HTTP_TIMEOUT) {
                Ok(()) => {}
                Err(e) if e.retryable => {
                    tracing::debug!(error = %e, "telemetry flush failed, buffering for retry");
                    buffer_to_disk(payload);
                }
                // A permanent rejection will never be accepted; don't buffer it
                // only for `send_buffered` to drop it on the next tick.
                Err(e) => {
                    tracing::debug!(error = %e, "telemetry payload rejected, dropping");
                }
            }
        }
    }

    /// Write every pending bucket to the disk buffer, with no network call -
    /// the app-close path (#576 B17).
    ///
    /// Sending on close made quitting wait on the server: one POST per
    /// project bucket, up to `HTTP_TIMEOUT` each, on the main thread. The
    /// buffer exists for exactly this - deferred sending - and the next launch's
    /// drain picks it up. Usage counts from a final session are worth
    /// milliseconds of quit time, not seconds.
    pub fn buffer_pending(&self) {
        let Some(dir) = telemetry_buffer_dir() else {
            return;
        };
        self.buffer_pending_to(&dir);
    }

    /// `buffer_pending` into a directory the caller names, so a test can own
    /// it (the real one is resolved from the environment; see #510).
    fn buffer_pending_to(&self, dir: &Path) {
        for payload in &self.take_payloads() {
            buffer_payload_in_dir(dir, payload);
        }
    }

    /// Send any payloads buffered from previous failed flushes (or left by the
    /// last app close), for at most [`DRAIN_BUDGET`].
    pub fn send_buffered(&self) {
        let enabled = self.inner.lock().enabled;
        if !enabled {
            return;
        }

        let Some(buffer_dir) = telemetry_buffer_dir() else {
            return;
        };
        drain_buffer_dir(&buffer_dir, Instant::now() + DRAIN_BUDGET, send_payload);
    }
}

/// Send the payloads buffered in `dir` (in directory order) until `deadline`;
/// return how many were sent.
///
/// Each send gets the smaller of `HTTP_TIMEOUT` and the time left, so the
/// pass as a whole ends by `deadline` however slow the endpoint is - what
/// stopping on a retryable failure alone could not promise, since a slow
/// endpoint that still answers never fails (#576 B17). The sender is a
/// parameter so a test can play that slow endpoint.
#[cfg(feature = "telemetry")]
fn drain_buffer_dir(
    dir: &Path,
    deadline: Instant,
    mut send: impl FnMut(&TelemetryPayload, std::time::Duration) -> Result<(), SendError>,
) -> usize {
    if buffer_dir_is_symlink(dir) {
        return 0;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };

    let mut sent = 0;
    for entry in entries.flatten() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            tracing::debug!(
                sent,
                "telemetry drain budget spent; the rest stays buffered"
            );
            break;
        }
        let Some(path) = admit_payload_entry(&entry) else {
            continue;
        };

        // `admit_payload_entry` already vouched for a non-empty regular
        // file within the size bound, so a read or parse failure now means
        // the bytes are corrupt (e.g. non-UTF-8), not transient - remove
        // it rather than re-examine it every tick and hold a cap slot
        // forever. (A zero-length file is a mid-write payload; admit skips
        // it WITHOUT removing, so it is never seen here.)
        let payload = read_payload_bounded(&path)
            .and_then(|content| serde_json::from_str::<TelemetryPayload>(&content).ok());
        let Some(payload) = payload else {
            let _ = fs::remove_file(&path);
            continue;
        };

        match send(&payload, left.min(HTTP_TIMEOUT)) {
            Ok(()) => {
                let _ = fs::remove_file(&path);
                sent += 1;
            }
            // Stop at the first RETRYABLE failure: the endpoint is
            // unreachable and the rest of the buffer would time out too.
            // The deadline above covers the other slow case, an endpoint
            // that answers, but slowly.
            Err(e) if e.retryable => break,
            // A permanently rejected payload fails fast, not on a timeout,
            // so it does NOT contribute to the stall - and breaking on it
            // would wedge every later buffered file behind one file that
            // can never leave. Drop it and drain the rest.
            Err(_) => {
                let _ = fs::remove_file(&path);
            }
        }
    }
    sent
}

// ══════════════════════════════════════════════════════════════════════
// TelemetryCollector - no-op stub (feature != "telemetry")
// ══════════════════════════════════════════════════════════════════════

#[cfg(not(feature = "telemetry"))]
#[derive(Debug, Clone)]
pub struct TelemetryCollector;

#[cfg(not(feature = "telemetry"))]
impl TelemetryCollector {
    pub fn new(_enabled: bool) -> Self {
        Self
    }
    pub fn track(&self, _event: &str) {}
    pub fn track_batch(&self, _events: &HashMap<String, u64>, _project_seq: u32) {}
    pub fn set_enabled(&self, _enabled: bool) {}
    pub fn is_enabled(&self) -> bool {
        false
    }
    pub fn flush(&self) {}
    pub fn buffer_pending(&self) {}
    pub fn send_buffered(&self) {}
}

// ── Helper functions (feature-gated) ─────────────────────────────────

/// Serialize the payload as JSON, gzip it, and POST to the telemetry endpoint,
/// giving up after `timeout`.
#[cfg(feature = "telemetry")]
fn send_payload(payload: &TelemetryPayload, timeout: std::time::Duration) -> Result<(), SendError> {
    // A payload that cannot be serialized or gzipped is broken, not transient -
    // it will never encode, so drop it rather than retry.
    let encode_err = |e: &dyn std::fmt::Display| SendError {
        retryable: false,
        msg: e.to_string(),
    };
    let json = serde_json::to_vec(payload).map_err(|e| encode_err(&e))?;

    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&json).map_err(|e| encode_err(&e))?;
    let body = encoder.finish().map_err(|e| encode_err(&e))?;

    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .new_agent();

    let response = agent
        .post(ENDPOINT_URL)
        .header(
            "User-Agent",
            &format!("Stalagmight/{}", env!("APP_VERSION")),
        )
        .header("Content-Encoding", "gzip")
        .content_type("application/json")
        .send(&body[..]);

    match response {
        Ok(_) => Ok(()),
        Err(e) => Err(classify_send_error(&e)),
    }
}

/// A failed send, split by whether the endpoint might accept the payload later.
#[cfg(feature = "telemetry")]
struct SendError {
    /// Transport failure, timeout, 5xx, or 429 - a later attempt may succeed,
    /// so the buffered copy is worth keeping. A permanent HTTP rejection (any
    /// other 4xx) is NOT retryable: the same bytes will fail identically
    /// forever, so retrying only wedges the drain behind a payload that can
    /// never leave.
    retryable: bool,
    msg: String,
}

#[cfg(feature = "telemetry")]
impl std::fmt::Display for SendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

#[cfg(feature = "telemetry")]
fn classify_send_error(e: &ureq::Error) -> SendError {
    let retryable = match e {
        // 408 request-timeout and 429 rate-limit are transient; 5xx is the
        // server's problem, not the payload's. Any other 4xx is a permanent
        // rejection of these exact bytes.
        ureq::Error::StatusCode(code) => *code == 408 || *code == 429 || *code >= 500,
        // Transport, timeout, protocol, io: the network, not the payload.
        _ => true,
    };
    SendError {
        retryable,
        msg: e.to_string(),
    }
}

#[cfg(feature = "telemetry")]
fn buffer_to_disk(payload: &TelemetryPayload) {
    let Some(dir) = telemetry_buffer_dir() else {
        return;
    };
    buffer_payload_in_dir(&dir, payload);
}

/// Write one payload into `dir` as `<unix-millis>-<n>.json`, creating `dir`
/// first, then prune the buffer back under [`MAX_BUFFERED_FILES`].
///
/// Split out from `buffer_to_disk` so a test can exercise the write against a
/// directory it owns (#510). On Linux, resolving the real buffer dir falls
/// through to `directories`, which reads `XDG_DATA_HOME` **at call time** - and
/// the logging tests in this same binary used to flip that variable
/// process-wide (retired with #519), so a test that resolved it once to write
/// and again to read could straddle the change and read a different (or
/// since-deleted) directory. Taking the directory as an argument keeps this
/// test out of the ambient environment entirely, and out of the developer's
/// own buffer.
///
/// Every failure is deliberately swallowed: buffering is the best-effort retry
/// path for telemetry that has already failed to send, so it must never surface
/// as an error.
#[cfg(feature = "telemetry")]
fn buffer_payload_in_dir(dir: &Path, payload: &TelemetryPayload) {
    // `None` when the clock is before the epoch (dead RTC): the millis-based
    // sort key would then be a meaningless 0 for every write this session, so
    // `buffer_payload_at` skips the prune rather than evict the payload it just
    // wrote. A well-formed timestamp is what makes the prune ordering sound.
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis());
    buffer_payload_at(dir, millis, payload);
}

/// Write + prune with the timestamp injected, so a test can drive the real
/// write-and-prune wiring at deterministic, strictly-increasing millis instead
/// of racing the wall clock (where >=32 writes in one millisecond would exhaust
/// [`MAX_NAME_ATTEMPTS`] and flake). `None` millis = a clock error: buffer
/// under a fallback name but skip the prune, whose key we cannot trust.
#[cfg(feature = "telemetry")]
fn buffer_payload_at(dir: &Path, millis: Option<u128>, payload: &TelemetryPayload) {
    // Refuse a symlinked buffer dir: `create_new` guards each FILE, but the
    // prune below would follow a symlinked DIR and delete `.json` files in
    // whatever it points at - unrelated user data outside our namespace. The
    // read path (`send_buffered`) refuses it too (#520 review).
    if buffer_dir_is_symlink(dir) {
        return;
    }

    // The buffer holds only this user's own usage data, so the directory is
    // owner-only where the platform can express it. Create any missing parents
    // first, then the leaf non-recursively with 0700, so the mode lands on the
    // buffer dir alone and not on the shared app-data parents this call might
    // otherwise create. A pre-existing buffer dir keeps its mode.
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        if let Some(parent) = dir.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::DirBuilder::new().mode(0o700).create(dir);
    }
    #[cfg(not(unix))]
    let _ = fs::create_dir_all(dir);

    if let Ok(json) = serde_json::to_string(payload) {
        write_unique_payload(dir, millis.unwrap_or(0), json.as_bytes());
        if millis.is_some() {
            prune_buffer_dir(dir);
        }
    }
}

/// Write `json` to the first free `<millis>-<n>.json` in `dir` (#520).
///
/// `create_new` makes the collision-handling atomic: an existing file - a
/// sibling payload buffered in the same millisecond, which the old truncating
/// `fs::write` silently overwrote - and a pre-planted symlink (dangling
/// included, which `fs::write` would have followed) both fail the open with
/// `AlreadyExists`, shifting the write to the next `n` instead of destroying
/// anything. Unix permissions are tightened to 0600: the payload is the user's
/// own usage data, and nothing else reads the buffer.
///
/// Failures stay non-fatal but are no longer silent: this is the code path
/// whose entire job is not losing the payload, so each way of losing one
/// leaves a debug line.
#[cfg(feature = "telemetry")]
fn write_unique_payload(dir: &Path, millis: u128, json: &[u8]) {
    for n in 0..MAX_NAME_ATTEMPTS {
        let path = dir.join(format!("{millis}-{n}.json"));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        match options.open(&path) {
            Ok(mut file) => {
                if let Err(e) = file.write_all(json) {
                    tracing::debug!(error = %e, "telemetry buffer write failed; removing the partial file");
                    let _ = fs::remove_file(&path);
                }
                return;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => {
                tracing::debug!(error = %e, "telemetry buffer file could not be created");
                return;
            }
        }
    }
    tracing::debug!("telemetry buffer names exhausted for this millisecond; payload dropped");
}

/// lstat-gate one buffer-dir entry (#520): `Some(path)` when it is a non-empty
/// regular `.json` file within [`MAX_BUFFERED_FILE_BYTES`]. A directory, a
/// symlink, or an oversized file wearing `.json` is foreign - REMOVED here, or
/// it would be re-examined on every retry tick forever and (counted but
/// undeletable, in the directory case) poison the prune's cap accounting into
/// evicting real payloads. A ZERO-length `.json` is left untouched: it is a
/// payload mid-write (`create_new` opens empty, then `write_all` fills it), so
/// deleting it here would race away a payload the writer is about to complete.
///
/// `DirEntry::metadata` is documented not to traverse symlinks - that is
/// load-bearing: it makes a planted symlink read as not-a-file, so the
/// `remove_file` below unlinks the LINK, never its target. A future
/// "simplification" to `fs::metadata(&path)` would silently follow it.
#[cfg(feature = "telemetry")]
fn admit_payload_entry(entry: &fs::DirEntry) -> Option<PathBuf> {
    let path = entry.path();
    if path.extension().and_then(|e| e.to_str()) != Some("json") {
        return None;
    }
    match entry.metadata() {
        Ok(m) if m.is_file() && m.len() == 0 => None,
        Ok(m) if m.is_file() && m.len() <= MAX_BUFFERED_FILE_BYTES => Some(path),
        Ok(m) => {
            if m.is_dir() {
                let _ = fs::remove_dir_all(&path);
            } else {
                if m.is_file() {
                    tracing::debug!(?path, "oversized telemetry buffer file discarded");
                }
                let _ = fs::remove_file(&path);
            }
            None
        }
        Err(_) => None,
    }
}

/// Read a payload back with the size check on the OPEN file descriptor
/// (fstat), not the directory entry - a path-based check can be raced by a
/// swap or an append between the stat and the read. The read is additionally
/// `take`-bounded as defence in depth, so even a file that grows between the
/// fstat and the read cannot exhaust memory.
///
/// Residual same-uid race, accepted: `fs::File::open` follows a symlink and
/// blocks on a FIFO, so a same-uid process that swaps an admitted regular file
/// for one of those AFTER the scan could redirect or stall this read. Closing
/// it fully needs `O_NOFOLLOW | O_NONBLOCK` (a `libc`/`rustix` dependency this
/// crate does not carry). The buffer dir is 0700 same-uid, the payload is
/// anonymous counters, and such an attacker can already delete the files
/// directly, so the write-path hardening (`create_new`) is the load-bearing
/// half; this read path is best-effort. Tracked as a #520 follow-up.
#[cfg(feature = "telemetry")]
fn read_payload_bounded(path: &Path) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_BUFFERED_FILE_BYTES {
        return None;
    }
    let mut content = String::new();
    file.take(MAX_BUFFERED_FILE_BYTES + 1)
        .read_to_string(&mut content)
        .ok()?;
    Some(content)
}

/// Sort key for a buffered filename: (`millis`, `n`) from `<millis>-<n>.json`,
/// with the pre-#520 bare `<millis>.json` reading as `n = 0`. Names that parse
/// as neither are foreign - never minted by the writer - and sort oldest so the
/// prune clears them first (`send_buffered` would unlink them as corrupt anyway).
///
/// A well-formed but implausibly FUTURE `<millis>` sorts newest and would
/// survive the prune ahead of real payloads. Planting one needs write access
/// to the 0700 same-uid buffer dir, from where an attacker can already delete
/// payloads directly, so this is not defended here; it is noted so a later
/// clock-trust change starts from the known gap (#520 follow-up).
#[cfg(feature = "telemetry")]
fn buffered_sort_key(path: &Path) -> (u128, u32) {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let (millis, n) = match stem.split_once('-') {
        Some((millis, n)) => (millis, n),
        None => (stem, "0"),
    };
    match (millis.parse::<u128>(), n.parse::<u32>()) {
        (Ok(millis), Ok(n)) => (millis, n),
        _ => (0, 0),
    }
}

/// Delete the oldest buffered payload files until at most
/// [`MAX_BUFFERED_FILES`] remain. Age comes from the encoded timestamp in the
/// filename, not mtime - the writer mints the names, so the key is
/// deterministic and testable.
///
/// The cap is counted over what `admit_payload_entry` admits - regular files
/// only - and the admission pass removes foreign objects as it goes. Counting
/// raw `.json` entries instead let a single undeletable directory inflate
/// `excess` forever, turning the prune into an evictor of real payloads while
/// the directory itself kept growing.
#[cfg(feature = "telemetry")]
fn prune_buffer_dir(dir: &Path) {
    // Self-safe against a symlinked buffer dir rather than trusting the caller:
    // this function DELETES, so a symlinked `dir` would evict `.json` files
    // from the target - unrelated user data. `buffer_payload_at` also refuses
    // one before writing, but the guard belongs here too so the deletion path
    // is safe on its own (#520 review).
    if buffer_dir_is_symlink(dir) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .filter_map(|e| admit_payload_entry(&e))
        .collect();
    if files.len() <= MAX_BUFFERED_FILES {
        return;
    }
    files.sort_by_key(|p| buffered_sort_key(p));
    let excess = files.len() - MAX_BUFFERED_FILES;
    for path in files.iter().take(excess) {
        let _ = fs::remove_file(path);
    }
}

/// True when `dir` exists and is itself a symlink rather than a real
/// directory. Both buffer paths refuse such a dir: the writer would create
/// files in, and the prune would DELETE files from, whatever it points at -
/// collateral damage to unrelated user data. `create_new` protects individual
/// files but not the directory node, so this is the guard for that node
/// (#520 review). A missing dir or a real dir returns false.
#[cfg(feature = "telemetry")]
fn buffer_dir_is_symlink(dir: &Path) -> bool {
    fs::symlink_metadata(dir).is_ok_and(|m| m.file_type().is_symlink())
}

/// Directory name under the local data dir holding payloads buffered for retry.
///
/// One constant for both resolutions below, so the writer and `send_buffered`
/// cannot disagree about it: renaming it in one place only would strand every
/// already-buffered payload, and no test can see that (resolving the real
/// directory is exactly the env-dependent step the tests avoid, #510).
#[cfg(feature = "telemetry")]
const BUFFER_DIR_NAME: &str = "telemetry-buffer";

#[cfg(feature = "telemetry")]
fn telemetry_buffer_dir() -> Option<PathBuf> {
    crate::app_dirs::data_local_dir()
        .map(|d| d.join(BUFFER_DIR_NAME))
        .or_else(|| {
            directories::ProjectDirs::from("com", "stalagmight", "Stalagmight")
                .map(|dirs| dirs.data_local_dir().join(BUFFER_DIR_NAME))
        })
}

#[cfg(feature = "telemetry")]
fn generate_session_id() -> String {
    let mut bytes = [0u8; 16];

    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        if f.read_exact(&mut bytes).is_ok() {
            return format_session_id(&bytes);
        }
    }

    // Fallback: use hash of current time + thread id for entropy
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    std::time::SystemTime::now().hash(&mut hasher);
    std::thread::current().id().hash(&mut hasher);
    let h1 = hasher.finish();
    bytes[..8].copy_from_slice(&h1.to_le_bytes());

    h1.hash(&mut hasher);
    let h2 = hasher.finish();
    bytes[8..].copy_from_slice(&h2.to_le_bytes());

    format_session_id(&bytes)
}

#[cfg(feature = "telemetry")]
fn format_session_id(bytes: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

/// Start a background thread that periodically flushes telemetry.
///
/// Flush policy:
/// - At thread start: one budgeted pass over the disk buffer - what the last
///   session left on close, or failed to send. Here rather than in Tauri's
///   setup, where it held up startup by up to cap x timeout on a slow
///   endpoint (#576 B17).
/// - Every 60s: flush if there are action events to send
/// - Every 10 min: flush even if only low-frequency events are pending
///   (dwell timers, `get_network_status` polling)
/// - On flush failure (no internet): retry every 60s until success,
///   then resume normal schedule
pub fn start_periodic_flush(collector: TelemetryCollector) {
    #[cfg(feature = "telemetry")]
    {
        std::thread::Builder::new()
            .name("telemetry-flush".to_string())
            .spawn(move || {
                const TICK_SECS: u64 = 60;
                const DWELL_ONLY_TICKS: u64 = 10; // 10 * 60s = 10 min
                let mut ticks_since_flush: u64 = 0;

                collector.send_buffered();
                // Whatever the budget or a failure left behind is retried on
                // the next tick, exactly like a failed flush's payload.
                let mut has_buffered_failure = telemetry_buffer_dir()
                    .and_then(|d| fs::read_dir(d).ok())
                    .is_some_and(|mut entries| entries.next().is_some());

                loop {
                    std::thread::sleep(std::time::Duration::from_secs(TICK_SECS));
                    ticks_since_flush += 1;

                    // Retry buffered payloads from previous failures
                    if has_buffered_failure {
                        collector.send_buffered();
                    }

                    let should_flush = {
                        let inner = collector.inner.lock();
                        if !inner.enabled || inner.events.is_empty() {
                            false
                        } else {
                            let has_action_events = inner.events.values().any(|bucket| {
                                bucket.keys().any(|k| {
                                    !k.starts_with("dwell.")
                                        && !k.starts_with("command.get_network_status")
                                })
                            });
                            has_action_events
                                || ticks_since_flush >= DWELL_ONLY_TICKS
                                || has_buffered_failure
                        }
                    };

                    if should_flush {
                        collector.flush();

                        // Check if any payloads were buffered (flush failure)
                        has_buffered_failure = telemetry_buffer_dir()
                            .and_then(|d| fs::read_dir(d).ok())
                            .map_or(false, |mut entries| entries.next().is_some());

                        if !has_buffered_failure {
                            ticks_since_flush = 0;
                        }
                        // If flush failed, keep ticks_since_flush to avoid
                        // dwell-only delays on retry
                    }
                }
            })
            .ok();
    }
    #[cfg(not(feature = "telemetry"))]
    {
        let _ = collector;
    }
}

/// Returns true when the telemetry feature is compiled in.
pub fn is_feature_enabled() -> bool {
    cfg!(feature = "telemetry")
}

// ── Tests (only with feature enabled) ────────────────────────────────

#[cfg(all(test, feature = "telemetry"))]
#[expect(clippy::unwrap_used, clippy::print_stdout)]
mod drain_tests;

#[cfg(all(test, feature = "telemetry"))]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn track_increments_counter() {
        let collector = TelemetryCollector::new(true);
        collector.track("command.create_project");
        collector.track("command.create_project");
        collector.track("command.save_project");

        let inner = collector.inner.lock();
        let bucket = inner.events.get(&0).expect("project_seq 0 bucket");
        assert_eq!(bucket.get("command.create_project"), Some(&2));
        assert_eq!(bucket.get("command.save_project"), Some(&1));
    }

    #[test]
    fn track_disabled_is_noop() {
        let collector = TelemetryCollector::new(false);
        collector.track("command.create_project");

        let inner = collector.inner.lock();
        assert!(inner.events.is_empty());
    }

    #[test]
    fn track_batch_merges_counts() {
        let collector = TelemetryCollector::new(true);
        // track() goes into project_seq 0
        collector.track("command.save_project");

        // Batch into project_seq 0 to merge with single-event track
        let mut batch = HashMap::new();
        batch.insert("command.save_project".to_string(), 3);
        batch.insert("command.undo".to_string(), 5);
        collector.track_batch(&batch, 0);

        let inner = collector.inner.lock();
        let bucket = inner.events.get(&0).expect("project_seq 0 bucket");
        assert_eq!(bucket.get("command.save_project"), Some(&4));
        assert_eq!(bucket.get("command.undo"), Some(&5));
    }

    #[test]
    fn track_batch_separates_project_seqs() {
        let collector = TelemetryCollector::new(true);

        let mut batch1 = HashMap::new();
        batch1.insert("command.create_shot".to_string(), 2);
        collector.track_batch(&batch1, 1);

        let mut batch2 = HashMap::new();
        batch2.insert("command.create_shot".to_string(), 3);
        collector.track_batch(&batch2, 2);

        let inner = collector.inner.lock();
        assert_eq!(
            inner
                .events
                .get(&1)
                .and_then(|b| b.get("command.create_shot")),
            Some(&2)
        );
        assert_eq!(
            inner
                .events
                .get(&2)
                .and_then(|b| b.get("command.create_shot")),
            Some(&3)
        );
    }

    #[test]
    fn set_enabled_toggles_tracking() {
        let collector = TelemetryCollector::new(false);
        collector.track("event.a");
        assert!(collector.inner.lock().events.is_empty());

        collector.set_enabled(true);
        collector.track("event.a");
        let inner = collector.inner.lock();
        let bucket = inner.events.get(&0).expect("project_seq 0 bucket");
        assert_eq!(bucket.get("event.a"), Some(&1));
    }

    #[test]
    fn session_id_format() {
        let id = generate_session_id();
        assert_eq!(id.len(), 36);
        assert_eq!(id.chars().nth(8), Some('-'));
        assert_eq!(id.chars().nth(13), Some('-'));
        assert_eq!(id.chars().nth(18), Some('-'));
        assert_eq!(id.chars().nth(23), Some('-'));
    }

    #[test]
    fn flush_clears_events_even_on_network_failure() {
        let collector = TelemetryCollector::new(true);
        collector.track("event.test");

        // Flush will fail (no real server) but should still clear events
        collector.flush();

        let inner = collector.inner.lock();
        assert!(inner.events.is_empty());
    }

    #[test]
    fn json_gzip_roundtrip() {
        // Mirrors the wire path: encode like send_payload (JSON -> gzip), decode
        // like the worker (gunzip -> JSON.parse), and confirm the self-describing
        // event map survives intact, including a dynamic display-state key.
        let mut events = HashMap::new();
        events.insert("command.create_shot:keyboard".to_string(), 10);
        events.insert("tile.fetch.esri".to_string(), 250);
        events.insert("display-state.showGrid:true".to_string(), 1);

        let payload = TelemetryPayload {
            v: PAYLOAD_VERSION,
            app_version: "0.8.0-dev-9f3c1a2b".to_string(),
            os: "linux".to_string(),
            session_id: "a1b2c3d4-e5f6-7890-abcd-ef1234567890".to_string(),
            duration_secs: 120,
            project_seq: 0,
            events: events.clone(),
        };

        let json = serde_json::to_vec(&payload).unwrap();
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&json).unwrap();
        let gzipped = encoder.finish().unwrap();
        assert!(gzipped.len() < json.len(), "gzip should shrink the payload");

        let mut decoder = flate2::read::GzDecoder::new(&gzipped[..]);
        let mut decompressed = Vec::new();
        decoder.read_to_end(&mut decompressed).unwrap();
        let decoded: TelemetryPayload = serde_json::from_slice(&decompressed).unwrap();

        assert_eq!(decoded.v, 3);
        assert_eq!(decoded.events, events);
        assert_eq!(decoded.session_id, payload.session_id);
    }

    /// The buffered file is the retry input `send_buffered` reads back, so this
    /// asserts the round trip rather than just that a file appeared: a write
    /// that produced unparseable or truncated JSON would leave the offline
    /// retry path silently dead.
    ///
    /// Writes into a directory this test owns. It used to resolve the real
    /// buffer dir - which on Linux reads `XDG_DATA_HOME` at call time, once to
    /// write and again to read - while the logging tests then flipped that
    /// variable process-wide (retired with #519), so the two resolutions could
    /// disagree and the read hit a directory that did not exist (#510). It also deleted every `.json` in the
    /// developer's own telemetry buffer as "cleanup"; the `TempDir` handles that.
    ///
    /// One payload; what two writes in the same millisecond do to each other
    /// is `same_millisecond_payloads_get_distinct_files` below (#520).
    #[test]
    fn buffer_to_disk_writes_a_payload_that_reads_back() {
        let temp = TempDir::new().expect("temp dir");
        let dir = temp.path().join(BUFFER_DIR_NAME);
        let payload = test_payload();

        // The directory does not exist yet - creating it is part of the contract.
        buffer_payload_in_dir(&dir, &payload);

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("json"))
            .collect();

        assert_eq!(files.len(), 1, "one payload in, one file out");
        let written = fs::read_to_string(files[0].path()).unwrap();
        let round_tripped: TelemetryPayload = serde_json::from_str(&written).unwrap();
        assert_eq!(round_tripped.v, PAYLOAD_VERSION);
        assert_eq!(round_tripped.session_id, payload.session_id);
        assert_eq!(round_tripped.duration_secs, payload.duration_secs);
        // The event map is the whole point of the payload - a buffered file that
        // round-trips its envelope but drops its counts is a silent data loss.
        assert_eq!(round_tripped.events, payload.events);
    }

    /// Two payloads buffered in the same millisecond used to collide on
    /// `<millis>.json` and the second truncated the first away - on the code
    /// path whose entire job is not losing the payload (#520). The fixed
    /// timestamp makes the collision deterministic instead of hoping two
    /// `buffer_payload_in_dir` calls land in one millisecond.
    #[test]
    fn same_millisecond_payloads_get_distinct_files() {
        let temp = TempDir::new().expect("temp dir");
        write_unique_payload(temp.path(), 123, b"{\"first\":1}");
        write_unique_payload(temp.path(), 123, b"{\"second\":2}");

        let first = fs::read_to_string(temp.path().join("123-0.json")).expect("first file");
        let second = fs::read_to_string(temp.path().join("123-1.json")).expect("second file");
        assert_eq!(first, "{\"first\":1}");
        assert_eq!(second, "{\"second\":2}");
    }

    /// A pre-planted symlink at the predictable name must not be written
    /// through (the old `fs::write` followed it - an overwrite primitive for a
    /// same-uid process or a misconfigured parent directory, #520).
    /// `create_new` refuses it and the payload shifts to the next name.
    #[test]
    #[cfg(unix)]
    fn write_refuses_a_preplanted_symlink() {
        let temp = TempDir::new().expect("temp dir");
        let victim = temp.path().join("victim.txt");
        fs::write(&victim, "precious").expect("write victim");
        std::os::unix::fs::symlink(&victim, temp.path().join("123-0.json")).expect("plant symlink");

        write_unique_payload(temp.path(), 123, b"{\"v\":3}");

        assert_eq!(
            fs::read_to_string(&victim).expect("victim"),
            "precious",
            "the symlink target must not be overwritten"
        );
        assert_eq!(
            fs::read_to_string(temp.path().join("123-1.json")).expect("shifted payload"),
            "{\"v\":3}",
            "the payload lands on the next free name"
        );
    }

    /// The buffer holds the user's own usage data; nothing else reads it, so
    /// the files are owner-only (#520).
    ///
    /// Caveat: under a 0o077 (or stricter) umask the DEFAULT mode is already
    /// 0600, so there this passes without the explicit `mode(0o600)` line -
    /// the assertion is injection-proof only under permissive umasks like
    /// CI's 022. Controlling the umask would mean mutating process-wide
    /// state, the exact class #519 removed from this binary.
    #[test]
    #[cfg(unix)]
    fn buffered_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().expect("temp dir");
        write_unique_payload(temp.path(), 123, b"{}");
        let mode = fs::metadata(temp.path().join("123-0.json"))
            .expect("metadata")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    /// The prune key must order the pre-#520 bare `<millis>.json` names, the
    /// counter-suffixed ones, and foreign names correctly - foreign sorts
    /// oldest so it is cleared first.
    #[test]
    fn buffered_sort_key_orders_legacy_counter_and_foreign_names() {
        let key = |name: &str| buffered_sort_key(Path::new(name));
        assert_eq!(key("123.json"), (123, 0), "legacy name reads as n = 0");
        assert_eq!(key("123-2.json"), (123, 2));
        assert!(key("123-0.json") < key("123-1.json"));
        assert!(key("123-9.json") < key("124-0.json"), "millis outranks n");
        assert_eq!(key("not-ours.json"), (0, 0), "foreign sorts oldest");
    }

    /// Beyond [`MAX_BUFFERED_FILES`] the OLDEST payloads are dropped - a long
    /// offline session used to grow the directory without limit (#520).
    #[test]
    fn prune_drops_the_oldest_files_beyond_the_cap() {
        let temp = TempDir::new().expect("temp dir");
        for millis in 0..(MAX_BUFFERED_FILES as u128 + 3) {
            write_unique_payload(temp.path(), millis, b"{}");
        }

        prune_buffer_dir(temp.path());

        let count = fs::read_dir(temp.path()).expect("read dir").count();
        assert_eq!(count, MAX_BUFFERED_FILES, "pruned back to the cap");
        for millis in 0..3u128 {
            assert!(
                !temp.path().join(format!("{millis}-0.json")).exists(),
                "the oldest files are the ones dropped"
            );
        }
        assert!(temp.path().join("3-0.json").exists(), "newer files survive");
    }

    fn test_payload() -> TelemetryPayload {
        let mut events = HashMap::new();
        events.insert("command.create_shot:keyboard".to_string(), 3);
        TelemetryPayload {
            v: PAYLOAD_VERSION,
            app_version: "0.1.0".to_string(),
            os: "test".to_string(),
            session_id: "test-session".to_string(),
            duration_secs: 60,
            project_seq: 0,
            events,
        }
    }

    /// The cap must hold at the REAL entry point. A test that calls
    /// `prune_buffer_dir` directly cannot notice the call being deleted from
    /// the write path - the vacuous-delegation shape - and this one exists
    /// precisely to fail on that deletion (proven by injection).
    ///
    /// Drives `buffer_payload_at` with injected, strictly-increasing millis:
    /// the wall-clock path would flake if >=32 of these fast writes landed in
    /// one millisecond and exhausted `MAX_NAME_ATTEMPTS`.
    #[test]
    fn the_cap_is_enforced_through_the_real_entry_point() {
        let temp = TempDir::new().expect("temp dir");
        let payload = test_payload();
        for millis in 0..(MAX_BUFFERED_FILES as u128 + 2) {
            buffer_payload_at(temp.path(), Some(millis), &payload);
        }
        let count = fs::read_dir(temp.path()).expect("read dir").count();
        assert_eq!(
            count, MAX_BUFFERED_FILES,
            "writing past the cap through the write path must prune"
        );
    }

    /// A clock error (millis `None`) still buffers the payload but skips the
    /// prune, whose key would be a meaningless 0 - so the just-written payload
    /// is never the prune's own first victim on a backwards clock.
    #[test]
    fn a_clock_error_buffers_without_pruning() {
        let temp = TempDir::new().expect("temp dir");
        let payload = test_payload();
        // Pre-fill past the cap so a prune, if it ran, would evict.
        for n in 0..(MAX_BUFFERED_FILES + 2) {
            fs::write(temp.path().join(format!("10-{n}.json")), "{}").expect("seed");
        }
        buffer_payload_at(temp.path(), None, &payload);
        let count = fs::read_dir(temp.path()).expect("read dir").count();
        assert_eq!(
            count,
            MAX_BUFFERED_FILES + 3,
            "the write lands and nothing is pruned on a clock error"
        );
    }

    /// The sharp cap-poisoning case: an undeletable `.json` DIRECTORY plus
    /// `MAX + 1` real payloads. Before #520 the directory was counted toward
    /// the cap but `remove_file` could not evict it, so the prune deleted a
    /// real payload every pass while the directory grew. Now the admission
    /// pass removes it and the cap is counted over real payloads only, so
    /// exactly one real payload (the oldest) is evicted and the rest survive.
    #[test]
    fn an_undeletable_directory_does_not_poison_the_cap() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir(temp.path().join("0-0.json")).expect("dir squatting a payload name");
        // MAX + 1 real payloads at millis 1..=MAX+1, so the oldest (1-0) is the
        // single legitimate eviction and everything from 2-0 up must survive.
        for millis in 1..=(MAX_BUFFERED_FILES as u128 + 1) {
            fs::write(temp.path().join(format!("{millis}-0.json")), "{}").expect("real payload");
        }

        prune_buffer_dir(temp.path());

        assert!(
            !temp.path().join("0-0.json").exists(),
            "the squatting directory is removed by the admission pass"
        );
        assert!(
            !temp.path().join("1-0.json").exists(),
            "exactly the oldest real payload is evicted"
        );
        let surviving = fs::read_dir(temp.path()).expect("read dir").count();
        assert_eq!(
            surviving, MAX_BUFFERED_FILES,
            "the directory did not steal a cap slot from a real payload"
        );
    }

    /// Foreign objects wearing `.json` - a directory, an oversized file, a
    /// dangling symlink - are removed by the admission pass, not retried
    /// forever; a zero-length `.json` (a payload mid-write) is left untouched;
    /// and a real payload survives.
    #[test]
    fn admission_removes_foreign_objects_and_spares_mid_write() {
        let temp = TempDir::new().expect("temp dir");
        fs::create_dir(temp.path().join("0-0.json")).expect("dir squatting a payload name");
        fs::write(
            temp.path().join("1-0.json"),
            vec![b' '; (MAX_BUFFERED_FILE_BYTES + 1) as usize],
        )
        .expect("oversized file");
        fs::write(temp.path().join("4-0.json"), "").expect("zero-length mid-write payload");
        #[cfg(unix)]
        std::os::unix::fs::symlink(temp.path().join("gone"), temp.path().join("2-0.json"))
            .expect("dangling symlink");
        write_unique_payload(temp.path(), 3, b"{}");

        // The admission pass runs as a side effect of collecting the survivors.
        let admitted: Vec<_> = fs::read_dir(temp.path())
            .expect("read dir")
            .flatten()
            .filter_map(|e| admit_payload_entry(&e))
            .collect();

        assert!(
            !temp.path().join("0-0.json").exists(),
            "a directory named *.json is removed, not retried forever"
        );
        assert!(
            !temp.path().join("1-0.json").exists(),
            "an oversized file is removed"
        );
        #[cfg(unix)]
        assert!(
            fs::symlink_metadata(temp.path().join("2-0.json")).is_err(),
            "the symlink itself is unlinked"
        );
        assert!(
            temp.path().join("4-0.json").exists(),
            "a zero-length payload mid-write is left for the writer to finish"
        );
        assert_eq!(
            admitted,
            vec![temp.path().join("3-0.json")],
            "only the completed real payload is admitted"
        );
    }

    /// A permanent HTTP rejection (a 4xx that is not 408/429) must classify as
    /// non-retryable, so `send_buffered` drops that payload and drains the
    /// rest instead of wedging the whole buffer behind it. Transient statuses
    /// (5xx, 429, 408) and transport errors stay retryable.
    #[test]
    fn send_errors_classify_retryable_vs_permanent() {
        assert!(!classify_send_error(&ureq::Error::StatusCode(400)).retryable);
        assert!(!classify_send_error(&ureq::Error::StatusCode(413)).retryable);
        assert!(classify_send_error(&ureq::Error::StatusCode(408)).retryable);
        assert!(classify_send_error(&ureq::Error::StatusCode(429)).retryable);
        assert!(classify_send_error(&ureq::Error::StatusCode(500)).retryable);
        assert!(classify_send_error(&ureq::Error::StatusCode(503)).retryable);
    }

    /// A symlinked buffer directory is refused by the write path: `create_new`
    /// guards each file, but a prune through a symlinked dir would delete
    /// `.json` files in the target - unrelated user data. The target's file
    /// must survive and nothing may be written into it (#520 review).
    #[test]
    #[cfg(unix)]
    fn a_symlinked_buffer_dir_is_left_alone() {
        let temp = TempDir::new().expect("temp dir");
        let target = temp.path().join("real-target");
        fs::create_dir(&target).expect("target dir");
        let victim = target.join("9-0.json");
        fs::write(&victim, "{\"user\":\"data\"}").expect("victim payload");

        let link = temp.path().join("telemetry-buffer");
        std::os::unix::fs::symlink(&target, &link).expect("symlink the buffer dir");

        buffer_payload_at(&link, Some(1), &test_payload());

        assert_eq!(
            fs::read_to_string(&victim).expect("victim survives"),
            "{\"user\":\"data\"}",
            "the write path must not touch a symlinked buffer dir's target"
        );
        // No payload of ours was written into the target either.
        let count = fs::read_dir(&target).expect("read target").count();
        assert_eq!(count, 1, "only the pre-existing file remains");

        // And the prune itself is self-safe: filling the target past the cap
        // and pruning through the symlink must delete nothing (Codex's exact
        // scenario - foreign names would sort oldest and be evicted).
        for millis in 100..(100 + MAX_BUFFERED_FILES as u128 + 5) {
            fs::write(target.join(format!("{millis}-0.json")), "{}").expect("target payload");
        }
        let before = fs::read_dir(&target).expect("read target").count();
        prune_buffer_dir(&link);
        let after = fs::read_dir(&target).expect("read target").count();
        assert_eq!(
            before, after,
            "pruning through a symlinked buffer dir must not delete the target's files"
        );
    }

    /// The read-back size guard is on the OPEN file (fstat), so an oversized
    /// file is rejected there even if it reached this point.
    #[test]
    fn read_back_rejects_an_oversized_file() {
        let temp = TempDir::new().expect("temp dir");
        let oversized = temp.path().join("1-0.json");
        fs::write(
            &oversized,
            vec![b' '; (MAX_BUFFERED_FILE_BYTES + 1) as usize],
        )
        .expect("oversized file");
        assert!(read_payload_bounded(&oversized).is_none());

        let ok = temp.path().join("2-0.json");
        fs::write(&ok, "{\"v\":3}").expect("small file");
        assert_eq!(read_payload_bounded(&ok).as_deref(), Some("{\"v\":3}"));
    }
}
