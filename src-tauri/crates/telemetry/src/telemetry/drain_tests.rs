//! [#576 B17] Neither startup nor quit may wait on the telemetry endpoint.
//!
//! The drain of the disk buffer is bounded by a wall-clock budget (a slow
//! endpoint that still answers never trips the stop-on-failure rule), and the
//! app-close path writes the pending buckets to that buffer instead of
//! posting them. The unit tests drive the drain with a fake sender; the
//! ignored end-to-end test runs the real HTTP path against a slow local
//! endpoint and needs a build pointed at it.

use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use stalagmight_models::test_support::missing_prerequisite;
use tempfile::TempDir;

use super::{
    buffer_payload_at, drain_buffer_dir, telemetry_buffer_dir, TelemetryCollector,
    TelemetryPayload, ENDPOINT_URL, HTTP_TIMEOUT, PAYLOAD_VERSION,
};

fn payload(project_seq: u32) -> TelemetryPayload {
    TelemetryPayload {
        v: PAYLOAD_VERSION,
        app_version: "0.0.0-test".to_string(),
        os: "linux".to_string(),
        session_id: "00000000-0000-0000-0000-000000000000".to_string(),
        duration_secs: 1,
        project_seq,
        events: HashMap::from([("command.save_project".to_string(), 1)]),
    }
}

fn json_files(dir: &Path) -> Vec<std::path::PathBuf> {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.extension().is_some_and(|x| x == "json"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn the_drain_stops_at_its_deadline_and_leaves_the_rest_buffered() {
    // An endpoint that answers every payload, slowly. Stopping on the first
    // retryable failure never fires, so only the deadline bounds the pass.
    let temp = TempDir::new().unwrap();
    for i in 0..10u128 {
        buffer_payload_at(temp.path(), Some(1_000 + i), &payload(0));
    }
    let budget = Duration::from_millis(350);
    let per_send = Duration::from_millis(100);

    let started = Instant::now();
    let mut timeouts = Vec::new();
    let sent = drain_buffer_dir(temp.path(), started + budget, |_, timeout| {
        timeouts.push(timeout);
        std::thread::sleep(per_send);
        Ok(())
    });
    let elapsed = started.elapsed();

    assert!(
        (1..10).contains(&sent),
        "sent {sent} of 10 inside a {budget:?} budget"
    );
    assert_eq!(
        json_files(temp.path()).len(),
        10 - sent,
        "what was not sent stays buffered for the next pass"
    );
    // One send may start just inside the deadline; nothing starts after it.
    assert!(
        elapsed < budget + per_send + Duration::from_millis(150),
        "the pass ran {elapsed:?} on a {budget:?} budget"
    );
    // And each send is told how long it may take: never past the deadline.
    assert!(
        timeouts.iter().all(|t| *t <= budget && *t <= HTTP_TIMEOUT),
        "{timeouts:?}"
    );
}

#[test]
fn closing_the_app_buffers_every_bucket_without_sending() {
    let temp = TempDir::new().unwrap();
    let collector = TelemetryCollector::new(true);
    collector.track("command.save_project");
    collector.track_batch(&HashMap::from([("action.a:toolbar".to_string(), 2)]), 1);
    collector.track_batch(&HashMap::from([("action.b:toolbar".to_string(), 3)]), 2);

    collector.buffer_pending_to(temp.path());

    let mut buffered: Vec<(u32, HashMap<String, u64>)> = json_files(temp.path())
        .iter()
        .map(|path| {
            let payload: TelemetryPayload =
                serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
            (payload.project_seq, payload.events)
        })
        .collect();
    buffered.sort_by_key(|(seq, _)| *seq);
    assert_eq!(
        buffered,
        vec![
            (0, HashMap::from([("command.save_project".to_string(), 1)])),
            (1, HashMap::from([("action.a:toolbar".to_string(), 2)])),
            (2, HashMap::from([("action.b:toolbar".to_string(), 3)])),
        ],
        "one payload per project bucket, each under its own sequence"
    );
    assert!(
        collector.inner.lock().events.is_empty(),
        "handed to the buffer, so not sent again by a later flush"
    );
}

#[test]
fn closing_with_telemetry_off_buffers_nothing() {
    let temp = TempDir::new().unwrap();
    let collector = TelemetryCollector::new(true);
    collector.track("command.save_project");
    collector.set_enabled(false);

    collector.buffer_pending_to(temp.path());

    assert!(json_files(temp.path()).is_empty());
}

/// The endpoint this binary was built for, as `127.0.0.1:<port>`, when it is
/// a local one the end-to-end test can stand in for.
fn local_endpoint() -> Option<String> {
    let rest = ENDPOINT_URL.strip_prefix("http://127.0.0.1:")?;
    let port = rest.split('/').next()?;
    port.parse::<u16>().ok()?;
    Some(format!("127.0.0.1:{port}"))
}

/// An HTTP endpoint that reads each request and answers `204` after `latency`.
/// Returns the number of requests it has answered so far.
fn slow_endpoint(addr: &str, latency: Duration) -> Arc<AtomicUsize> {
    let listener = TcpListener::bind(addr).unwrap();
    let answered = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&answered);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let counter = Arc::clone(&counter);
            std::thread::spawn(move || {
                let mut reader = BufReader::new(stream);
                let mut body_len = 0usize;
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 {
                        return;
                    }
                    let line = line.trim_end().to_ascii_lowercase();
                    if line.is_empty() {
                        break;
                    }
                    if let Some(len) = line.strip_prefix("content-length:") {
                        body_len = len.trim().parse().unwrap_or(0);
                    }
                }
                let mut body = vec![0u8; body_len];
                let _ = reader.read_exact(&mut body);
                std::thread::sleep(latency);
                counter.fetch_add(1, Ordering::SeqCst);
                let _ = reader.into_inner().write_all(
                    b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            });
        }
    });
    answered
}

/// What the app-close command (`flush_telemetry`) does with the collector.
fn app_close(collector: &TelemetryCollector) {
    collector.buffer_pending();
}

#[test]
#[ignore = "end-to-end: build with TELEMETRY_ENDPOINT=http://127.0.0.1:18765/ingest and run \
            with --ignored --nocapture (the endpoint is compiled in)"]
fn neither_startup_nor_quit_waits_on_a_slow_endpoint() {
    let Some(addr) = local_endpoint() else {
        missing_prerequisite(&format!(
            "a build with TELEMETRY_ENDPOINT=http://127.0.0.1:<port>/... (this one was built \
             for {ENDPOINT_URL})"
        ));
    };
    const BUFFERED: u128 = 30;
    let latency = Duration::from_secs(1);
    let answered = slow_endpoint(&addr, latency);

    // The real buffer directory, pointed at a temp dir before anything reads it.
    let home = TempDir::new().unwrap();
    crate::app_dirs::init_paths(
        home.path().join("data"),
        home.path().join("config"),
        home.path().join("cache"),
        None,
    );
    let buffer = telemetry_buffer_dir().unwrap();
    for i in 0..BUFFERED {
        buffer_payload_at(&buffer, Some(1_000 + i), &payload(0));
    }
    let collector = TelemetryCollector::new(true);

    // The drain of what earlier sessions left: synchronous in Tauri's setup
    // before #576 B17, the flush thread's first job after it.
    let started = Instant::now();
    collector.send_buffered();
    let drain = started.elapsed();
    println!(
        "buffer drain: {BUFFERED} buffered payloads, endpoint answers in {latency:?}: \
         send_buffered() returned after {drain:.1?}, {} sent, {} still buffered",
        answered.load(Ordering::SeqCst),
        json_files(&buffer).len()
    );

    // App close with three project buckets pending.
    collector.track("command.save_project");
    collector.track_batch(&HashMap::from([("action.a:toolbar".to_string(), 2)]), 1);
    collector.track_batch(&HashMap::from([("action.b:toolbar".to_string(), 3)]), 2);
    let (files_before, answered_before) =
        (json_files(&buffer).len(), answered.load(Ordering::SeqCst));
    let started = Instant::now();
    app_close(&collector);
    let quit = started.elapsed();
    let requests = answered.load(Ordering::SeqCst) - answered_before;
    let written = json_files(&buffer).len().saturating_sub(files_before);
    println!(
        "app close with 3 project buckets pending: took {quit:.1?}, {requests} request(s) to \
         the endpoint, {written} payload file(s) buffered for the next launch"
    );

    assert!(
        drain < Duration::from_secs(25),
        "the buffer drain held its thread for {drain:?}"
    );
    assert!(
        requests == 0 && quit < Duration::from_secs(1),
        "app close waited {quit:?} on {requests} request(s)"
    );
}
