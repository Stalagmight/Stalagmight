//! [#576 W75-NF7] A save made for the tree an in-place SpeleoDB swap replaced is
//! refused with `PROJECT_CHANGED`, nothing written.
//!
//! The swap (History > View, Update, Start editing's fast-forward, Take remote, a
//! re-open from SpeleoDB) replaces the open project's cache file with another tree
//! of the SAME project - same path, same `created_at` - and opens it again, so the
//! project identity a save names cannot tell the two apart. A dive-plan save that
//! reached `command_lock` after the reopen anchored the old tree's station refs
//! against the new tree; a project save saved the new tree for a request made
//! before it. The save now also names the tree generation its tree came with
//! (`treeGeneration`, from the load and refresh envelopes), and the replace bumps
//! it.
//!
//! The swap is driven here as the verbs drive it: the cache file is replaced
//! (`replace_cache_stalx_abandoning`, the replace body every pull, fast-forward and
//! revision swap runs) and opened again (`do_load_project_full`). The identities are
//! built from the wire form, as the IPC receives them. All data is synthetic, in a
//! temp dir.

use std::path::{Path, PathBuf};

use tempfile::TempDir;

use super::speleodb_cache::InheritedUndo;
use super::{for_project, AppState, ProjectIdentity};
use crate::core::{Command, CommandErrorCode};
use crate::models::{DivePlanSnapshot, Project, SectionData, ShotData};
use crate::persistence::Database;
use crate::speleodb::dump::Dump;
use crate::test_fixtures::state_with_null_sink;

/// The dump entry the swap extracts.
const ENTRY: &str = "misc/project.stalx";

/// A fresh state's tree generation; opening a project does not move it.
const BEFORE_THE_SWAP: u64 = 0;
/// The generation after one swap of the open file.
const AFTER_ONE_SWAP: u64 = 1;

/// One tree of `base`: a single section of `shots` shots, named `section`. Every
/// tree of one project shares its `created_at`, as a server revision does.
fn tree_of(base: &Project, section: &str, shots: usize) -> Project {
    let mut project = base.clone();
    project.sections.clear();
    project.add_section(SectionData {
        name: Some(section.to_string()),
        shots: (0..shots)
            .map(|i| ShotData {
                length: 3.0 + i as f64,
                ..Default::default()
            })
            .collect(),
        ..Default::default()
    });
    project
}

fn write(path: &Path, project: &Project) {
    let mut db = Database::open(path).unwrap();
    db.save_project(project).unwrap();
}

/// A one-entry server dump carrying `project`.
fn dump_of(project: &Project) -> Vec<u8> {
    use std::io::Write as _;
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("server.stalx");
    write(&path, project);
    let bytes = std::fs::read(path).unwrap();
    let mut writer = zip::write::ZipWriter::new(std::io::Cursor::new(Vec::<u8>::new()));
    let opts =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer.start_file(ENTRY, opts).unwrap();
    writer.write_all(&bytes).unwrap();
    writer.finish().unwrap().into_inner()
}

/// The open project as the frontend names it on a save, with the tree generation
/// its tree came with (`None`: it names none). The wire form, as the IPC gets it.
fn made_for(state: &AppState, tree_generation: Option<u64>) -> ProjectIdentity {
    let open = state.project_snapshot().unwrap();
    let mut wire = serde_json::json!({
        "createdAt": open.created_at.as_str(),
        "filePath": open.file_path.as_ref().map(|f| f.to_string_lossy().into_owned()),
    });
    if let Some(generation) = tree_generation {
        wire["treeGeneration"] = serde_json::json!(generation);
    }
    serde_json::from_value(wire).unwrap()
}

/// The open project's file, `old`'s tree, opened as a load opens it.
struct Fixture {
    _temp: TempDir,
    dest: PathBuf,
    base: Project,
    state: AppState,
}

fn open_old_tree() -> Fixture {
    let temp = TempDir::new().unwrap();
    let dest = temp.path().join("project.stalx");
    let base = Project::new("Cave".to_string(), None);
    write(&dest, &tree_of(&base, "Old tree", 3));
    let state = state_with_null_sink();
    state.do_load_project_full(dest.clone()).unwrap();
    Fixture {
        _temp: temp,
        dest,
        base,
        state,
    }
}

/// Swap the open file in place, as the fast-forward and the revision swap do:
/// replace the cache file, then close (a no-op, the replace closed it) and reopen.
fn swap_in_place(fx: &Fixture, new_tree: &Project) {
    let zip = dump_of(new_tree);
    let mut dump = Dump::open(&zip).unwrap();
    fx.state
        .replace_cache_stalx_abandoning(&mut dump, ENTRY, &fx.dest, InheritedUndo::Drop)
        .unwrap();
    fx.state.close_project();
    fx.state.do_load_project_full(fx.dest.clone()).unwrap();
}

fn section_names(state: &AppState) -> Vec<String> {
    state
        .project_snapshot()
        .unwrap()
        .sections
        .iter()
        .map(|s| s.name.clone().unwrap_or_default())
        .collect()
}

fn empty_plan() -> DivePlanSnapshot {
    serde_json::from_str("{}").unwrap()
}

/// Swap in the new tree, checking it really is the same project at the same path.
fn swapped(fx: &Fixture) -> ProjectIdentity {
    let before = made_for(&fx.state, Some(BEFORE_THE_SWAP));
    swap_in_place(fx, &tree_of(&fx.base, "New tree", 1));
    assert_eq!(
        section_names(&fx.state),
        ["New tree"],
        "the swap opened the new tree"
    );
    let after = made_for(&fx.state, None);
    assert_eq!(
        (&after.created_at, &after.file_path),
        (&before.created_at, &before.file_path),
        "the swapped file is the same project at the same path"
    );
    before
}

#[test]
fn a_dive_plan_save_made_for_the_tree_a_swap_replaced_is_refused() {
    let fx = open_old_tree();
    let before = swapped(&fx);

    let answer = for_project(Some(before), || {
        fx.state
            .save_dive_plan_for(None, "Made for the old tree".to_string(), empty_plan())
    });
    println!("dive-plan save for the replaced tree: {answer:?}");

    assert_eq!(
        answer.err().map(|e| e.code),
        Some(CommandErrorCode::ProjectChanged),
        "a plan made for the replaced tree must not be anchored against the new one"
    );
    assert!(
        fx.state.list_dive_plans().unwrap().is_empty(),
        "nothing was written to the .plnx"
    );
}

#[test]
fn a_project_save_made_for_the_tree_a_swap_replaced_is_refused() {
    let fx = open_old_tree();
    let before = swapped(&fx);
    let bytes_before = std::fs::read(&fx.dest).unwrap();
    let elsewhere = fx.dest.with_file_name("Save As target.stalx");

    let save = for_project(Some(before.clone()), || {
        fx.state.execute(Command::SaveProject { path: None })
    });
    let save_as = for_project(Some(before), || {
        fx.state.execute(Command::SaveProject {
            path: Some(elsewhere.clone()),
        })
    });
    println!("save: {:?}, save as: {:?}", save.code, save_as.code);

    assert_eq!(save.code, Some(CommandErrorCode::ProjectChanged), "save");
    assert_eq!(
        save_as.code,
        Some(CommandErrorCode::ProjectChanged),
        "Save As"
    );
    assert_eq!(
        std::fs::read(&fx.dest).unwrap(),
        bytes_before,
        "the open file is untouched"
    );
    assert!(!elsewhere.exists(), "no Save As copy was written");
}

#[test]
fn a_save_made_for_the_swapped_in_tree_lands() {
    let fx = open_old_tree();
    swapped(&fx);

    let plan = for_project(Some(made_for(&fx.state, Some(AFTER_ONE_SWAP))), || {
        fx.state
            .save_dive_plan_for(None, "Made for the new tree".to_string(), empty_plan())
    });
    let save = for_project(Some(made_for(&fx.state, Some(AFTER_ONE_SWAP))), || {
        fx.state.execute(Command::SaveProject { path: None })
    });
    println!(
        "plan: {:?}, save: {:?}",
        plan.as_ref().map(|m| m.id),
        save.code
    );

    assert!(plan.is_ok(), "the new tree's own plan is saved: {plan:?}");
    assert!(
        save.success,
        "the new tree's own save lands: {:?}",
        save.error
    );
}

#[test]
fn a_save_naming_no_tree_generation_is_not_checked() {
    let fx = open_old_tree();
    swapped(&fx);

    let save = for_project(Some(made_for(&fx.state, None)), || {
        fx.state.execute(Command::SaveProject { path: None })
    });
    println!("save naming no generation: {:?}", save.code);

    assert!(
        save.success,
        "an older frontend's save is not refused: {:?}",
        save.error
    );
}

#[test]
fn a_failed_replace_keeps_the_generation_of_the_tree_still_shown() {
    let fx = open_old_tree();
    let before = made_for(&fx.state, Some(BEFORE_THE_SWAP));
    let zip = dump_of(&tree_of(&fx.base, "New tree", 1));
    let mut dump = Dump::open(&zip).unwrap();

    // An entry the dump does not have: the extract fails after the close, and the
    // replace opens the same bytes again.
    let failed = fx.state.replace_cache_stalx_abandoning(
        &mut dump,
        "misc/missing.stalx",
        &fx.dest,
        InheritedUndo::Drop,
    );
    assert!(failed.is_err(), "the extract fails");
    assert_eq!(
        section_names(&fx.state),
        ["Old tree"],
        "the old tree is open again"
    );

    let save = for_project(Some(before), || {
        fx.state.execute(Command::SaveProject { path: None })
    });
    println!("save after a failed replace: {:?}", save.code);

    assert!(
        save.success,
        "the tree still shown is the one the save was made for: {:?}",
        save.error
    );
}

#[test]
fn replacing_another_cache_file_keeps_the_generation() {
    let fx = open_old_tree();
    let before = made_for(&fx.state, Some(BEFORE_THE_SWAP));
    let other = fx.dest.with_file_name("other.stalx");
    write(
        &other,
        &tree_of(&Project::new("Other".to_string(), None), "Other", 1),
    );
    let zip = dump_of(&tree_of(&fx.base, "New tree", 1));
    let mut dump = Dump::open(&zip).unwrap();

    fx.state
        .replace_cache_stalx_abandoning(&mut dump, ENTRY, &other, InheritedUndo::Drop)
        .unwrap();

    let save = for_project(Some(before), || {
        fx.state.execute(Command::SaveProject { path: None })
    });
    println!("save after another file's replace: {:?}", save.code);

    assert!(
        save.success,
        "the open project's tree did not change: {:?}",
        save.error
    );
}
