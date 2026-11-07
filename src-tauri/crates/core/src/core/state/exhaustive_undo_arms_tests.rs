//! [#576 RV4-R-U3] The last two catch-alls on the undo/redo path name their
//! variants.
//!
//! A7 made the undo/redo dispatchers exhaustive so a new `Command` or `UndoData`
//! variant is a compile error where it must be decided, not a silent runtime
//! default. Two wildcards survived: `execute_without_undo` (redo's
//! re-execution, `InverseCommand` undo and every Batch sub-command), whose
//! `_ =>` failed a new variant only at runtime - rolling back a whole batch -
//! and `UndoData::shot_target` (the load-time filter for entries whose shot no
//! longer resolves), whose `_ => None` would let a new shot-addressing variant
//! skip that filter. Both now list their variants.
//!
//! Structural, like the other source tripwires in this crate: the property is
//! that adding a variant fails to COMPILE, which no runtime test can observe.

/// The body of `fn <name>` in `source`: from its signature to the next item at
/// the same indentation.
fn body<'a>(source: &'a str, signature: &str) -> &'a str {
    let start = source
        .find(signature)
        .unwrap_or_else(|| panic!("{signature} not found"));
    let rest = source.get(start..).unwrap_or_default();
    let end = rest.find("\n    }\n").map_or(rest.len(), |at| at + 6);
    rest.get(..end).unwrap_or(rest)
}

fn wildcard_arms(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| line.starts_with("_ =>"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn the_undo_path_dispatchers_have_no_wildcard_arm() {
    let restore = include_str!("undo_restore.rs");
    let undo = include_str!("../undo.rs");
    let execute_without_undo = wildcard_arms(body(
        restore,
        "pub(super) fn execute_without_undo(&self, cmd: Command) -> CommandResult {",
    ));
    let shot_target = wildcard_arms(body(
        undo,
        "pub fn shot_target(&self) -> Option<(SectionId, Option<&str>, ShotId)> {",
    ));
    println!("execute_without_undo wildcard arms: {execute_without_undo:?}");
    println!("UndoData::shot_target wildcard arms: {shot_target:?}");
    assert!(
        execute_without_undo.is_empty(),
        "a new Command variant must be decided in execute_without_undo at compile time"
    );
    assert!(
        shot_target.is_empty(),
        "a new UndoData variant must be decided in shot_target at compile time"
    );
}
