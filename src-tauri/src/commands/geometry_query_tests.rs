//! [#576 C7] The geometry IPC commands reject a failure instead of resolving to
//! an empty answer.
//!
//! `get_adjusted_stations`, `get_station_distances_from` and
//! `get_along_line_distance` run their query on a blocking task. A panic there
//! (or an unreadable project) used to become `[]` / `null` through
//! `unwrap_or_default` / `.ok().flatten()`, which the frontend reads as a
//! successful empty answer. `settle_blocking` is the one place the task's outcome
//! becomes the command's, so it is what these pin.

use super::project::settle_blocking;
use crate::core::CommandErrorCode;

#[test]
fn a_crashed_task_rejects_instead_of_answering_empty() {
    // The task panicked: `spawn_blocking` hands back a JoinError, which used to be
    // swallowed into an empty vec. [#576 C4] A crashed task is INTERNAL_ERROR, as
    // on the shared pool.
    let crashed: Result<Result<Vec<u32>, String>, &str> = Err("task 7 panicked");
    let settled = settle_blocking("the closure-adjusted solve", crashed);
    println!("panicked task -> {settled:?}");
    let error = settled.expect_err("a crashed task rejects");
    assert_eq!(error.code, CommandErrorCode::InternalError);
    assert_eq!(
        error.message,
        "Could not compute the closure-adjusted solve: the computation failed (task 7 panicked)"
    );
}

#[test]
fn an_unreadable_project_rejects_and_an_absent_one_answers_empty() {
    let unreadable: Result<Result<Option<u32>, String>, &str> =
        Ok(Err("the open project could not be read".to_string()));
    let settled = settle_blocking("the distance field", unreadable);
    println!("unreadable project -> {settled:?}");
    let error = settled.expect_err("an unreadable project rejects");
    assert_eq!(error.code, CommandErrorCode::Failed);
    assert_eq!(
        error.message,
        "Could not compute the distance field: the open project could not be read"
    );

    // No project open is still an honest empty answer, and an answer passes through.
    let absent: Result<Result<Option<u32>, String>, &str> = Ok(Ok(None));
    assert_eq!(
        settle_blocking("the distance field", absent).expect("an answer"),
        None
    );
    let answered: Result<Result<Option<u32>, String>, &str> = Ok(Ok(Some(3)));
    assert_eq!(
        settle_blocking("the distance field", answered).expect("an answer"),
        Some(3)
    );
}
