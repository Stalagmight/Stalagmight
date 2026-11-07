//! [#576 NF-W4T-2] The mobile entry-point attribute is on `run()`, the function
//! the mobile runtime calls - not on whatever item happens to follow it. It sat on
//! `EXIT_LOCK_RELEASE_TIMEOUT`, so a mobile build would have applied it to a
//! constant. A source check: the desktop build never expands the attribute.

#[test]
fn the_mobile_entry_point_attribute_is_on_run() {
    let source = include_str!("lib.rs");
    let attribute = "#[cfg_attr(mobile, tauri::mobile_entry_point)]";
    let at = source.find(attribute).expect("the attribute is present");
    let next_item = source[at + attribute.len()..]
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .unwrap_or_default();
    println!("the item after the mobile entry-point attribute: {next_item}");
    assert!(
        next_item.starts_with("pub fn run()"),
        "the attribute must sit directly on run(), found: {next_item}"
    );
}
