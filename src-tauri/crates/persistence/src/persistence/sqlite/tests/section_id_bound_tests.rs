//! Section ids at and past the feature-id encoding bound.
//!
//! Lines 8559-8709 of the former single-file `persistence/sqlite/tests.rs` (#576 AFK-D3), moved
//! verbatim; shared helpers live in the module root (`tests.rs`).

use super::*;

/// #557: the mint-time bound (#556) vets what the APP creates, not what a FILE
/// carries - a pre-#556 build, another tool, or a hostile author can persist a
/// section id past the shot-id encode. `Section::new` computes
/// `id * FEATURE_ID_STRIDE` unchecked (debug panic, release wrap), so the open
/// must refuse first, with a message naming the id and the bound.
#[test]
fn an_over_bound_section_id_refuses_the_open() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("overbound.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Overbound".to_string(), None);
    db.save_project(&project).unwrap();

    let hostile = crate::models::MAX_SECTION_ID + 1;
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![hostile],
        )
        .unwrap();

    let err = db.load_project().unwrap_err().to_string();
    assert!(
        err.contains(&hostile.to_string()),
        "the refusal must name the offending id: {err}"
    );
    assert!(
        err.contains(&crate::models::MAX_SECTION_ID.to_string()),
        "the refusal must name the bound: {err}"
    );
}

/// #557: `u32::MAX` is the id that used to overflow the counter rebuild's
/// `id + 1` (debug panic; release wrap to 0, after which the kept counter is
/// whatever smaller ids produced and future mints could REUSE live section
/// ids). It is refused like any other over-bound id, before the counter runs,
/// so debug and release agree.
#[test]
fn a_u32_max_section_id_is_refused_before_the_counter_rebuild() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("umax.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Umax".to_string(), None);
    db.save_project(&project).unwrap();

    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![u32::MAX],
        )
        .unwrap();

    let err = db.load_project().unwrap_err().to_string();
    assert!(
        err.contains(&u32::MAX.to_string()),
        "the refusal must name the offending id: {err}"
    );
    assert!(
        err.contains(&crate::models::MAX_SECTION_ID.to_string()),
        "the refusal must name the bound: {err}"
    );
}

/// The boundary id itself is legal - the refusal must not be off by one, and
/// the counter rebuild must come back one past it.
#[test]
fn the_last_encodable_section_id_still_opens() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("boundary.stalx");
    let mut db = Database::open(&path).unwrap();
    let project = Project::new("Boundary".to_string(), None);
    db.save_project(&project).unwrap();

    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![crate::models::MAX_SECTION_ID],
        )
        .unwrap();

    let loaded = db.load_project().unwrap();
    assert!(
        loaded
            .sections
            .iter()
            .any(|s| s.id == crate::models::MAX_SECTION_ID),
        "the boundary section must load"
    );
    assert_eq!(
        loaded.next_section_id,
        crate::models::MAX_SECTION_ID + 1,
        "the counter rebuild must sit one past the largest loaded id"
    );
}

/// #557: `unrecognised_shot_kinds` runs its own query over the shots table, so
/// it can meet a section id the open-refusal never vetted (the diagnostic can
/// be called without a full load). The encode must saturate, not panic.
#[test]
fn unrecognised_shot_kinds_saturates_an_over_bound_section_id() {
    let db = Database::in_memory().unwrap();
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (?1, '#abcdef', '2026-01-01T00:00:00Z')",
            rusqlite::params![u32::MAX],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, kind)
             VALUES (?1, 0, 0.0, 180.0, 5.0, 0.0, 1.0, 99)",
            rusqlite::params![u32::MAX],
        )
        .unwrap();

    // A second, legal section pins the FORMULA (a bare `shot_id: section_id`
    // would also yield u32::MAX for the hostile row above). Its unreadable
    // shot is the second one in stored order but sits on rank 3, so the id is
    // `7 * STRIDE + 1`: shots are numbered by position, as the loader does
    // (B35(f), #576).
    db.conn
        .execute(
            "INSERT INTO sections (id, color, created_at) VALUES (7, '#abcdef', '2026-01-01T00:00:00Z')",
            [],
        )
        .unwrap();
    db.conn
        .execute(
            "INSERT INTO shots (section_id, sequence_order, azimuth_in, azimuth_out, length, depth_in, depth_out, kind)
             VALUES (7, 1, 0.0, 180.0, 5.0, 0.0, 1.0, 0),
                    (7, 3, 0.0, 180.0, 5.0, 0.0, 1.0, 99)",
            [],
        )
        .unwrap();

    let kinds = db.unrecognised_shot_kinds().unwrap();
    assert_eq!(kinds.len(), 2, "both unrecognised kinds must be reported");
    let legal = kinds.iter().find(|k| k.section_id == 7).unwrap();
    assert_eq!(
        legal.shot_id,
        7 * crate::models::FEATURE_ID_STRIDE + 1,
        "a legal section id must still encode by the stride formula"
    );
    let hostile = kinds.iter().find(|k| k.section_id == u32::MAX).unwrap();
    assert_eq!(
        hostile.shot_id,
        u32::MAX,
        "an over-bound section id saturates the encode instead of panicking"
    );
}
