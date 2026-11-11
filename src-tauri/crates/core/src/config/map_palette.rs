//! The `[map_palette]` config table: which colour palette the map and the image
//! export use for depth and for distance colouring, and the palettes the user
//! pinned to the top of the picker.
//!
//! Global on purpose. The palette follows the user across projects, so it lives
//! here in `config.toml` and never in a survey's `.stalx` (whose
//! `display_settings` table is per-project) or anything sent to SpeleoDB. The
//! Levels range that goes with it (min / gamma / max) is session-only frontend
//! state and is not persisted anywhere.
//!
//! Palette ids are plain strings rather than an enum: the catalogue lives in
//! the frontend (`src/utils/palettes`), and an id written by a newer build must
//! still load here. The frontend resolves an id it does not know to the theme
//! palette, without rewriting it.

use serde::{Deserialize, Deserializer, Serialize};
use ts_rs::TS;

/// The palette id that means "the active theme's own ramp" - the default.
pub const THEME_PALETTE_ID: &str = "theme";
/// Longest accepted palette id.
pub const MAX_PALETTE_ID_LEN: usize = 64;
/// Most palettes that can be pinned.
pub const MAX_PINNED_PALETTES: usize = 64;

/// One palette selection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PaletteChoiceConfig {
    /// Palette id from the frontend catalogue, e.g. `"theme"` or `"mpl.viridis"`.
    #[serde(default = "default_palette_id")]
    pub id: String,
    /// Run the palette end to start.
    #[serde(default)]
    pub reversed: bool,
}

impl Default for PaletteChoiceConfig {
    fn default() -> Self {
        Self {
            id: default_palette_id(),
            reversed: false,
        }
    }
}

fn default_palette_id() -> String {
    THEME_PALETTE_ID.to_string()
}

/// The `[map_palette]` table.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MapPaletteConfig {
    /// Palette for colouring by depth.
    #[serde(default, deserialize_with = "lenient_choice")]
    pub depth: PaletteChoiceConfig,
    /// Palette for colouring by along-cave (penetration) distance.
    #[serde(default, deserialize_with = "lenient_choice")]
    pub distance: PaletteChoiceConfig,
    /// Palette ids pinned to the top of the picker, in pin order.
    #[serde(default, deserialize_with = "lenient_pinned")]
    pub pinned: Vec<String>,
}

/// A malformed entry must not fail the whole config parse:
/// `Config::load().unwrap_or_default()` would then reset every other setting
/// because one palette field was hand-edited badly (the same reason
/// `lenient_column_widths` exists). Anything unusable falls back to the default.
fn lenient_choice<'de, D>(deserializer: D) -> Result<PaletteChoiceConfig, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(obj) = value.as_object() else {
        return Ok(PaletteChoiceConfig::default());
    };
    Ok(PaletteChoiceConfig {
        id: obj
            .get("id")
            .and_then(serde_json::Value::as_str)
            .map_or_else(default_palette_id, str::to_string),
        reversed: obj
            .get("reversed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    })
}

fn lenient_pinned<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let Some(items) = value.as_array() else {
        return Ok(Vec::new());
    };
    Ok(items
        .iter()
        .filter_map(|v| v.as_str().map(str::to_string))
        .collect())
}

/// A palette id the config accepts: 1-64 chars of `[a-z0-9_.-]`.
pub fn is_valid_palette_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_PALETTE_ID_LEN
        && id.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'.' | b'-')
        })
}

impl PaletteChoiceConfig {
    fn normalize(&mut self) {
        if !is_valid_palette_id(&self.id) {
            self.id = default_palette_id();
        }
    }
}

impl MapPaletteConfig {
    /// Invalid ids fall back to the theme palette; pins lose invalid ids and
    /// duplicates (first pin wins) and are capped at [`MAX_PINNED_PALETTES`].
    pub fn normalize(&mut self) {
        self.depth.normalize();
        self.distance.normalize();
        let mut seen = std::collections::HashSet::new();
        self.pinned
            .retain(|id| is_valid_palette_id(id) && seen.insert(id.clone()));
        self.pinned.truncate(MAX_PINNED_PALETTES);
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn normalize_resets_bad_ids_and_dedupes_pins() {
        let mut cfg = MapPaletteConfig {
            depth: PaletteChoiceConfig {
                id: "Not Valid!".into(),
                reversed: true,
            },
            distance: PaletteChoiceConfig {
                id: "mpl.viridis".into(),
                reversed: false,
            },
            pinned: vec![
                "mpl.magma".into(),
                String::new(),
                "mpl.magma".into(),
                "x".repeat(65),
                "grad.cenote".into(),
            ],
        };
        cfg.normalize();
        assert_eq!(cfg.depth.id, THEME_PALETTE_ID);
        assert!(cfg.depth.reversed, "reversed survives an id reset");
        assert_eq!(cfg.distance.id, "mpl.viridis");
        assert_eq!(cfg.pinned, vec!["mpl.magma", "grad.cenote"]);
    }

    #[test]
    fn normalize_caps_pins() {
        let mut cfg = MapPaletteConfig {
            pinned: (0..100).map(|i| format!("p{i}")).collect(),
            ..MapPaletteConfig::default()
        };
        cfg.normalize();
        assert_eq!(cfg.pinned.len(), MAX_PINNED_PALETTES);
        assert_eq!(cfg.pinned[0], "p0");
    }

    #[test]
    fn malformed_toml_falls_back_instead_of_failing() {
        let cfg: MapPaletteConfig = toml::from_str(
            "depth = \"viridis\"\npinned = \"mpl.magma\"\n[distance]\nid = 7\nreversed = true\n",
        )
        .expect("lenient parse");
        assert_eq!(cfg.depth, PaletteChoiceConfig::default());
        assert_eq!(cfg.distance.id, THEME_PALETTE_ID);
        assert!(cfg.distance.reversed);
        assert!(cfg.pinned.is_empty());
    }

    #[test]
    fn json_round_trip() {
        let cfg = MapPaletteConfig {
            depth: PaletteChoiceConfig {
                id: "mpl.cividis".into(),
                reversed: true,
            },
            distance: PaletteChoiceConfig::default(),
            pinned: vec!["mpl.cividis".into()],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: MapPaletteConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }
}
