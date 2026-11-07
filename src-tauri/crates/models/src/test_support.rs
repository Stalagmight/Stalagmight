//! Helpers shared by the workspace's tests. Compiled for this crate's own
//! tests and for every dependent that enables `test-support` in its
//! `[dev-dependencies]`; production builds never see this module.
//!
//! # Opt-in prerequisites (#576 E13)
//!
//! An `#[ignore]`d test that needs something from outside the repo - a live
//! server's credentials, a private survey file, a corpus directory - takes it
//! through [`require_env`], [`require_file`] or [`require_env_file`]. Each
//! panics naming exactly what is missing.
//!
//! `#[ignore]` is already the skip: a default `cargo test` never runs these.
//! Someone who runs one on purpose (`-- --ignored <name>`) and forgot, or
//! mistyped, a variable must see a failure, not an `ok` from a test that
//! returned before its first assertion. The same helpers serve the default
//! tests that read a committed fixture: a fixture that went missing is a
//! failure too, never a silent skip.
//!
//! Blanket `cargo test -- --ignored` therefore fails for every opt-in test
//! whose prerequisites are absent; filter the run to the tests you can feed
//! (docs/testing-recipes.md, "Opt-in tests").
//!
//! # Permission preconditions (#576 NF-ROOT-PERM)
//!
//! A test that takes a permission away with mode bits - a read-only folder, a
//! 0444 file, an unsearchable directory - relies on the process honouring
//! them. Root, and any process holding `CAP_DAC_OVERRIDE`, does not: run that
//! way, such a test passes without ever meeting the refusal it is about. Each
//! one confirms the bits bite right after setting them, through
//! [`assert_dir_refuses_new_files`], [`assert_file_refuses_writes`] or
//! [`assert_lookup_refused`], which fail the test otherwise. CI runs the Rust
//! tests as an unprivileged user for this reason
//! (`scripts/ci-cargo-test-unprivileged.sh`).
//!
//! # Section lookups (#576 E9(c))
//!
//! [`section_named`] and [`section_by_id`] find a section by what the test
//! means, not by where `Project::sections` happens to store it. New tests use
//! them rather than `sections[N]`.

use std::path::{Path, PathBuf};

use crate::project::Project;
use crate::section::{Section, SectionId};

/// Fail the calling test because `what` - an environment variable, a file, a
/// build-time setting - is not there. The single exit every helper here
/// takes, for the cases they do not cover themselves.
///
/// # Panics
///
/// Always: failing the calling test, with `what` in the message, is the point.
#[track_caller]
#[expect(
    clippy::panic,
    reason = "a test helper: failing the calling test is its whole job"
)]
pub fn missing_prerequisite(what: &str) -> ! {
    panic!(
        "missing test prerequisite: {what}. A test run without what it needs fails \
         instead of passing having tested nothing; provide it, or run only the tests you \
         can feed (docs/testing-recipes.md, \"Opt-in tests\")."
    )
}

/// The value of the environment variable `var`. Unset, empty and non-UTF-8
/// all count as missing: none of them is something a test can use.
///
/// # Panics
///
/// Through [`missing_prerequisite`] when it is not there.
#[track_caller]
pub fn require_env(var: &str) -> String {
    match std::env::var(var) {
        Ok(value) if !value.is_empty() => value,
        Ok(_) => missing_prerequisite(&format!("environment variable {var} is set but empty")),
        Err(std::env::VarError::NotPresent) => {
            missing_prerequisite(&format!("environment variable {var} is not set"))
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            missing_prerequisite(&format!("environment variable {var} is not valid UTF-8"))
        }
    }
}

/// `path`, which must exist (a file or a directory).
///
/// # Panics
///
/// Through [`missing_prerequisite`] when it is not there.
#[track_caller]
pub fn require_file(path: impl AsRef<Path>) -> PathBuf {
    let path = path.as_ref();
    match path.try_exists() {
        Ok(true) => path.to_path_buf(),
        Ok(false) => missing_prerequisite(&format!("{} does not exist", path.display())),
        Err(e) => missing_prerequisite(&format!("{} cannot be checked: {e}", path.display())),
    }
}

/// The path the environment variable `var` names, which must exist.
///
/// # Panics
///
/// Through [`missing_prerequisite`] when it is not there.
#[track_caller]
pub fn require_env_file(var: &str) -> PathBuf {
    let path = PathBuf::from(require_env(var));
    match path.try_exists() {
        Ok(true) => path,
        Ok(false) => missing_prerequisite(&format!(
            "{var}={} names a path that does not exist",
            path.display()
        )),
        Err(e) => missing_prerequisite(&format!("{var}={} cannot be checked: {e}", path.display())),
    }
}

/// Fail the calling test because the permission it took away did not bite.
#[track_caller]
#[expect(
    clippy::panic,
    reason = "a test helper: failing the calling test is its whole job"
)]
fn permission_precondition_failed(what: &str) -> ! {
    panic!(
        "permission precondition failed: {what}. This process ignores mode bits (it runs \
         as root, or holds CAP_DAC_OVERRIDE), so the test would pass without meeting the \
         refusal it is about. Run the Rust tests as an unprivileged user, as CI does \
         (scripts/ci-cargo-test-unprivileged.sh)."
    )
}

/// Fail the calling test because a permission probe failed for a reason that is
/// not a permission refusal: the probe was pointed at the wrong path (a typo, a
/// directory that was never created), so the refusal it is about was never tested
/// (#576 RV5-RL-5).
#[track_caller]
#[expect(
    clippy::panic,
    reason = "a test helper: failing the calling test is its whole job"
)]
fn probe_failed_for_another_reason(what: &str, error: &std::io::Error) -> ! {
    panic!(
        "permission precondition probe misdirected: {what} failed with {error} ({:?}), not \
         a permission refusal. Point it at the path whose permission the test took away.",
        error.kind()
    )
}

/// `dir`, whose write permission the caller took away, refuses a new entry.
/// Probes by creating (and, if that works, removing) a file in it.
///
/// # Panics
///
/// When the file can be created, or when creating it fails with anything but
/// `PermissionDenied` (a missing or mistyped `dir` refuses a new file too, and
/// would pass without the permission ever being tested).
#[track_caller]
pub fn assert_dir_refuses_new_files(dir: &Path) {
    let probe = dir.join(".permission-precondition-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            permission_precondition_failed(&format!("{} still accepts new files", dir.display()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(e) => {
            probe_failed_for_another_reason(&format!("creating a file in {}", dir.display()), &e)
        }
    }
}

/// `path`, whose write permission the caller took away, refuses to open for
/// writing. The probe opens without truncating, so it changes nothing.
///
/// # Panics
///
/// When the file opens for writing, or when opening it fails with anything but
/// `PermissionDenied` (a missing file refuses too).
#[track_caller]
pub fn assert_file_refuses_writes(path: &Path) {
    match std::fs::OpenOptions::new().write(true).open(path) {
        Ok(_) => {
            permission_precondition_failed(&format!("{} still opens for writing", path.display()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
        Err(e) => {
            probe_failed_for_another_reason(&format!("opening {} for writing", path.display()), &e)
        }
    }
}

/// `path` cannot even be looked up, because the caller made a directory above
/// it unsearchable: a stat of it fails with a permission error (not "not
/// found", which is what a privileged process sees for a path that is absent).
///
/// # Panics
///
/// When the stat succeeds or fails for another reason.
#[track_caller]
pub fn assert_lookup_refused(path: &Path) {
    match std::fs::symlink_metadata(path) {
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {}
        other => permission_precondition_failed(&format!(
            "looking up {} is not refused ({:?})",
            path.display(),
            other.map(|_| "it succeeded")
        )),
    }
}

/// Fail the calling test because a section lookup found nothing to return.
#[track_caller]
#[expect(
    clippy::panic,
    reason = "a test helper: failing the calling test is its whole job"
)]
fn no_such_section(what: &str, project: &Project) -> ! {
    let have: Vec<(SectionId, Option<&str>)> = project
        .sections
        .iter()
        .map(|s| (s.id, s.name.as_deref()))
        .collect();
    panic!("{what}; the project has (id, name): {have:?}")
}

/// The one section named `name`. Prefer it (or [`section_by_id`]) to
/// `project.sections[N]` in new tests: it says which section the test means,
/// and does not tie the test to the order sections are stored in (#576 E9(c)).
///
/// # Panics
///
/// When no section, or more than one, carries that name.
#[track_caller]
pub fn section_named<'a>(project: &'a Project, name: &str) -> &'a Section {
    let mut matching = project
        .sections
        .iter()
        .filter(|s| s.name.as_deref() == Some(name));
    match (matching.next(), matching.next()) {
        (Some(section), None) => section,
        (None, _) => no_such_section(&format!("no section is named {name:?}"), project),
        (Some(_), Some(_)) => {
            no_such_section(&format!("more than one section is named {name:?}"), project)
        }
    }
}

/// The section whose id is `id`.
///
/// # Panics
///
/// When the project has no such section.
#[track_caller]
pub fn section_by_id(project: &Project, id: SectionId) -> &Section {
    project
        .get_section(id)
        .unwrap_or_else(|| no_such_section(&format!("no section has id {id}"), project))
}

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: a failed setup step is the failure"
)]
mod tests {
    use super::*;

    const UNSET: &str = "STALAGMIGHT_TEST_SUPPORT_NEVER_SET_E13";

    #[test]
    #[should_panic(
        expected = "missing test prerequisite: environment variable STALAGMIGHT_TEST_SUPPORT_NEVER_SET_E13 is not set"
    )]
    fn an_unset_variable_fails_the_test_and_names_the_variable() {
        assert!(
            std::env::var_os(UNSET).is_none(),
            "premise: {UNSET} is unset"
        );
        require_env(UNSET);
    }

    #[test]
    fn a_set_variable_is_returned() {
        // Cargo sets this for every test binary it runs.
        let dir = require_env("CARGO_MANIFEST_DIR");
        assert_eq!(dir, env!("CARGO_MANIFEST_DIR"));
    }

    #[test]
    #[should_panic(expected = "never-there.bin does not exist")]
    fn a_missing_file_fails_the_test_and_names_the_file() {
        require_file(Path::new(env!("CARGO_MANIFEST_DIR")).join("never-there.bin"));
    }

    #[test]
    fn an_existing_file_is_returned() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(require_file(&manifest), manifest);
    }

    #[test]
    #[should_panic(
        expected = "missing test prerequisite: environment variable STALAGMIGHT_TEST_SUPPORT_NEVER_SET_E13 is not set"
    )]
    fn a_file_variable_that_is_unset_names_the_variable() {
        require_env_file(UNSET);
    }

    #[test]
    fn a_file_variable_naming_an_existing_path_is_returned() {
        let dir = require_env_file("CARGO_MANIFEST_DIR");
        assert!(dir.join("Cargo.toml").exists());
    }

    /// A scratch directory under the system temp dir, writable again and
    /// removed on drop (this crate has no `tempfile` dependency).
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "stalagmight-test-support-{tag}-{}",
                std::process::id()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("create the scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(0o755));
            }
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    #[should_panic(expected = "still accepts new files")]
    fn a_writable_dir_fails_the_dir_precondition() {
        let dir = Scratch::new("writable-dir");
        assert_dir_refuses_new_files(&dir.0);
    }

    #[test]
    #[should_panic(expected = "still opens for writing")]
    fn a_writable_file_fails_the_file_precondition() {
        let dir = Scratch::new("writable-file");
        let file = dir.0.join("CaveA.stalx");
        std::fs::write(&file, b"x").expect("write the file");
        assert_file_refuses_writes(&file);
    }

    #[test]
    #[should_panic(expected = "is not refused")]
    fn a_path_that_can_be_looked_up_fails_the_lookup_precondition() {
        assert_lookup_refused(Path::new(env!("CARGO_MANIFEST_DIR")));
    }

    /// The other side of the three above, and a canary for the environment:
    /// run as root these fail, which is the point - a permission test cannot
    /// mean anything there (CI runs the tests unprivileged).
    #[cfg(unix)]
    #[test]
    fn mode_bits_that_bite_satisfy_every_precondition() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = Scratch::new("read-only");
        let file = dir.0.join("CaveA.stalx");
        std::fs::write(&file, b"x").expect("write the file");
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444))
            .expect("make the file read-only");
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o555))
            .expect("make the dir read-only");
        assert_dir_refuses_new_files(&dir.0);
        assert_file_refuses_writes(&file);
        std::fs::set_permissions(&dir.0, std::fs::Permissions::from_mode(0o000))
            .expect("make the dir unsearchable");
        assert_lookup_refused(&file);
    }

    /// Two sections named A and B, plus a second B; A was added second, so a
    /// positional `sections[0]` would not find it.
    fn caves() -> (Project, SectionId) {
        let mut project = Project::new("CaveA".to_string(), None);
        let mut add = |name: &str| {
            project
                .add_section(crate::section::SectionData {
                    name: Some(name.to_string()),
                    ..Default::default()
                })
                .id
        };
        add("B");
        let a = add("A");
        add("B");
        (project, a)
    }

    #[test]
    fn a_section_is_found_by_its_name_and_by_its_id() {
        let (project, a) = caves();
        assert_eq!(section_named(&project, "A").id, a);
        assert_eq!(section_by_id(&project, a).name.as_deref(), Some("A"));
    }

    #[test]
    #[should_panic(expected = "no section is named \"C\"")]
    fn a_missing_name_fails_and_lists_the_sections() {
        section_named(&caves().0, "C");
    }

    #[test]
    #[should_panic(expected = "more than one section is named \"B\"")]
    fn an_ambiguous_name_fails_rather_than_picking_one() {
        section_named(&caves().0, "B");
    }

    #[test]
    #[should_panic(expected = "no section has id 999")]
    fn a_missing_id_fails() {
        section_by_id(&caves().0, 999);
    }
}
