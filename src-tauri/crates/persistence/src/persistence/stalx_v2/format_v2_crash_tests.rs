//! The durable save and the external-modification guard: after any failure
//! or crash the destination holds exactly the old or exactly the new file,
//! and a torn temp is refused by the decoder.

use std::path::Path;

use super::atomic::{Stage, AFTER_GUARD, FAIL_AT};
use super::test_helpers::{generate, SMALL};
use super::{
    check_unchanged, decode, encode, temp_path, write_atomic, DecodeOptions, Fingerprint, Guard,
    Mode, WriteError,
};

fn files(seed: u64) -> (Vec<u8>, Vec<u8>) {
    let (p, extras) = generate(seed, SMALL);
    let old = encode(&p, &extras, Mode::Normal).unwrap();
    let mut q = p;
    q.name = "after the save".to_string();
    let new = encode(&q, &extras, Mode::Normal).unwrap();
    (old, new)
}

fn name_on_disk(path: &Path) -> String {
    decode(&std::fs::read(path).unwrap(), &DecodeOptions::default())
        .unwrap()
        .project
        .name
}

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn a_save_replaces_the_file_and_leaves_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cave.stalx");
    let (old, new) = files(1);
    let first = write_atomic(&dest, &old, Guard::Absent).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), old);
    let (_, read) = Fingerprint::read(&dest, u64::MAX).unwrap();
    assert!(first.same_content(&read));
    let second = write_atomic(&dest, &new, Guard::Expect(&first)).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), new);
    assert!(!second.same_content(&first));
    assert_eq!(entries(dir.path()), vec!["cave.stalx".to_string()]);
    assert_eq!(temp_path(&dest), dir.path().join("cave.stalx.tmp"));
}

#[test]
fn the_guard_refuses_an_externally_modified_file() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cave.stalx");
    let (old, new) = files(2);
    let saved = write_atomic(&dest, &old, Guard::Absent).unwrap();
    // Same size, one byte different: only the hash tells.
    let mut edited = old.clone();
    let at = edited.len() / 2;
    edited[at] = if edited[at] == b'1' { b'2' } else { b'1' };
    std::fs::write(&dest, &edited).unwrap();
    match write_atomic(&dest, &new, Guard::Expect(&saved)) {
        Err(WriteError::ExternallyModified {
            found_len: Some(_), ..
        }) => {}
        other => panic!("{other:?}"),
    }
    assert_eq!(
        std::fs::read(&dest).unwrap(),
        edited,
        "the external edit is kept"
    );
    assert_eq!(entries(dir.path()), vec!["cave.stalx".to_string()]);
    // Deleted meanwhile.
    std::fs::remove_file(&dest).unwrap();
    assert!(matches!(
        write_atomic(&dest, &new, Guard::Expect(&saved)),
        Err(WriteError::ExternallyModified {
            found_len: None,
            ..
        })
    ));
    // A first save does not replace a file that appeared meanwhile.
    std::fs::write(&dest, b"someone else's").unwrap();
    assert!(matches!(
        write_atomic(&dest, &new, Guard::Absent),
        Err(WriteError::Exists { .. })
    ));
    // Touched but unchanged content passes.
    let saved = write_atomic(&dest, &old, Guard::Overwrite).unwrap();
    std::fs::write(&dest, &old).unwrap();
    check_unchanged(&dest, &saved).unwrap();
}

/// Pre-PR review finding 6: a first save (`Guard::Absent`) checked that
/// nothing was there, then renamed over whatever appeared in between. On
/// Unix the last step itself refuses a taken name now (a hard link), so a
/// file another program created after the check is kept and the save is
/// refused as `Exists`, with no temp left behind. Injection: a plain rename
/// as the last step replaces "someone else's" here.
#[cfg(unix)]
#[test]
fn a_first_save_never_replaces_a_file_that_appears_after_the_check() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cave.stalx");
    let (old, _) = files(4);
    AFTER_GUARD.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|dest: &Path| {
            std::fs::write(dest, b"someone else's").unwrap();
        }));
    });
    let result = write_atomic(&dest, &old, Guard::Absent);
    AFTER_GUARD.with(|hook| hook.borrow_mut().take());
    assert!(
        matches!(result, Err(WriteError::Exists { .. })),
        "{result:?}"
    );
    assert_eq!(std::fs::read(&dest).unwrap(), b"someone else's");
    assert_eq!(entries(dir.path()), vec!["cave.stalx".to_string()]);

    // A dangling link counts as taken too, as the check says.
    std::fs::remove_file(&dest).unwrap();
    AFTER_GUARD.with(|hook| {
        *hook.borrow_mut() = Some(Box::new(|dest: &Path| {
            std::os::unix::fs::symlink("nowhere.stalx", dest).unwrap();
        }));
    });
    let result = write_atomic(&dest, &old, Guard::Absent);
    AFTER_GUARD.with(|hook| hook.borrow_mut().take());
    assert!(
        matches!(result, Err(WriteError::Exists { .. })),
        "{result:?}"
    );
    assert!(std::fs::symlink_metadata(&dest)
        .unwrap()
        .file_type()
        .is_symlink());

    // Nothing in the way: the first save lands, under its name only.
    std::fs::remove_file(&dest).unwrap();
    let saved = write_atomic(&dest, &old, Guard::Absent).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), old);
    let (_, read) = Fingerprint::read(&dest, u64::MAX).unwrap();
    assert!(saved.same_content(&read));
    assert_eq!(entries(dir.path()), vec!["cave.stalx".to_string()]);
}

#[test]
fn a_failure_at_any_step_keeps_the_old_file() {
    for stage in [
        Stage::CreateTemp,
        Stage::WriteTemp,
        Stage::SyncTemp,
        Stage::Guard,
        Stage::Rename,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("cave.stalx");
        let (old, new) = files(3);
        let saved = write_atomic(&dest, &old, Guard::Absent).unwrap();
        FAIL_AT.with(|f| f.set(Some(stage)));
        let result = write_atomic(&dest, &new, Guard::Expect(&saved));
        FAIL_AT.with(|f| f.set(None));
        assert!(result.is_err(), "{stage:?}");
        assert_eq!(std::fs::read(&dest).unwrap(), old, "{stage:?}");
        assert_eq!(
            entries(dir.path()),
            vec!["cave.stalx".to_string()],
            "{stage:?}: the temp is cleaned up"
        );
    }
}

/// The on-disk states a crash at each boundary leaves, built directly.
#[test]
fn crash_states_read_as_old_or_new_and_torn_temps_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cave.stalx");
    let temp = temp_path(&dest);
    let (old, new) = files(4);
    std::fs::write(&dest, &old).unwrap();
    // Killed while writing the temp: any prefix of the new bytes.
    for cut in (0..new.len() - 1).step_by(7) {
        std::fs::write(&temp, &new[..cut]).unwrap();
        assert!(
            decode(&new[..cut], &DecodeOptions::default()).is_err(),
            "cut {cut}"
        );
        assert_eq!(name_on_disk(&dest), name_on_disk_of(&old));
    }
    // Killed after the temp's fsync, before the rename: old destination, a
    // complete temp that reads as the interrupted save.
    std::fs::write(&temp, &new).unwrap();
    assert_eq!(name_on_disk(&dest), name_on_disk_of(&old));
    assert_eq!(name_on_disk(&temp), "after the save");
    // The next save sweeps the leftover and lands.
    let saved = write_atomic(&dest, &new, Guard::Overwrite).unwrap();
    assert_eq!(name_on_disk(&dest), "after the save");
    assert!(!temp.exists());
    check_unchanged(&dest, &saved).unwrap();
}

fn name_on_disk_of(bytes: &[u8]) -> String {
    decode(bytes, &DecodeOptions::default())
        .unwrap()
        .project
        .name
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_temp_name_is_removed_not_followed() {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("cave.stalx");
    let victim = dir.path().join("victim");
    std::fs::write(&victim, b"not yours").unwrap();
    std::os::unix::fs::symlink(&victim, temp_path(&dest)).unwrap();
    let (old, _) = files(5);
    std::fs::write(&dest, b"x").unwrap();
    std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600)).unwrap();
    write_atomic(&dest, &old, Guard::Overwrite).unwrap();
    assert_eq!(std::fs::read(&victim).unwrap(), b"not yours");
    assert_eq!(std::fs::read(&dest).unwrap(), old);
    let mode = std::fs::metadata(&dest).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600, "the file keeps its permissions");
}

#[test]
fn reading_a_fingerprint_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("big.stalx");
    std::fs::write(&path, vec![b' '; 4096]).unwrap();
    assert!(Fingerprint::read(&path, 1024).is_err());
    let (bytes, fingerprint) = Fingerprint::read(&path, 4096).unwrap();
    assert_eq!(bytes.len(), 4096);
    assert_eq!(fingerprint.len, 4096);
}
