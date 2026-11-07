//! Persisted data-table column widths: the bounds, the tolerant deserializer and
//! the normalizer.
//!
//! Split out of `config/types.rs` rather than rebaselined. Adding these pushed that
//! file from 1430 to 1505 production LOC, i.e. across the ratchet's 1500 `WARN_LOC`
//! line, at which point a file is frozen and may shrink but not grow. #157 says to
//! split at an API boundary instead of raising the baseline, and this is a clean one:
//! nothing here needs the rest of `types.rs`, and `PanelsConfig` needs only the two
//! entry points.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Deserializer};

/// Smallest column width the grid can render legibly, and the widest it will
/// persist. Mirrors `MIN_COL_WIDTH` and `MAX_COL_WIDTH`, BOTH of which live in
/// src/components/common/SpreadsheetGrid/useColumnWidths.ts (SpreadsheetGrid.tsx
/// only imports them). The mirror is enforced by
/// src/test/__tests__/configBoundsMirror.test.ts, which parses this file and
/// compares against the TS constants - change all four together.
const MIN_COLUMN_WIDTH: u32 = 40;
const MAX_COLUMN_WIDTH: u32 = 2000;

/// Ceilings on the SHAPE of a column-width map, as opposed to the values in it.
/// Clamping each width bounds one entry; without these, the map itself is
/// unbounded in both entry count and key length. That matters because nothing
/// prunes a key that no longer names a real column: the frontend re-sends the
/// whole map on every later resize, so one bad write is re-persisted forever.
/// Both grids together declare well under a hundred columns, so 256 is generous
/// for any real layout. Same reasoning as `MAX_BINDINGS_PER_ACTION` in `types.rs`.
///
/// Note what this does and does not bound: it caps what we RETAIN and write back,
/// not what a single hostile IPC payload or config file transiently allocates -
/// serde has already materialised the input by the time these apply.
const MAX_COLUMN_WIDTH_ENTRIES: usize = 256;
const MAX_COLUMN_KEY_LEN: usize = 128;

/// Tolerant deserializer for the data-table column-width maps. Captures the raw
/// value as a `serde_json::Value` first - this works through any self-describing
/// `Deserializer`, TOML's included, since `Value`'s own `Deserialize` impl only
/// relies on `deserialize_any` - then keeps the entries that are usable pixel
/// widths and DROPS the rest.
///
/// Per-entry rather than per-field, so a hand-edited `length = -5` costs that one
/// column its width instead of taking down the whole `[panels]` table. Same
/// reasoning as `lenient_dpv` in `models/config/gas.rs`: erroring here would fail the entire
/// `toml::from_str::<Config>`, and `Config::load().unwrap_or_default()` would
/// then reset every unrelated setting in the file. Out-of-range-but-integral
/// values survive deserialization and are clamped by [`normalize_column_widths`];
/// negative, fractional and non-numeric ones are not representable as a width at
/// all and are dropped here.
pub(crate) fn lenient_column_widths<'de, D>(
    deserializer: D,
) -> Result<BTreeMap<String, u32>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(entries) = value.as_object() else {
        return Ok(BTreeMap::new());
    };
    Ok(entries
        .iter()
        .filter_map(|(key, raw)| {
            // Saturate rather than drop: a value past u32::MAX is still an integral
            // width, and the doc above promises those reach `normalize_column_widths`
            // to be clamped. `ok()?` here would silently discard the entry instead.
            let width = u32::try_from(raw.as_u64()?).unwrap_or(u32::MAX);
            Some((key.clone(), width))
        })
        .collect())
}

/// Clamp persisted column widths into the range the grid can actually render.
/// The frontend clamps while dragging, but the load path did not - so a
/// hand-edited or older config could supply a 0-pixel column (invisible, and
/// un-resizable because there is no border left to grab) or a 100000-pixel one
/// that pushes every later column off-screen.
pub fn normalize_column_widths(widths: &mut BTreeMap<String, u32>) {
    // Shape first, then values. Both the load path (`Config::normalize`) and the
    // IPC write path (`update_data_table_view`) come through here, so this is
    // the one place that has to hold for a hand-edited file and for a caller that
    // reached `invoke()` alike.
    widths.retain(|key, _| !key.is_empty() && key.len() <= MAX_COLUMN_KEY_LEN);

    if widths.len() > MAX_COLUMN_WIDTH_ENTRIES {
        tracing::warn!(
            "Column-width map has {} entries, truncating to {}",
            widths.len(),
            MAX_COLUMN_WIDTH_ENTRIES
        );
        // BTreeMap has no truncate; drop everything past the cap in key order so
        // the survivors are at least deterministic rather than hash-dependent.
        let excess: Vec<String> = widths
            .keys()
            .skip(MAX_COLUMN_WIDTH_ENTRIES)
            .cloned()
            .collect();
        for key in excess {
            widths.remove(&key);
        }
    }

    for width in widths.values_mut() {
        *width = (*width).clamp(MIN_COLUMN_WIDTH, MAX_COLUMN_WIDTH);
    }
}

/// Bound a persisted LIST of column keys - the column-order and hidden-column
/// lists - the same way [`normalize_column_widths`] bounds the width maps.
///
/// These four `Vec<String>` fields were the one part of the data-table view state
/// with no ceiling at all: `update_data_table_view` wrote them straight
/// through from the renderer, and nothing on the load path pruned them either.
/// That is the same unbounded-growth hole the width maps already close, and it
/// bites harder here because there is no per-entry value to clamp - the entries
/// ARE the payload. `config.toml` is re-parsed at every launch, so one oversized
/// write is paid for on every start until someone hand-edits the file.
///
/// The ceilings are deliberately the ones the width maps use
/// (`MAX_COLUMN_WIDTH_ENTRIES` / `MAX_COLUMN_KEY_LEN`): both sides key the same
/// grid columns, so a key too long to be a real column key is degenerate in a
/// width map and in an order list for exactly the same reason, and letting the
/// two drift apart would mean a key that survives one field and not its sibling.
///
/// Also dedups, which the width maps get for free from `BTreeMap`: a repeated key
/// in an order list is meaningless to the grid and doubles the entry count a
/// hostile caller has to send to reach the cap.
pub(crate) fn normalize_column_names(names: &mut Vec<String>) {
    // Shape only - there is no value half here. Order is preserved for the
    // survivors because these lists ARE an order: reordering them would silently
    // rearrange the user's columns.
    let mut seen: HashSet<String> = HashSet::new();
    names.retain(|name| {
        !name.is_empty() && name.len() <= MAX_COLUMN_KEY_LEN && seen.insert(name.clone())
    });

    if names.len() > MAX_COLUMN_WIDTH_ENTRIES {
        tracing::warn!(
            "Column-name list has {} entries, truncating to {}",
            names.len(),
            MAX_COLUMN_WIDTH_ENTRIES
        );
        names.truncate(MAX_COLUMN_WIDTH_ENTRIES);
    }
}
