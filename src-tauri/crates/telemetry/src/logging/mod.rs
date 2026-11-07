pub mod panic_hook;

use anyhow::Result;
use serde::Serialize;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use stalagmight_models::poison_policy::RecoveringMutex;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
use ts_rs::TS;

/// Maximum number of log entries kept in the ring buffer.
const RING_BUFFER_CAPACITY: usize = 500;

/// File-name prefix handed to the daily rolling appender.
///
/// The appender does NOT write `stalagmight.log`: with `Rotation::DAILY` it
/// writes `<prefix>.<date>`, i.e. `stalagmight.log.2026-08-11`, so the files'
/// `extension()` is the date and never `log`. Shared with the cleanup sweep
/// (`is_log_file`) so the two cannot drift apart - matching on the `log`
/// extension alone would skip every file the appender has ever produced.
const LOG_FILE_PREFIX: &str = "stalagmight.log";

/// Monotonic sequence counter for ring-buffer entries. Lets a caller ask for
/// "everything after seq N" (`get_recent_logs_since`) so a checkpoint-based
/// consumer (the agent harness) can detect new lines without content-diffing.
static LOG_SEQ: AtomicU64 = AtomicU64::new(0);

/// One entry in the in-memory ring buffer: a formatted log line plus a
/// monotonic sequence number and a wall-clock timestamp (ms since the epoch).
#[derive(Clone, Debug, Serialize, TS)]
pub struct RingLogEntry {
    #[ts(type = "number")]
    pub seq: u64,
    #[ts(type = "number")]
    pub ts_ms: i64,
    pub line: String,
}

/// Result of `get_recent_logs_since`: ring entries newer than the requested
/// checkpoint, the newest sequence number now present, and whether the ring
/// rotated past the checkpoint (so some entries were lost between checks).
#[derive(Clone, Debug, Default, Serialize, TS)]
pub struct LogsSince {
    pub entries: Vec<RingLogEntry>,
    #[ts(type = "number")]
    pub latest_seq: u64,
    pub dropped: bool,
}

/// Global ring buffer for recent log entries, readable by the `get_recent_logs` command.
static LOG_BUFFER: OnceLock<Arc<RecoveringMutex<VecDeque<RingLogEntry>>>> = OnceLock::new();

/// The directory the rolling file appender was actually opened in, recorded by
/// [`init`].
///
/// `log_directory()` does not resolve to the same path before and after Tauri's
/// `.setup()`: `init()` runs first, while `app_dirs` is still uninitialised, so
/// it gets the `directories` fallback (`~/.local/share/stalagmight/logs` on
/// Linux); once `.setup()` has called `init_app_dirs` the same function returns
/// the Tauri identifier dir (`<local-data>/com.stalagmight.desktop/logs`). A
/// sweep scheduled from `.setup()` must therefore target the directory the log
/// files are really in - calling `log_directory()` again there would name a
/// directory that holds no logs and no-op at `cleanup_logs_in_dir`'s
/// `exists()` guard, leaving the real one to grow forever.
static ACTIVE_LOG_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Append a formatted line to a ring buffer, minting a fresh sequence number and
/// timestamp and evicting the oldest entry when full. Shared by the tracing
/// layer and the frontend-log bridge so both paths stay consistent.
fn push_line_to(buffer: &RecoveringMutex<VecDeque<RingLogEntry>>, line: String) {
    let mut buf = buffer.lock();
    let seq = LOG_SEQ.fetch_add(1, Ordering::Relaxed) + 1;
    let ts_ms = chrono::Utc::now().timestamp_millis();
    if buf.len() >= RING_BUFFER_CAPACITY {
        buf.pop_front();
    }
    buf.push_back(RingLogEntry { seq, ts_ms, line });
}

/// A tracing `Layer` that captures formatted log lines into a shared ring buffer.
struct RingBufferLayer {
    buffer: Arc<RecoveringMutex<VecDeque<RingLogEntry>>>,
}

impl<S> tracing_subscriber::Layer<S> for RingBufferLayer
where
    S: tracing::Subscriber,
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        use std::fmt::Write;
        use tracing::field::{Field, Visit};

        /// Collects the message and all other fields from a tracing event.
        struct FieldVisitor {
            message: String,
            fields: Vec<(String, String)>,
        }

        impl Visit for FieldVisitor {
            fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
                let s = format!("{value:?}");
                if field.name() == "message" {
                    self.message = s;
                } else {
                    self.fields.push((field.name().to_string(), s));
                }
            }
            fn record_str(&mut self, field: &Field, value: &str) {
                if field.name() == "message" {
                    self.message = value.to_string();
                } else {
                    self.fields
                        .push((field.name().to_string(), value.to_string()));
                }
            }
            fn record_i64(&mut self, field: &Field, value: i64) {
                self.fields
                    .push((field.name().to_string(), value.to_string()));
            }
            fn record_u64(&mut self, field: &Field, value: u64) {
                self.fields
                    .push((field.name().to_string(), value.to_string()));
            }
            fn record_f64(&mut self, field: &Field, value: f64) {
                self.fields
                    .push((field.name().to_string(), value.to_string()));
            }
            fn record_bool(&mut self, field: &Field, value: bool) {
                self.fields
                    .push((field.name().to_string(), value.to_string()));
            }
        }

        let metadata = event.metadata();
        let mut visitor = FieldVisitor {
            message: String::new(),
            fields: Vec::new(),
        };
        event.record(&mut visitor);

        let mut line = format!(
            "{} {}: {}",
            metadata.level(),
            metadata.target(),
            visitor.message
        );

        // Append structured fields as key=value pairs
        if !visitor.fields.is_empty() {
            line.push_str(" {");
            for (i, (key, val)) in visitor.fields.iter().enumerate() {
                if i > 0 {
                    line.push_str(", ");
                }
                write!(&mut line, "{key}={val}").ok();
            }
            line.push('}');
        }

        push_line_to(&self.buffer, line);
    }
}

/// Initialize the logging system.
///
/// Returns a guard that must be kept alive for the duration of the application.
/// On platforms where file logging is unavailable (e.g. Android before setup),
/// only stderr and the in-memory ring buffer are used - no panic.
pub fn init() -> Option<WorkerGuard> {
    // Create the in-memory ring buffer
    let buffer = Arc::new(RecoveringMutex::new(
        "log_buffer",
        VecDeque::with_capacity(RING_BUFFER_CAPACITY),
    ));
    LOG_BUFFER.set(Arc::clone(&buffer)).ok();
    let ring_layer = RingBufferLayer { buffer };

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,stalagmight=debug"));

    // Try to set up file logging; skip gracefully if the directory is
    // unavailable or read-only (common on Android before Tauri setup).
    let (file_layer, guard) = match log_directory() {
        Ok(log_dir) if std::fs::create_dir_all(&log_dir).is_ok() => {
            let file_appender = tracing_appender::rolling::daily(&log_dir, LOG_FILE_PREFIX);
            let (non_blocking, guard) = tracing_appender::non_blocking(file_appender);
            let layer = fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .json();
            // Remember where the appender really opened, so a later
            // `cleanup_logs` sweeps THIS directory rather than whatever
            // `log_directory()` resolves to once `app_dirs` is up.
            ACTIVE_LOG_DIR.set(log_dir).ok();
            (Some(layer), Some(guard))
        }
        _ => (None, None),
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(file_layer)
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_ansi(true)
                .pretty(),
        )
        .with(ring_layer)
        .init();

    tracing::info!("Logging initialized");

    guard
}

/// Return recent log entries from the in-memory ring buffer.
///
/// Signature preserved (bare lines) so the in-app debug dialog is unaffected;
/// checkpoint-aware consumers use `get_recent_logs_since`.
pub fn get_recent_logs() -> Vec<String> {
    LOG_BUFFER
        .get()
        .map(|buf| buf.lock().iter().map(|e| e.line.clone()).collect())
        .unwrap_or_default()
}

/// Pure core of `get_recent_logs_since`, split out so the checkpoint/`dropped`
/// logic can be unit-tested against a hand-built buffer (the real one is a
/// process-global `OnceLock` that cannot be re-initialised in tests).
fn logs_since_from(buf: &VecDeque<RingLogEntry>, after_seq: u64) -> LogsSince {
    let latest_seq = buf.back().map_or(0, |e| e.seq);
    let oldest_seq = buf.front().map_or(0, |e| e.seq);
    // If the oldest entry we still hold is newer than after_seq + 1, the ring
    // rotated past the checkpoint and some entries were lost between checks.
    let dropped = after_seq > 0 && oldest_seq > after_seq + 1;
    let entries: Vec<RingLogEntry> = buf.iter().filter(|e| e.seq > after_seq).cloned().collect();
    LogsSince {
        entries,
        latest_seq,
        dropped,
    }
}

/// Return ring-buffer entries newer than `after_seq`, the newest sequence number
/// now present, and a `dropped` flag set when the ring rotated past the
/// checkpoint. Pass `after_seq = 0` to get everything currently buffered.
pub fn get_recent_logs_since(after_seq: u64) -> LogsSince {
    LOG_BUFFER
        .get()
        .map(|buf| logs_since_from(&buf.lock(), after_seq))
        .unwrap_or_default()
}

/// Push a pre-formatted log line into the ring buffer (used for frontend console logs).
pub fn push_to_buffer(line: String) {
    if let Some(buf) = LOG_BUFFER.get() {
        push_line_to(buf, line);
    }
}

/// Get the log directory path.
///
/// Prefers `app_dirs` (Tauri path resolver, works on Android). Falls back to
/// the `directories` crate for the pre-setup logging call on desktop where
/// `app_dirs` has not been initialised yet.
pub fn log_directory() -> Result<PathBuf> {
    if let Some(dir) = crate::app_dirs::data_local_dir() {
        return Ok(dir.join("logs"));
    }
    // Pre-setup fallback (desktop only - `directories` returns None on Android)
    let dirs = directories::ProjectDirs::from("com", "stalagmight", "Stalagmight")
        .ok_or_else(|| anyhow::anyhow!("No home directory"))?;
    Ok(dirs.data_local_dir().join("logs"))
}

/// Directory a [`cleanup_logs`] sweep must target: the one [`init`] recorded in
/// [`ACTIVE_LOG_DIR`], falling back to [`log_directory`] when init never ran
/// (unit tests, or a build with no file logging). Split out from `cleanup_logs`
/// so the preference can be tested without writing the process-global
/// `OnceLock`. See [`ACTIVE_LOG_DIR`] for why the recorded path wins.
fn cleanup_target_dir(recorded: Option<&PathBuf>) -> Result<PathBuf> {
    match recorded {
        Some(dir) => Ok(dir.clone()),
        None => log_directory(),
    }
}

/// Clean up old log files if directory size exceeds max.
///
/// Called once per launch from the Tauri `setup` hook; `max_size_mb` comes
/// straight from `logging.max_directory_size_mb`. The 1..=10000 bound on it is
/// enforced by the Settings UI only - neither the config layer nor the
/// deserializer clamps it - so this entry point floors it at 1 MB.
pub fn cleanup_logs(max_size_mb: u32) -> Result<()> {
    let log_dir = cleanup_target_dir(ACTIVE_LOG_DIR.get())?;
    // Floor: a hand-edited config.toml carrying 0 would otherwise delete every
    // log AND every crash-*.json panic marker on every launch.
    let max_size_mb = max_size_mb.max(1);
    let max_size_bytes = u64::from(max_size_mb) * 1024 * 1024;
    cleanup_logs_in_dir(&log_dir, max_size_bytes)
}

/// One file the sweep may delete, stat'ed exactly ONCE.
///
/// The captured `size` feeds both the running total and the per-delete
/// subtraction. The previous shape stat'ed every entry twice - once for the
/// sum, again at delete time - and subtracted the *fresh* size from a total
/// built from the *old* one. The live appender file grows between the two
/// reads (and a first stat that failed contributed 0 while the second
/// succeeds), so the subtraction could exceed the sum: a u64 underflow, which
/// panics inside Tauri's `setup` in dev builds and wraps to ~`u64::MAX` in
/// release, where the `total_size <= max_size_bytes` guard never trips again
/// and the sweep deletes everything it can.
struct SweepCandidate {
    path: PathBuf,
    size: u64,
    modified: std::time::SystemTime,
    /// Log-family file (vs. a `crash-*.json` marker). Only log files have a
    /// live writer, so only they need the newest-file exemption below.
    is_log: bool,
}

/// Clean up old log files (`stalagmight.log`, its daily-rotated
/// `stalagmight.log.<date>` siblings, and any plain `.log` file) plus
/// `crash-*.json` panic markers in `log_dir` if their total size exceeds
/// `max_size_bytes`. Deletes oldest files first and never deletes the newest
/// log file - the one the rolling appender holds open (see the `live_log`
/// comment below) - so the cap can be missed by at most that one file.
/// Crash markers are included because the panic hook writes one per backend
/// panic in ALL builds; without this sweep they accumulate unbounded in the
/// user's data dir (nothing else ever deletes them). Extracted for
/// cross-platform testability (`log_directory()` relies on platform paths).
/// Takes a raw byte limit, unclamped: [`cleanup_logs`] owns the 1 MB floor.
fn cleanup_logs_in_dir(log_dir: &std::path::Path, max_size_bytes: u64) -> Result<()> {
    if !log_dir.exists() {
        return Ok(());
    }

    let is_crash_marker = |p: &std::path::Path| {
        p.extension().is_some_and(|ext| ext == "json")
            && p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("crash-"))
    };
    // `stalagmight.log.2026-08-11` (what the daily appender actually writes)
    // has extension `2026-08-11`, so an extension-only test would match none of
    // the log files. Accept the appender's prefix followed by nothing or by a
    // dot - the suffix is a date in practice, but this deliberately does not
    // parse it, so a future rotation granularity still matches. What the dot
    // rules out is a longer *name* that merely starts with the prefix
    // (`stalagmight.logbook`, `stalagmight.log-old`): those belong to nobody
    // here. Plus any plain `.log` file left by an older build.
    let is_log_file = |p: &std::path::Path| {
        p.extension().is_some_and(|ext| ext == "log")
            || p.file_name()
                .and_then(|n| n.to_str())
                .and_then(|n| n.strip_prefix(LOG_FILE_PREFIX))
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.'))
    };

    // Stat each candidate exactly once, here, and drive the sum, the sort and
    // the deletion accounting off those captured values (see `SweepCandidate`
    // for the underflow this replaces). An entry whose stat fails is dropped
    // outright rather than counted as zero - it cannot be sized, so it cannot
    // be accounted for on deletion either.
    let mut entries: Vec<SweepCandidate> = std::fs::read_dir(log_dir)?
        .filter_map(std::result::Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let is_log = is_log_file(&path);
            if !is_log && !is_crash_marker(&path) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(SweepCandidate {
                path,
                size: metadata.len(),
                modified: metadata
                    .modified()
                    .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                is_log,
            })
        })
        .collect();

    // Oldest first, on the captured mtimes (the old `sort_by_key` re-stat'ed
    // every file once per comparison).
    entries.sort_by_key(|c| c.modified);

    // The newest log-family file is the one the daily rolling appender holds
    // open right now. On Linux/macOS `remove_file` SUCCEEDS on an open file:
    // the appender keeps writing to the unlinked inode until the next date
    // rollover, so the running session's logs are lost silently - precisely
    // when a directory big enough to trip the cap means we most want them.
    // Never delete it. Crash markers have no live writer, so they are all fair
    // game, and the cap can be overshot by at most this one file.
    let live_log = entries.iter().rposition(|c| c.is_log);

    let mut total_size: u64 = entries.iter().map(|c| c.size).sum();

    for (index, candidate) in entries.iter().enumerate() {
        if total_size <= max_size_bytes {
            break;
        }
        if Some(index) == live_log {
            continue;
        }

        if std::fs::remove_file(&candidate.path).is_ok() {
            // The size that went into `total_size`, not a fresh stat.
            // `saturating_sub` is belt and braces now that they cannot differ.
            total_size = total_size.saturating_sub(candidate.size);
            tracing::debug!(path = ?candidate.path, "Deleted old log file");
        }
    }

    Ok(())
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    use tempfile::TempDir;

    /// Write `contents` to `path` and pin its mtime to `mtime_secs` after the
    /// epoch. Every file a test writes into a temp dir lands within the same
    /// clock tick on some filesystems, which leaves the sweep's oldest-first
    /// order (and therefore which file survives as the newest) up to
    /// `read_dir` ordering. Pinning it makes those tests deterministic.
    fn write_with_mtime(path: &std::path::Path, contents: &[u8], mtime_secs: u64) {
        std::fs::write(path, contents).expect("write file");
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .expect("open for mtime")
            .set_modified(
                std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(mtime_secs),
            )
            .expect("set mtime");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn log_directory_uses_xdg_data_dir() {
        // Mirror of the macOS test below: pin the `directories` fallback shape
        // by READING the environment, never writing it. The old version set
        // XDG_DATA_HOME process-wide behind a lock only its own callers took,
        // racing every sibling test that calls env::var (#519; and `set_var`
        // stops compiling as written in edition 2024).
        //
        // With XDG_DATA_HOME unset (the common CI case) the prefix assert
        // pins nothing the `directories` crate does not compute itself, so
        // the repo-owned assertions are the two path components below it:
        // the `stalagmight` project segment and the `logs` leaf.
        assert!(
            crate::app_dirs::data_local_dir().is_none(),
            "something initialised app_dirs in this test binary; this test \
             pins the `directories` FALLBACK and now proves nothing"
        );
        let log_dir = log_directory().expect("log dir");
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            // `directories` ignores a relative XDG_DATA_HOME (per the XDG
            // spec) and an EMPTY HOME (dirs-sys falls back to getpwuid);
            // mirror both, or a correct log_directory() fails this test.
            .filter(|p| p.is_absolute())
            .or_else(|| {
                std::env::var_os("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|home| PathBuf::from(home).join(".local/share"))
            });
        // No usable XDG_DATA_HOME or HOME means dirs-sys resolved the home
        // dir via getpwuid, which this test cannot mirror without libc - the
        // component asserts below still hold, so skip only the prefix.
        if let Some(base) = base {
            assert!(
                log_dir.starts_with(&base),
                "Expected log dir under {base:?}, got {log_dir:?}"
            );
        }
        assert!(
            log_dir.components().any(|c| c.as_os_str() == "stalagmight"),
            "the ProjectDirs(com, stalagmight, Stalagmight) segment must survive: {log_dir:?}"
        );
        assert_eq!(log_dir.file_name().and_then(|s| s.to_str()), Some("logs"));
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn log_directory_uses_macos_library_path() {
        let log_dir = log_directory().expect("log dir");
        let home = std::env::var("HOME").expect("HOME must be set");
        let expected_prefix = format!("{home}/Library/Application Support");
        assert!(
            log_dir.starts_with(&expected_prefix),
            "Expected log dir under {expected_prefix}, got {log_dir:?}"
        );
        assert_eq!(log_dir.file_name().and_then(|s| s.to_str()), Some("logs"));
    }

    #[test]
    fn cleanup_logs_in_dir_removes_all_but_the_live_log_when_limit_zero() {
        // A raw zero-byte limit on the internal helper sweeps everything it is
        // allowed to sweep - which is everything EXCEPT the newest log file,
        // the one the rolling appender is writing through an open handle.
        // (The public `cleanup_logs` never passes zero; it floors at 1 MB.)
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let first = log_dir.join("first.log");
        let marker = log_dir.join("crash-7-1751970000000.json");
        let second = log_dir.join("second.log");
        write_with_mtime(&first, b"abc", 1_000);
        write_with_mtime(&marker, b"{}", 2_000);
        write_with_mtime(&second, b"defg", 3_000);

        cleanup_logs_in_dir(&log_dir, 0).expect("cleanup logs");

        assert!(!first.exists(), "the older log goes");
        assert!(
            !marker.exists(),
            "crash markers have no live writer, so every one of them goes"
        );
        assert!(
            second.exists(),
            "the newest log file must survive: remove_file succeeds on the \
             file the appender holds open, and it then writes the rest of the \
             session into an unlinked inode nobody can read"
        );
    }

    #[test]
    fn cleanup_logs_in_dir_keeps_the_live_log_even_when_it_alone_busts_the_cap() {
        // Regression (#319 review, finding 3): the loop used to delete oldest
        // first with no exemption, so a cap smaller than the current day's log
        // took that log with it. The sweep must instead give up under the cap.
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let rotated = log_dir.join(format!("{LOG_FILE_PREFIX}.2026-02-05"));
        let live = log_dir.join(format!("{LOG_FILE_PREFIX}.2026-02-06"));
        write_with_mtime(&rotated, &[b'o'; 64], 1_000);
        write_with_mtime(&live, &[b'n'; 4096], 2_000);

        // 100 bytes: smaller than the live file on its own.
        cleanup_logs_in_dir(&log_dir, 100).expect("cleanup logs");

        assert!(!rotated.exists(), "everything deletable still goes");
        assert!(
            live.exists(),
            "overshooting the cap by the live file beats losing the session's logs"
        );
    }

    #[test]
    fn cleanup_logs_in_dir_stops_at_the_cap_using_captured_sizes() {
        // Regression (#319 review, finding 1): the loop stat'ed each file
        // twice - once for the total, again at delete time - and subtracted
        // the second reading from a total built from the first. Sizes are now
        // captured once up front, so the running total is exactly the sum of
        // what is left and the loop stops the moment it is under the cap.
        // Three 10-byte logs against a 25-byte cap: only the oldest has to go.
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let oldest = log_dir.join("a.log");
        let middle = log_dir.join("b.log");
        let newest = log_dir.join("c.log");
        write_with_mtime(&oldest, &[b'a'; 10], 1_000);
        write_with_mtime(&middle, &[b'b'; 10], 2_000);
        write_with_mtime(&newest, &[b'c'; 10], 3_000);

        cleanup_logs_in_dir(&log_dir, 25).expect("cleanup logs");

        assert!(!oldest.exists(), "30 bytes > 25: the oldest file goes");
        assert!(
            middle.exists(),
            "20 bytes <= 25 after one delete: the sweep must stop there"
        );
        assert!(newest.exists(), "and the live log is never a candidate");
    }

    #[test]
    fn cleanup_target_dir_prefers_the_recorded_active_dir() {
        // Pure form of the rule `cleanup_logs` follows: whatever `init()`
        // recorded wins over `log_directory()`, because the two disagree once
        // `app_dirs` is initialised (pre-setup fallback vs identifier dir) and
        // only the recorded one holds log files. Order-independent: it never
        // touches the process-global OnceLock.
        let recorded = PathBuf::from("/somewhere/pre-setup/logs");
        let target = cleanup_target_dir(Some(&recorded)).expect("target dir");
        assert_eq!(target, recorded);
        assert_ne!(
            target,
            log_directory().expect("log dir"),
            "the recorded dir must not silently collapse to log_directory()"
        );
    }

    #[test]
    fn cleanup_target_dir_falls_back_to_log_directory_when_unrecorded() {
        // init() never ran (unit tests, or file logging unavailable): the sweep
        // still has to have a target, and log_directory() is the only guess.
        assert_eq!(
            cleanup_target_dir(None).expect("target dir"),
            log_directory().expect("log dir")
        );
    }

    #[test]
    fn cleanup_logs_sweeps_the_recorded_dir_under_a_one_mb_floor() {
        // End-to-end through the public entry point, which is the only place
        // the last three of these are observable:
        //   * with ACTIVE_LOG_DIR set, `cleanup_logs` deletes inside THAT
        //     directory - unset, it would resolve `log_directory()`, which
        //     post-setup names a different (empty) directory and no-ops;
        //   * `max_size_mb = 0` (reachable only by hand-editing config.toml,
        //     since the 1..=10000 bound is the Settings UI's) is floored to
        //     1 MB instead of deleting every log and every crash marker;
        //   * the newest log file survives regardless of the cap.
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        // `ACTIVE_LOG_DIR` is process-global, and this is the only test that
        // writes it (`init()` installs a global subscriber, so no unit test
        // calls it). Claim it, and refuse to continue if something else got
        // there first - asserting BEFORE any deletion, so a future setter can
        // never make this test sweep a real log directory.
        let recorded = ACTIVE_LOG_DIR.get_or_init(|| log_dir.clone());
        assert_eq!(
            recorded, &log_dir,
            "another test set ACTIVE_LOG_DIR; nothing else in this binary may"
        );

        let rotated = log_dir.join("stalagmight.log.2020-01-01");
        let marker = log_dir.join("crash-42-1751970000000.json");
        let live = log_dir.join("stalagmight.log.2026-08-11");
        let unrelated = log_dir.join("settings-backup.json");
        // One byte over the floor, so the sweep runs but only has to take the
        // oldest file to get back under it.
        write_with_mtime(&rotated, &vec![b'o'; 1024 * 1024 + 1], 1_000);
        write_with_mtime(&marker, b"{}", 2_000);
        write_with_mtime(&live, b"live", 3_000);
        std::fs::write(&unrelated, b"{}").expect("write other");

        cleanup_logs(0).expect("cleanup logs");

        assert!(!rotated.exists(), "recorded dir must be swept");
        assert!(
            marker.exists(),
            "0 MB floored to 1 MB: the sweep is already under the cap after \
             the rotated log, so panic markers are not wiped on every launch"
        );
        assert!(
            live.exists(),
            "the appender's current file is never deleted"
        );
        assert!(unrelated.exists(), "unrelated files stay");
    }

    #[test]
    fn cleanup_logs_in_dir_sweeps_daily_rotated_names() {
        // Regression: the appender writes `stalagmight.log.<date>`, whose
        // `extension()` is the DATE. A sweep that only matched `.log` files
        // deleted crash markers and left every real log file behind, so the
        // directory still grew without bound.
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        let rotated = log_dir.join(format!("{LOG_FILE_PREFIX}.2026-02-06"));
        let bare = log_dir.join(LOG_FILE_PREFIX);
        // Newest, so it stands in for the file the appender holds open and is
        // exempt from deletion - which lets the two above prove they are
        // recognised as log files by being deleted.
        let live = log_dir.join(format!("{LOG_FILE_PREFIX}.2026-02-07"));
        let foreign = log_dir.join("notes.txt");
        // Prefix matching must not spill onto a merely similar name. Three
        // shapes, each defeating a different sloppy test: `-extra` fails even
        // a bare `starts_with(LOG_FILE_PREFIX)`, while `logbook` and
        // `log-old` both PASS that and are rejected only by the
        // `rest.is_empty() || rest.starts_with('.')` refinement (the leftovers
        // are "book" and "-old"). Nothing here owns any of them.
        let lookalike = log_dir.join("stalagmight-extra.2026-02-06");
        let logbook = log_dir.join("stalagmight.logbook");
        let log_old = log_dir.join("stalagmight.log-old");
        write_with_mtime(&rotated, b"rotated", 1_000);
        write_with_mtime(&bare, b"bare", 2_000);
        write_with_mtime(&live, b"live", 3_000);
        std::fs::write(&foreign, b"notes").expect("write notes");
        std::fs::write(&lookalike, b"other").expect("write other");
        std::fs::write(&logbook, b"book").expect("write logbook");
        std::fs::write(&log_old, b"old").expect("write log-old");

        cleanup_logs_in_dir(&log_dir, 0).expect("cleanup logs");

        assert!(!rotated.exists(), "daily-rotated log must be swept");
        assert!(!bare.exists(), "bare prefix file must be swept");
        assert!(live.exists(), "the newest log file is never swept");
        assert!(lookalike.exists(), "a similar name is not our log file");
        assert!(logbook.exists(), "the prefix must be followed by a dot");
        assert!(log_old.exists(), "'-old' is not a dot suffix either");
        assert!(foreign.exists(), "unrelated files must be untouched");
    }

    #[test]
    fn push_line_mints_monotonic_seq_and_line() {
        let buffer = RecoveringMutex::new("log_buffer", VecDeque::new());
        push_line_to(&buffer, "a".to_string());
        push_line_to(&buffer, "b".to_string());
        let buf = buffer.lock();
        assert_eq!(buf.len(), 2);
        assert!(buf[1].seq > buf[0].seq, "seq must be strictly increasing");
        assert_eq!(buf[0].line, "a");
        assert_eq!(buf[1].line, "b");
    }

    #[test]
    fn logs_since_returns_only_newer_entries() {
        let mut buf = VecDeque::new();
        for seq in 3..=6 {
            buf.push_back(RingLogEntry {
                seq,
                ts_ms: 0,
                line: format!("l{seq}"),
            });
        }
        let since = logs_since_from(&buf, 4);
        assert_eq!(
            since.entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![5, 6]
        );
        assert_eq!(since.latest_seq, 6);
        assert!(!since.dropped);
    }

    #[test]
    fn logs_since_flags_dropped_when_ring_rotated_past_checkpoint() {
        // Oldest retained seq is 7, but the caller last saw seq 4, so seqs 5 and
        // 6 were evicted before this read: report the gap.
        let mut buf = VecDeque::new();
        for seq in 7..=9 {
            buf.push_back(RingLogEntry {
                seq,
                ts_ms: 0,
                line: format!("l{seq}"),
            });
        }
        let since = logs_since_from(&buf, 4);
        assert!(since.dropped);
        assert_eq!(since.latest_seq, 9);
    }

    #[test]
    fn logs_since_zero_checkpoint_never_flags_dropped() {
        let mut buf = VecDeque::new();
        buf.push_back(RingLogEntry {
            seq: 100,
            ts_ms: 0,
            line: "x".to_string(),
        });
        let since = logs_since_from(&buf, 0);
        assert!(!since.dropped);
        assert_eq!(since.entries.len(), 1);
    }

    #[test]
    fn cleanup_logs_in_dir_keeps_files_under_limit() {
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        // 3 bytes + 4 bytes = 7 bytes total
        std::fs::write(log_dir.join("first.log"), b"abc").expect("write log");
        std::fs::write(log_dir.join("second.log"), b"defg").expect("write log");

        // Limit of 10 bytes - both files fit
        cleanup_logs_in_dir(&log_dir, 10).expect("cleanup logs");

        assert!(log_dir.join("first.log").exists());
        assert!(log_dir.join("second.log").exists());
    }

    #[test]
    fn cleanup_logs_in_dir_sweeps_crash_markers_but_not_other_json() {
        let dir = TempDir::new().expect("temp dir");
        let log_dir = dir.path().join("logs");
        std::fs::create_dir_all(&log_dir).expect("create log dir");

        // The panic hook writes crash-<pid>-<ts>.json beside the logs in ALL
        // builds; the sweep must bound them (they used to accumulate forever).
        // An unrelated .json in the same dir must be left alone.
        let marker = log_dir.join("crash-1234-1751970000000.json");
        let other = log_dir.join("settings-backup.json");
        std::fs::write(&marker, b"{\"message\":\"panic\"}").expect("write marker");
        std::fs::write(&other, b"{}").expect("write other");

        cleanup_logs_in_dir(&log_dir, 0).expect("cleanup logs");

        assert!(!marker.exists(), "crash marker must be swept");
        assert!(other.exists(), "unrelated json must be untouched");
    }

    #[test]
    fn ring_buffer_evicts_oldest_at_capacity() {
        // Push past capacity: the ring must stay bounded and retain the NEWEST
        // entries with seq order intact (the eviction path was untested).
        let buffer = RecoveringMutex::new("log_buffer", VecDeque::new());
        let total = RING_BUFFER_CAPACITY + 25;
        for i in 0..total {
            push_line_to(&buffer, format!("line-{i}"));
        }
        let buf = buffer.lock();
        assert_eq!(buf.len(), RING_BUFFER_CAPACITY, "ring must be bounded");
        assert_eq!(
            buf.back().expect("newest").line,
            format!("line-{}", total - 1),
            "newest entry retained"
        );
        assert_eq!(
            buf.front().expect("oldest").line,
            format!("line-{}", total - RING_BUFFER_CAPACITY),
            "oldest entries evicted in order"
        );
        // Strictly increasing, NOT a fixed delta. `seq` is handed out by a
        // process-wide counter, so ANY other test that logs while this one runs takes
        // numbers out of the middle of our range - and `back.seq - front.seq ==
        // CAPACITY - 1` then fails on a ring that has done nothing wrong. It held only
        // as long as nothing else in a 2700-test parallel run happened to log during
        // these few microseconds, which is luck rather than a property, and it stopped
        // holding the moment the suite grew enough to reschedule around it.
        //
        // What eviction must not do is reorder or renumber what it keeps. Monotonicity
        // across the retained window says exactly that, and says it without the test
        // having to claim it is the only thing logging in the process.
        assert!(
            buf.iter()
                .zip(buf.iter().skip(1))
                .all(|(a, b)| a.seq < b.seq),
            "eviction keeps the retained entries in seq order"
        );
    }
}
