//! [#576 RV5-RL-5] The permission preconditions accept only a PERMISSION refusal.
//!
//! `assert_dir_refuses_new_files` and `assert_file_refuses_writes` passed on any
//! error, so a call pointed at a mistyped or missing path - which refuses a new
//! file or a write just as well - passed without the permission it is about ever
//! being tested. They now require `ErrorKind::PermissionDenied`, like
//! `assert_lookup_refused`. All paths are synthetic, under the system temp dir.

use std::path::PathBuf;

use crate::test_support::{assert_dir_refuses_new_files, assert_file_refuses_writes};

/// A path under the system temp dir that does not exist.
fn missing(tag: &str) -> PathBuf {
    let path = std::env::temp_dir()
        .join(format!(
            "stalagmight-rl5-never-created-{tag}-{}",
            std::process::id()
        ))
        .join("CaveA");
    assert!(!path.exists(), "premise: {} is absent", path.display());
    path
}

#[test]
#[should_panic(expected = "not a permission refusal")]
fn a_missing_dir_fails_the_dir_precondition() {
    assert_dir_refuses_new_files(&missing("dir"));
}

#[test]
#[should_panic(expected = "not a permission refusal")]
fn a_missing_file_fails_the_file_precondition() {
    assert_file_refuses_writes(&missing("file").join("CaveA.stalx"));
}
