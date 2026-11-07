//! [#576 NF-FIX3R-1] Every crash gets its own marker file.
//!
//! Markers were named `crash-<pid>-<millisecond>.json`, so two panics in the same
//! millisecond (two threads failing together, or a panic during a panic's
//! unwinding) wrote the same name and the second replaced the first: the crash
//! oracle saw one crash where there were two.

use super::write_crash_marker;
use tempfile::TempDir;

#[test]
fn panics_in_the_same_millisecond_each_keep_their_marker() {
    let dir = TempDir::new().expect("temp dir");
    for i in 0..50 {
        write_crash_marker(
            dir.path(),
            &format!("boom {i}"),
            "file.rs:1:2",
            "worker",
            "",
        );
    }
    let markers = std::fs::read_dir(dir.path())
        .expect("read dir")
        .filter_map(Result::ok)
        .filter(|e| e.file_name().to_string_lossy().starts_with("crash-"))
        .count();
    println!("50 crash markers written back to back -> {markers} files on disk");
    assert_eq!(markers, 50, "no marker overwrote another");
}
