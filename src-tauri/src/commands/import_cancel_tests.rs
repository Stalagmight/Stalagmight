//! [#576 NF-FIX3A-2] The loading overlay's Cancel stops a file import between
//! its phases, with nothing committed.
//!
//! `cancel_loading` flips the token of the active load it names, and only a project open
//! or create ever registered one: an import ran on regardless, so Cancel cleared
//! the overlay while the import went on and landed anyway. The import runner now
//! registers its own token and checks it between phases; past the point the
//! import is applied, it completes. All data is synthetic, built in code.

use std::path::Path;
use std::sync::Arc;

use crate::commands::import_runner::{
    run_file_import, ApplyPath, ConvertedImport, ImportKind, SilentSink,
};
use crate::core::state::{AppState, VirtualShotMode};
use crate::models::{ImportPlacement, ImportSectionConfig, ShotData};
use stalagmight_core::test_fixtures::state_with_project;

const TEST_IMPORT: ImportKind = ImportKind {
    receipt_label: "Synthetic",
    error_label: "Synthetic",
    purpose: "import Synthetic",
};

/// One synthetic section of two shots, placed at the origin.
fn one_section() -> ConvertedImport {
    let shot = ShotData {
        azimuth_in: 90.0,
        azimuth_out: 270.0,
        length: 5.0,
        ..Default::default()
    };
    ConvertedImport {
        sections: vec![ImportSectionConfig {
            reverse: false,
            section_index: 0,
            name: Some("SectionC".to_string()),
            surveyor: None,
            date: None,
            color: "#123456".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![shot.clone(), shot],
            placement: ImportPlacement::Origin,
            station_comments: Vec::new(),
            azimuth_reference: None,
        }],
        closures: Vec::new(),
        warnings: Vec::new(),
        sections_found: 1,
        unit_conversion: None,
        apply: ApplyPath::Sections,
    }
}

fn import_cancelling_during(state: &AppState, during_convert: bool) -> crate::core::CommandResult {
    run_file_import(
        state,
        Arc::new(SilentSink),
        Some("import"),
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| {
            if !during_convert {
                // The user presses the overlay's Cancel while the file is read.
                state.cancel_loading(Some("import"));
            }
            Ok(())
        },
        |()| {
            if during_convert {
                state.cancel_loading(Some("import"));
            }
            Ok(one_section())
        },
    )
}

#[test]
fn cancel_during_an_import_phase_commits_nothing() {
    for (phase, during_convert) in [("reading", false), ("converting", true)] {
        let state = state_with_project("CaveC");
        let result = import_cancelling_during(&state, during_convert);
        let sections = state.get_project_state().expect("project").sections.len();
        println!(
            "Cancel while {phase}: success={} error={:?} sections in the project={sections} \
             undo available={}",
            result.success,
            result.error,
            state.can_undo()
        );
        assert!(
            !result.success,
            "{phase}: a cancelled import does not succeed"
        );
        assert_eq!(
            result.code,
            Some(crate::core::CommandErrorCode::ImportCancelled),
            "{phase}: the failure says it was cancelled: {:?}",
            result.error
        );
        assert_eq!(sections, 0, "{phase}: nothing was imported");
        assert!(!state.can_undo(), "{phase}: and nothing was recorded");
    }
}

/// [#576 C4] The cancelled import's message is display text: the kind of failure
/// is its code, so no marker leads the sentence the import dialog shows.
#[test]
fn a_cancelled_imports_message_carries_no_marker() {
    for (phase, during_convert) in [("reading", false), ("converting", true)] {
        let state = state_with_project("CaveC");
        let result = import_cancelling_during(&state, during_convert);
        let error = result.error.clone().unwrap_or_default();
        println!(
            "Cancel while {phase}: code={:?} message={error:?}",
            result.code
        );
        assert!(
            !result.success,
            "{phase}: a cancelled import does not succeed"
        );
        assert!(
            !error.starts_with("IMPORT_CANCELLED") && !error.contains("IMPORT_CANCELLED:"),
            "{phase}: the message carries the IMPORT_CANCELLED marker: {error:?}"
        );
        assert!(
            error.contains("was cancelled; nothing was imported"),
            "{phase}: the message still says what happened: {error:?}"
        );
        // ...and the IPC error carries the code the dialog routes on.
        let wire = result.into_error();
        assert_eq!(wire.code, crate::core::CommandErrorCode::ImportCancelled);
    }
}

#[test]
fn an_import_nobody_cancels_still_lands_and_frees_its_token() {
    let state = state_with_project("CaveC");
    let result = run_file_import(
        &state,
        Arc::new(SilentSink),
        Some("import"),
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| Ok(()),
        |()| Ok(one_section()),
    );
    assert!(result.success, "{:?}", result.error);
    assert_eq!(
        state.get_project_state().expect("project").sections.len(),
        1
    );

    // The import's token is released: a Cancel pressed afterwards reaches no one,
    // and the next import is not born cancelled. That import has a name of its own,
    // as every import does (`nameImport`: a new `newLoadToken` per import).
    state.cancel_loading(Some("import"));
    let again = run_file_import(
        &state,
        Arc::new(SilentSink),
        Some("next-import"),
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| Ok(()),
        |()| Ok(one_section()),
    );
    assert!(again.success, "{:?}", again.error);
    assert_eq!(
        state.get_project_state().expect("project").sections.len(),
        2
    );
}

/// An import named `load_token` of one synthetic section that nobody cancels while
/// it runs.
fn import_named(state: &AppState, load_token: Option<&str>) -> crate::core::CommandResult {
    run_file_import(
        state,
        Arc::new(SilentSink),
        load_token,
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| Ok(()),
        |()| Ok(one_section()),
    )
}

/// A Cancel that reaches the backend before its import registered (the import's
/// command still waiting for the blocking pool, or its IPC still in the frontend's
/// edit queue) stops that import when it registers, and nothing else (Codex review
/// on #630). It found no load of that name in progress and did nothing, so the
/// import landed.
#[test]
fn a_cancel_sent_before_its_import_registered_stops_that_import_alone() {
    let state = state_with_project("CaveC");
    // The Cancel arrives first; a tokenless one alongside it names nothing.
    state.cancel_loading(Some("early-import"));
    state.cancel_loading(None);

    // Another import, and an unnamed one, run while it waits: neither is stopped.
    let other = import_named(&state, Some("other-import"));
    let unnamed = import_named(&state, None);
    let sections_before = state.get_project_state().expect("project").sections.len();

    // The import it names registers now: stopped, with nothing committed.
    let named = import_named(&state, Some("early-import"));
    let sections_after = state.get_project_state().expect("project").sections.len();

    // The Cancel was spent on that import: it stops one import, not a name forever.
    let rerun = import_named(&state, Some("early-import"));
    println!(
        "early Cancel: other import success={}; unnamed import success={}; the named \
         import code={:?} error={:?}, sections {sections_before} -> {sections_after}; a \
         later run of that name success={}",
        other.success, unnamed.success, named.code, named.error, rerun.success
    );
    assert!(
        other.success,
        "another import is not stopped: {:?}",
        other.error
    );
    assert!(
        unnamed.success,
        "an unnamed import is not stopped: {:?}",
        unnamed.error
    );
    assert_eq!(
        named.code,
        Some(crate::core::CommandErrorCode::ImportCancelled),
        "the import the early Cancel named is the cancelled import: {:?}",
        named.error
    );
    assert_eq!(sections_after, sections_before, "and it imported nothing");
    assert!(rerun.success, "the Cancel is spent: {:?}", rerun.error);
}

/// [#576 RV20-17] The parse runs under the import's Cancel flag, which the
/// parsers' wait for a file lease watches, and a parse that gives up because
/// Cancel was pressed ends as the cancelled import, not as a failed parse. The
/// wait took no flag, so a Cancel pressed during it waited the open out.
#[test]
fn a_parse_that_gives_up_on_cancel_ends_as_the_cancelled_import() {
    let state = state_with_project("CaveC");
    let mut seen_by_the_parse = None;
    let result = run_file_import(
        &state,
        Arc::new(SilentSink),
        Some("import"),
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| {
            // Cancel pressed while the file's open waits; the wait gives up.
            state.cancel_loading(Some("import"));
            seen_by_the_parse = Some(crate::parsers::cancel_pressed());
            Err::<(), _>(anyhow::anyhow!("reading /tmp/synthetic.dat: cancelled"))
        },
        |()| Ok(one_section()),
    );
    println!(
        "parse gave up on Cancel: Cancel seen by the parse {seen_by_the_parse:?}; code {:?}; \
         message {:?}",
        result.code, result.error
    );
    assert_eq!(
        seen_by_the_parse,
        Some(true),
        "the parse runs under the import's Cancel flag"
    );
    assert_eq!(
        result.code,
        Some(crate::core::CommandErrorCode::ImportCancelled),
        "the outcome is the cancelled import: {:?}",
        result.error
    );
}
