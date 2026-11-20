//! The wire types generate today's TypeScript.
//!
//! `src/types/ipc/` is generated from the legacy types until the core switch
//! points `examples/gen_ipc_types.rs` at the wire types. Each test renders a
//! wire type's file exactly as the generator writes it (ts-rs's
//! `export_to_string`, then the generator's import flattening) and compares
//! it with the committed file, less the declared changes (plan 9.2). The
//! comparison holds on both sides of the switch: before it the committed
//! file still has the dropped field (removed here before comparing), after
//! it the generator wrote the file from the wire type itself.

use std::path::PathBuf;

use ts_rs::TS;

use super::{LineWire, ProjectStateWire, ProjectWire};

/// The committed generated file `name` (`src/types/ipc/<name>.ts`).
fn committed(name: &str) -> String {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../src/types/ipc")
        .join(format!("{name}.ts"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// `T`'s file as `pnpm gen:ipc-types` writes it: every import specifier
/// rewritten to the flat sibling form `./X` (`flatten_import_paths`).
fn generated<T: TS + 'static>() -> String {
    T::export_to_string()
        .unwrap()
        .split('\n')
        .map(|line| {
            let from = " from \"";
            match (line.starts_with("import type "), line.find(from)) {
                (true, Some(open)) => {
                    let start = open + from.len();
                    let spec = &line[start..line.len() - 2];
                    let stem = spec.rsplit('/').next().unwrap_or(spec);
                    format!("{}./{stem}\";", &line[..start])
                }
                _ => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `committed` without the fields the wire drops on purpose.
fn without(mut committed: String, dropped: &[&str]) -> String {
    for field in dropped {
        committed = committed.replacen(field, "", 1);
    }
    committed
}

#[test]
fn the_wire_project_is_todays_project_less_line_offsets() {
    let expected = without(
        committed("Project"),
        &["line_offsets: Record<number, [number, number]>, "],
    );
    assert_eq!(generated::<ProjectWire>(), expected);
    assert!(
        !generated::<ProjectWire>().contains("line_offsets"),
        "plan 9.2 drops line_offsets from the wire"
    );
}

#[test]
fn the_wire_line_is_todays_line() {
    assert_eq!(generated::<LineWire>(), committed("Line"));
}

#[test]
fn the_wire_project_state_is_todays_project_state() {
    assert_eq!(generated::<ProjectStateWire>(), committed("ProjectState"));
}

/// The wire types must not write anything before the switch: none carries
/// `#[ts(export)]` (which would add an `export_bindings_*` test writing into
/// `src-tauri/bindings/`), and their export paths are the legacy types' so
/// the generator writes them over the same files once switched.
#[test]
fn the_wire_types_export_to_the_legacy_file_names() {
    assert_eq!(ProjectWire::EXPORT_TO, Some("bindings/Project.ts"));
    assert_eq!(LineWire::EXPORT_TO, Some("bindings/Line.ts"));
    assert_eq!(
        ProjectStateWire::EXPORT_TO,
        Some("bindings/ProjectState.ts")
    );
}
