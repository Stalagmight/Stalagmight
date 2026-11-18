//! The `config.toml` names of the settings the Section -> Line rename touched.
//!
//! A Line was called a Section until that rename, and the config file keeps the
//! old names: an older build requires `section_colors`, so a file without it
//! fails that build's whole parse, and `Config::load().unwrap_or_default()` then
//! resets every setting. IPC and the frontend use the new names. Reading accepts
//! both (a `#[serde(alias)]` on each renamed field) and [`to_disk`] writes the
//! old ones. Two names the frontend owns are not serde fields, so no alias can
//! reach them, and they are translated in both directions here: the grid column
//! key the `[panels]` layouts store, and the highlighting role the
//! `[syntactic_highlighting.overrides.<preset>]` tables are keyed by.

use toml::Value;

/// Renamed keys: (table, name in code, name on disk); `""` is the top level.
const KEYS: &[(&str, &str, &str)] = &[
    ("", "line_colors", "section_colors"),
    ("shortcuts", "toggle_line_labels", "toggle_section_labels"),
    (
        "panels",
        "new_line_column_order",
        "new_section_column_order",
    ),
    (
        "panels",
        "new_line_hidden_columns",
        "new_section_hidden_columns",
    ),
    (
        "panels",
        "new_line_column_widths",
        "new_section_column_widths",
    ),
    (
        "panels",
        "edit_line_column_order",
        "edit_section_column_order",
    ),
    (
        "panels",
        "edit_line_hidden_columns",
        "edit_section_hidden_columns",
    ),
    (
        "panels",
        "edit_line_column_widths",
        "edit_section_column_widths",
    ),
];

/// Grid column keys a `[panels]` layout stores: (name in code, name on disk).
const COLUMN_KEYS: &[(&str, &str)] = &[("line_id", "section_id")];

/// Highlighting roles an override table is keyed by: (name in code, name on disk).
const ROLES: &[(&str, &str)] = &[("line", "section")];

/// The `[panels]` fields that hold grid column keys: key lists, and width maps
/// keyed by column key.
const COLUMN_LAYOUT_SUFFIXES: &[&str] = &[
    "_column_order",
    "_hidden_columns",
    "_columns_seen",
    "_column_widths",
];

/// Rename a config document this build serialized into the file's vocabulary.
pub(super) fn to_disk(document: &mut Value) {
    let Some(root) = document.as_table_mut() else {
        return;
    };
    for &(table, code, disk) in KEYS {
        let target = if table.is_empty() {
            Some(&mut *root)
        } else {
            root.get_mut(table).and_then(Value::as_table_mut)
        };
        let Some(target) = target else {
            continue;
        };
        if let Some(value) = target.remove(code) {
            target.insert(disk.to_string(), value);
        }
    }
    for &(code, disk) in COLUMN_KEYS {
        rename_column_key(root, code, disk);
    }
    for &(code, disk) in ROLES {
        rename_role(root, code, disk);
    }
}

/// Rename the column keys of a config document read from disk into this
/// build's vocabulary. The renamed table keys need nothing here: each renamed
/// field carries its old name as a serde alias.
pub(super) fn from_disk(document: &mut Value) {
    let Some(root) = document.as_table_mut() else {
        return;
    };
    for &(code, disk) in COLUMN_KEYS {
        rename_column_key(root, disk, code);
    }
    for &(code, disk) in ROLES {
        rename_role(root, disk, code);
    }
}

/// Rename the grid column key `from` to `to` in every `[panels]` layout.
fn rename_column_key(root: &mut toml::map::Map<String, Value>, from: &str, to: &str) {
    let Some(panels) = root.get_mut("panels").and_then(Value::as_table_mut) else {
        return;
    };
    for (field, layout) in panels.iter_mut() {
        if !COLUMN_LAYOUT_SUFFIXES
            .iter()
            .any(|suffix| field.ends_with(suffix))
        {
            continue;
        }
        match layout {
            Value::Array(keys) => {
                for key in keys {
                    if key.as_str() == Some(from) {
                        *key = Value::String(to.to_string());
                    }
                }
            }
            Value::Table(widths) => {
                if let Some(width) = widths.remove(from) {
                    widths.insert(to.to_string(), width);
                }
            }
            _ => {}
        }
    }
}

/// Rename the highlighting role `from` to `to` in every preset's override table.
fn rename_role(root: &mut toml::map::Map<String, Value>, from: &str, to: &str) {
    let Some(presets) = root
        .get_mut("syntactic_highlighting")
        .and_then(|section| section.get_mut("overrides"))
        .and_then(Value::as_table_mut)
    else {
        return;
    };
    for roles in presets
        .iter_mut()
        .filter_map(|(_, roles)| roles.as_table_mut())
    {
        if let Some(color) = roles.remove(from) {
            roles.insert(to.to_string(), color);
        }
    }
}
