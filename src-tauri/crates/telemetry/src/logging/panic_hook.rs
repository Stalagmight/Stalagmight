//! Panic observability: route panics through tracing (ring buffer + JSON log
//! file + stderr) and write a structured crash-marker file that a crash oracle
//! (the agent harness) can detect, then delegate to the previously installed
//! hook so stderr printing and unwind/abort behaviour are unchanged.

use std::cell::Cell;
use std::path::{Path, PathBuf};

thread_local! {
    /// How many [`catching`] scopes are open on this thread (#576 RV3-R-12).
    static CATCHING: Cell<u32> = const { Cell::new(0) };
}

/// Run `f`, marking a panic raised on this thread inside it as one a
/// `catch_unwind` around `f` handles (#576 RV3-R-12). The hook runs BEFORE the
/// unwind reaches that `catch_unwind`, so without this a handled panic - an
/// importer failing one file - wrote a crash marker, and the agent harness's
/// crash oracle read it as the app crashing. Inside, the hook logs it as a
/// caught panic (an ERROR line, not `PANIC:`) and writes no marker.
///
/// Only for a closure whose panics are all caught on this same thread: a
/// panic on another thread is not caught here, and is still a crash.
pub fn catching<R>(f: impl FnOnce() -> R) -> R {
    /// Closes the scope on the way out, panicking or not.
    struct Scope;
    impl Drop for Scope {
        fn drop(&mut self) {
            CATCHING.with(|depth| depth.set(depth.get().saturating_sub(1)));
        }
    }
    CATCHING.with(|depth| depth.set(depth.get().saturating_add(1)));
    let _scope = Scope;
    f()
}

/// Whether a panic on this thread now will be caught ([`catching`]).
fn is_caught() -> bool {
    CATCHING.try_with(|depth| depth.get() > 0).unwrap_or(false)
}

/// Install the panic hook. Call once from `run()`, immediately after
/// `logging::init()` so the tracing subscriber is live. Chains to the
/// previously installed hook.
///
/// The crash marker is the durable signal: the JSON file layer is non-blocking
/// and can lose its last lines on abort, whereas the marker's `fs::write` and
/// the ring-buffer layer are synchronous.
pub fn install() {
    install_with_marker_dir(super::log_directory().ok());
}

/// [`install`] with the crash-marker folder spelled out, so a test can watch it.
fn install_with_marker_dir(marker_dir: Option<PathBuf>) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let message = payload_str(info);
        let location = info.location().map_or_else(
            || "unknown".to_string(),
            |l| format!("{}:{}:{}", l.file(), l.line(), l.column()),
        );
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();

        // A panic a guard will catch is a failed step, not a crash: logged
        // distinctly, and no marker (#576 RV3-R-12).
        if is_caught() {
            tracing::error!(
                panic_message = %message,
                panic_location = %location,
                panic_thread = %thread,
                "Caught panic: a guarded step failed on an internal error and was handled"
            );
            previous(info);
            return;
        }
        // force_capture ignores RUST_BACKTRACE so the marker always carries a trace.
        let backtrace = std::backtrace::Backtrace::force_capture().to_string();

        // Write the durable marker FIRST, before any blocking log write. The
        // stderr tracing layer below is synchronous, so if the harness stalls
        // draining the child's stderr pipe, `tracing::error!` could block; doing
        // the marker's synchronous `fs::write` first guarantees the crash signal
        // lands regardless.
        if let Some(dir) = marker_dir.as_deref() {
            write_crash_marker(dir, &message, &location, &thread, &backtrace);
        }

        tracing::error!(
            panic_message = %message,
            panic_location = %location,
            panic_thread = %thread,
            "PANIC: backend thread panicked"
        );

        previous(info);
    }));
}

/// Extract the panic payload as a string, handling the common `&str` / `String`
/// payload types and falling back for anything else.
fn payload_str(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = info.payload().downcast_ref::<String>() {
        s.clone()
    } else {
        "non-string panic payload".to_string()
    }
}

/// Best-effort, synchronous, non-panicking crash-marker writer. Separate fn so
/// it is unit-testable with a temp dir without installing the global hook.
fn write_crash_marker(dir: &Path, message: &str, location: &str, thread: &str, backtrace: &str) {
    let now = chrono::Utc::now();
    let marker = serde_json::json!({
        "ts": now.to_rfc3339(),
        "pid": std::process::id(),
        "message": message,
        "location": location,
        "thread": thread,
        "backtrace": backtrace,
    });
    // [#576 NF-FIX3R-1] A per-process sequence number after the millisecond:
    // two panics in the same millisecond (two threads failing together) used to
    // write the same name, and the second marker replaced the first. Still
    // `crash-<pid>-<ms>...`, which is all the oracle and the log scan match on.
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(
        "crash-{}-{}-{sequence}.json",
        std::process::id(),
        now.timestamp_millis()
    ));
    let _ = std::fs::create_dir_all(dir);
    if let Ok(bytes) = serde_json::to_vec_pretty(&marker) {
        let _ = std::fs::write(path, bytes);
    }
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn write_crash_marker_creates_readable_json() {
        let dir = TempDir::new().expect("temp dir");
        write_crash_marker(dir.path(), "boom", "file.rs:1:2", "worker", "bt-here");

        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .filter_map(Result::ok)
            .collect();
        assert_eq!(entries.len(), 1, "exactly one crash marker written");

        let name = entries[0].file_name();
        let name = name.to_string_lossy();
        assert!(name.starts_with("crash-"), "marker name: {name}");
        assert!(name.ends_with(".json"), "marker name: {name}");

        let content = std::fs::read_to_string(entries[0].path()).expect("read marker");
        let parsed: serde_json::Value = serde_json::from_str(&content).expect("parse json");
        assert_eq!(parsed["message"], "boom");
        assert_eq!(parsed["location"], "file.rs:1:2");
        assert_eq!(parsed["thread"], "worker");
        assert_eq!(parsed["backtrace"], "bt-here");
        assert!(parsed["ts"].is_string());
        assert!(parsed["pid"].is_number());
    }

    /// The crash markers in `dir` whose message is `message`. Other tests in
    /// this process can panic while the hook is installed; only ours count.
    fn markers_for(dir: &Path, message: &str) -> usize {
        std::fs::read_dir(dir)
            .expect("read dir")
            .filter_map(Result::ok)
            .filter(|e| {
                std::fs::read_to_string(e.path())
                    .is_ok_and(|text| text.contains(&format!("\"message\": \"{message}\"")))
            })
            .count()
    }

    /// #576 RV3-R-12: a panic inside [`catching`] - caught by the guard around
    /// it - writes no crash marker; the same panic outside one (here caught
    /// too, but by nobody who said so) still does. The scope closes when the
    /// panic unwinds through it.
    #[test]
    #[expect(
        clippy::panic,
        clippy::print_stdout,
        reason = "the panics are the subject; the printed counts are the evidence"
    )]
    fn a_caught_panic_writes_no_crash_marker() {
        let dir = TempDir::new().expect("temp dir");
        install_with_marker_dir(Some(dir.path().to_path_buf()));

        let caught =
            catching(|| std::panic::catch_unwind(|| panic!("rv3-r-12 caught synthetic panic")));
        let after = is_caught();
        // Matched by message, so the next panic's marker is not counted here.
        // (Since #576 NF-FIX3R-1 a marker name carries a sequence after the
        // millisecond, so a second panic in the same one no longer overwrites it.)
        let caught_markers = markers_for(dir.path(), "rv3-r-12 caught synthetic panic");
        let uncaught = std::panic::catch_unwind(|| panic!("rv3-r-12 unmarked synthetic panic"));
        // Back to the default hook for the rest of the process.
        drop(std::panic::take_hook());
        let unmarked_markers = markers_for(dir.path(), "rv3-r-12 unmarked synthetic panic");
        println!("markers: caught panic {caught_markers}, unmarked panic {unmarked_markers}");
        assert!(
            caught.is_err() && uncaught.is_err(),
            "premise: both panicked"
        );
        assert_eq!(caught_markers, 0, "a caught panic is not a crash");
        assert_eq!(
            unmarked_markers, 1,
            "an unmarked panic still writes its marker"
        );
        assert!(
            !after,
            "the scope closes even when the panic unwinds through it"
        );
    }
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::print_stdout,
    reason = "test module: panics on a failed assumption are the point, and the printed \
              values are the evidence a run captures"
)]
mod marker_name_tests;
