//! [#576 RV4-R-NITS S-8] Take remote and Discard changes replace the survey, so the
//! read-only Plan and Export workspaces refuse them like Restore revision and
//! Unwind (RV2-R-15), with the same coded notice and before any network call.
//!
//! Driven against the loopback recording stub; all data is synthetic.

use super::speleodb::{editing_fixture, recording_stub, HELD_BY_SAME_ACCOUNT_AT_BASE_BODY};
use super::WorkspaceMode;
use crate::models::CommandErrorCode;

#[test]
fn take_remote_and_discard_are_refused_in_plan_and_export() {
    let mut refused_everywhere = true;
    for mode in [WorkspaceMode::Plan, WorkspaceMode::Export] {
        for verb in ["Take remote", "Discard changes"] {
            let (base, _listener, seen) = recording_stub(HELD_BY_SAME_ACCOUNT_AT_BASE_BODY);
            let (_temp, state) = editing_fixture(&base);
            state.set_workspace_mode(mode);

            let refusal = match verb {
                "Take remote" => state.speleodb_take_remote().err(),
                _ => state.speleodb_discard_changes(true, true).err(),
            };

            let requests = seen.lock().unwrap().len();
            // [#576 C4] The workspace gate's own code, and no marker in the text.
            let coded = refusal.as_ref().is_some_and(|e| {
                e.code == CommandErrorCode::WorkspaceReadOnly
                    && !e.message.contains("WORKSPACE_READ_ONLY")
            });
            println!("{verb} in {mode:?} -> refusal {refusal:?}; requests sent: {requests}");
            refused_everywhere &= coded && requests == 0 && state.edit_enabled();
        }
    }
    assert!(
        refused_everywhere,
        "each verb is refused with the workspace notice, before any request, with the \
         edit session untouched (see the lines above)"
    );
}
