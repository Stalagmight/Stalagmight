//! The `[map_palette]` config table: which colour palette the map and the image
//! export use for depth and for distance colouring, the palette and options of
//! the Survey Issues colouring, and the palettes the user pinned to the top of
//! the picker.
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
/// Most entries kept in `issues_hidden` (the frontend has five errors today).
pub const MAX_HIDDEN_ISSUES: usize = 16;

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

/// How the Survey Issues colouring combines a shot's errors into one colour.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum IssueCombine {
    /// The worst of the shot's errors decides its colour.
    #[default]
    Worst,
    /// The average of the shot's errors (their palette positions) decides it.
    Average,
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
    /// Palette for the Survey Issues colouring and the Errors labels. The
    /// default id (`"theme"`) here is the original green -> yellow -> red.
    #[serde(default, deserialize_with = "lenient_choice")]
    pub issues: PaletteChoiceConfig,
    /// How a shot's errors combine into its Survey Issues colour.
    #[serde(default, deserialize_with = "lenient_combine")]
    pub issues_combine: IssueCombine,
    /// The errors left out of the Survey Issues colouring and the Errors labels
    /// (frontend ids such as `"azimuth"`). Listing what is off, not what is on,
    /// keeps an error added by a newer build on by default.
    #[serde(default, deserialize_with = "lenient_pinned")]
    pub issues_hidden: Vec<String>,
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

fn lenient_combine<'de, D>(deserializer: D) -> Result<IssueCombine, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(match value.as_str() {
        Some("average") => IssueCombine::Average,
        _ => IssueCombine::Worst,
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
        self.issues.normalize();
        let mut seen = std::collections::HashSet::new();
        self.pinned
            .retain(|id| is_valid_palette_id(id) && seen.insert(id.clone()));
        self.pinned.truncate(MAX_PINNED_PALETTES);
        // Error ids share the palette-id alphabet; an unknown one (from a newer
        // build) is kept so that build still finds it hidden.
        let mut seen_hidden = std::collections::HashSet::new();
        self.issues_hidden
            .retain(|id| is_valid_palette_id(id) && seen_hidden.insert(id.clone()));
        self.issues_hidden.truncate(MAX_HIDDEN_ISSUES);
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
            ..MapPaletteConfig::default()
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
    fn issue_options_default_and_normalize() {
        let cfg = MapPaletteConfig::default();
        assert_eq!(cfg.issues, PaletteChoiceConfig::default());
        assert_eq!(cfg.issues_combine, IssueCombine::Worst);
        assert!(cfg.issues_hidden.is_empty());

        let mut cfg = MapPaletteConfig {
            issues: PaletteChoiceConfig {
                id: "Bad Id".into(),
                reversed: true,
            },
            issues_hidden: vec![
                "length".into(),
                "length".into(),
                "Not ok".into(),
                "future_error".into(),
            ],
            ..MapPaletteConfig::default()
        };
        cfg.normalize();
        assert_eq!(cfg.issues.id, THEME_PALETTE_ID);
        assert!(cfg.issues.reversed);
        assert_eq!(cfg.issues_hidden, vec!["length", "future_error"]);
    }

    #[test]
    fn issues_hidden_is_capped() {
        let mut cfg = MapPaletteConfig {
            issues_hidden: (0..40).map(|i| format!("error{i}")).collect(),
            ..MapPaletteConfig::default()
        };
        cfg.normalize();
        assert_eq!(cfg.issues_hidden.len(), MAX_HIDDEN_ISSUES);
        assert_eq!(cfg.issues_hidden[0], "error0");
    }

    #[test]
    fn issue_options_parse_leniently() {
        let cfg: MapPaletteConfig =
            toml::from_str("issues_combine = \"average\"\nissues_hidden = [\"closure\"]\n")
                .expect("parse");
        assert_eq!(cfg.issues_combine, IssueCombine::Average);
        assert_eq!(cfg.issues_hidden, vec!["closure"]);
        let bad: MapPaletteConfig =
            toml::from_str("issues_combine = 3\nissues_hidden = \"x\"\n").expect("lenient");
        assert_eq!(bad.issues_combine, IssueCombine::Worst);
        assert!(bad.issues_hidden.is_empty());
        // An old config without the keys loads with the defaults.
        let old: MapPaletteConfig = toml::from_str("[depth]\nid = \"mpl.magma\"\n").expect("old");
        assert_eq!(old.issues, PaletteChoiceConfig::default());
        assert_eq!(old.issues_combine, IssueCombine::Worst);
    }

    #[test]
    fn json_round_trip() {
        let cfg = MapPaletteConfig {
            depth: PaletteChoiceConfig {
                id: "mpl.cividis".into(),
                reversed: true,
            },
            distance: PaletteChoiceConfig::default(),
            issues: PaletteChoiceConfig {
                id: "mpl.plasma".into(),
                reversed: false,
            },
            issues_combine: IssueCombine::Average,
            issues_hidden: vec!["length".into()],
            pinned: vec!["mpl.cividis".into()],
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: MapPaletteConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cfg);
    }
}
