//! [#576 NF-FIX3A-2] The loading overlay's Cancel stops a file import between
//! its phases, with nothing committed.
//!
//! `cancel_loading` flips the token of the active load, and only a project open
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
        &TEST_IMPORT,
        Path::new("/tmp/synthetic.dat"),
        None,
        VirtualShotMode::Auto,
        |_| {
            if !during_convert {
                // The user presses the overlay's Cancel while the file is read.
                state.cancel_loading();
            }
            Ok(())
        },
        |()| {
            if during_convert {
                state.cancel_loading();
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
    // and the next import is not born cancelled.
    state.cancel_loading();
    let again = run_file_import(
        &state,
        Arc::new(SilentSink),
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
