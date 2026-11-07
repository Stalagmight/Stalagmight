//! [#576 NF-W6A-4] The image export and its preview render the project with the
//! closure-adjusted stations of that same settled version.
//!
//! Both used to read the project (`settled_snapshot`, and the preview a plain
//! `project_snapshot`) and then, in a separate span, the adjusted solve
//! (`get_adjusted_stations`): a command landing in between paired the project of
//! one version with the stations of another. The pairing itself is tested
//! behaviourally in `core::state::settled_lookup_tests`; a command-level race
//! cannot be forced without a Wry runtime, so the commands are pinned to it here
//! by their source, the idiom `load_path_tests` uses.

#[test]
fn the_export_and_its_preview_take_project_and_stations_from_one_settled_version() {
    let source = include_str!("export.rs");
    for command in [
        "pub async fn export_image<",
        "pub async fn preview_svg_export(",
    ] {
        let body = source
            .split_once(command)
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.split("\n}\n").next())
            .unwrap_or_default();
        let settled = body.find(".settled_project()");
        let paired = body.find("state.adjusted_stations_of(&settled)");
        let second_read = body.contains("get_adjusted_stations()")
            || body.contains("settled_snapshot()")
            || body.contains("project_snapshot()");
        println!(
            "{command}: settled_project at {settled:?}, adjusted_stations_of at {paired:?}, \
             a separate project/adjusted read: {second_read}"
        );
        assert!(
            settled.is_some() && settled < paired,
            "{command} must take one settled project and its adjusted stations"
        );
        assert!(
            !second_read,
            "{command} must not read either one separately"
        );
    }
}
