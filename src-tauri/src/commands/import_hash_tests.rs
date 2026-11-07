//! [#576 RV3-R-11] `calculate_file_hash` reads through the importers' capped,
//! regular-files-only stream. Every import dialog calls it on the path it just
//! parsed; a whole-file `fs::read` blocked on a FIFO swapped in after the
//! parse and read a file grown past its ceiling in full.
//!
//! All files are synthetic bytes written here. No fixture is read.

use sha2::{Digest, Sha256};

use tauri::async_runtime::block_on;

use super::import::calculate_file_hash;

/// The digest is unchanged by streaming: the same as hashing the whole file,
/// for a file spanning many read chunks.
#[test]
fn a_streamed_hash_equals_the_whole_file_hash() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cave.dmp");
    let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 253) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let expected = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(block_on(calculate_file_hash(path)).unwrap(), expected);
}

/// A folder is not a file to hash.
#[test]
fn a_folder_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let err = block_on(calculate_file_hash(dir.path().to_path_buf())).unwrap_err();
    println!("hash of a folder -> {err}");
    assert!(err.message.contains("not a regular file"), "{err}");
}

/// A FIFO is refused at once instead of blocking the command until somebody
/// writes to it. On a timeout the FIFO is opened for writing and closed, so a
/// blocked read sees an empty file and the test fails instead of hanging.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("cave.tml");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success(), "premise: mkfifo made {}", fifo.display());

    let (sender, receiver) = std::sync::mpsc::channel();
    let target = fifo.clone();
    let worker = std::thread::spawn(move || {
        let _ = sender.send(block_on(calculate_file_hash(target)));
    });
    let outcome = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .ok();
    if outcome.is_none() {
        let _ = std::fs::OpenOptions::new().write(true).open(&fifo);
    }
    let _ = worker.join();
    println!("hash of a FIFO -> {outcome:?}");
    let err = outcome
        .expect("must return within five seconds")
        .unwrap_err();
    assert!(err.message.contains("not a regular file"), "{err}");
}
