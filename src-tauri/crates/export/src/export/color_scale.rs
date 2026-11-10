//! The user's depth / distance colour scale for raster export: a palette the
//! frontend bakes into a `#rrggbb` table, and a Photoshop-Levels range
//! (min / max / gamma) - the same model the live map uses
//! (`src/utils/palettes`). TypeScript stays the single source of palette
//! colours; this side only maps a value to a table position.
//!
//! Absent fields keep today's export exactly: no palette = the theme depth ramp
//! or the fixed distance ramp; no levels = the data's full extent, linear.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::image::{
    depth_gradient_color, depth_range, distance_gradient_color, distance_ramp_at, DepthRamp,
    DIST_UNREACHABLE,
};
use crate::models::Project;

/// Most entries a baked palette may carry (the frontend sends 256).
pub const MAX_PALETTE_ENTRIES: usize = 4096;

/// A Levels range: values at or below `min` take the palette start, at or
/// above `max` its end; `gamma` bends the middle (`t' = t^(1/gamma)`).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
pub struct ExportLevels {
    pub min: f64,
    pub max: f64,
    pub gamma: f64,
}

/// One kind's colour scale for an export. Every field is optional.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct ExportColorScale {
    /// The palette baked to `#rrggbb` at evenly spaced positions 0..1.
    /// Absent (or unusable) = the default ramp for the kind.
    #[serde(default)]
    #[ts(optional)]
    pub palette: Option<Vec<String>>,
    /// The Levels range. Absent = the data's own extent, linear.
    #[serde(default)]
    #[ts(optional)]
    pub levels: Option<ExportLevels>,
}

/// An [`ExportColorScale`] parsed once per export.
pub(crate) struct ResolvedScale {
    lut: Option<Vec<(u8, u8, u8)>>,
    levels: Option<ExportLevels>,
}

fn parse_hex(s: &str) -> Option<(u8, u8, u8)> {
    let rest = s.trim().strip_prefix('#')?;
    if rest.len() != 6 || !rest.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&rest[i..i + 2], 16).ok();
    Some((byte(0)?, byte(2)?, byte(4)?))
}

impl ResolvedScale {
    /// Parse a scale. A palette with any unusable entry, fewer than two
    /// entries or more than [`MAX_PALETTE_ENTRIES`] is dropped (default ramp)
    /// rather than drawn with holes; non-finite or inverted levels likewise.
    pub(crate) fn new(scale: &ExportColorScale) -> Self {
        let lut = scale.palette.as_ref().and_then(|entries| {
            if entries.len() < 2 || entries.len() > MAX_PALETTE_ENTRIES {
                return None;
            }
            entries
                .iter()
                .map(|e| parse_hex(e))
                .collect::<Option<Vec<_>>>()
        });
        let levels = scale.levels.filter(|l| {
            l.min.is_finite()
                && l.max.is_finite()
                && l.gamma.is_finite()
                && l.gamma > 0.0
                && l.max >= l.min
        });
        Self { lut, levels }
    }

    /// The Levels range to use for data spanning `extent`.
    pub(crate) fn range(&self, extent: (f64, f64)) -> (f64, f64, f64) {
        self.levels
            .map_or((extent.0, extent.1, 1.0), |l| (l.min, l.max, l.gamma))
    }

    /// The palette colour at position `t` (0..1), linearly interpolated
    /// between the baked entries. `None` without a palette.
    pub(crate) fn color_at(&self, t: f64) -> Option<String> {
        let lut = self.lut.as_ref()?;
        let t = if t.is_finite() {
            t.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let pos = t * (lut.len() - 1) as f64;
        let i = (pos.floor() as usize).min(lut.len() - 2);
        let f = pos - i as f64;
        let (a, b) = (lut[i], lut[i + 1]);
        let mix = |x: u8, y: u8| -> u8 {
            (f64::from(x) + (f64::from(y) - f64::from(x)) * f).round() as u8
        };
        Some(format!(
            "#{:02x}{:02x}{:02x}",
            mix(a.0, b.0),
            mix(a.1, b.1),
            mix(a.2, b.2)
        ))
    }
}

/// Palette position for `value` through a Levels range: clamp to `[min, max]`,
/// normalise, then bend by gamma. A flat range maps everything to 0.
pub(crate) fn levels_t(value: f64, (min, max, gamma): (f64, f64, f64)) -> f64 {
    if max <= min || !value.is_finite() {
        return 0.0;
    }
    let t = ((value.clamp(min, max) - min) / (max - min)).clamp(0.0, 1.0);
    if (gamma - 1.0).abs() < f64::EPSILON {
        t
    } else {
        t.powf(1.0 / gamma)
    }
}

/// What an export histogram bins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase")]
pub enum ExportValueKind {
    Depth,
    Distance,
}

/// Equal-width bin counts over `[min, max]`, for the export panel's Levels
/// slider. Built from exactly the data the export colours, so its extent is
/// the export's auto range.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ValueHistogram {
    pub min: f64,
    pub max: f64,
    pub counts: Vec<u32>,
}

/// Bin finite `values` over `(min, max)`; out-of-range values land in the end
/// bins, a flat range in the first.
pub(crate) fn bin_values(
    values: impl Iterator<Item = f64>,
    (min, max): (f64, f64),
    bins: usize,
) -> Vec<u32> {
    let bins = bins.clamp(1, 1024);
    let mut counts = vec![0u32; bins];
    let span = max - min;
    for v in values.filter(|v| v.is_finite()) {
        let k = if span > 0.0 {
            (((v - min) / span) * bins as f64).floor()
        } else {
            0.0
        };
        let k = if k < 0.0 {
            0
        } else {
            (k as usize).min(bins - 1)
        };
        counts[k] = counts[k].saturating_add(1);
    }
    counts
}

/// Depth colour through the user's colour scale when one is set, else the
/// theme ramp over the data extent exactly as before.
pub(super) fn depth_color(
    depth: f64,
    extent: (f64, f64),
    ramp: DepthRamp<'_>,
    scale: Option<&ResolvedScale>,
) -> String {
    let Some(scale) = scale else {
        return depth_gradient_color(depth, extent.0, extent.1, ramp);
    };
    if !depth.is_finite() {
        return "#000000".to_string();
    }
    let range = scale.range(extent);
    let t = levels_t(depth, range);
    if let Some(c) = scale.color_at(t) {
        return c;
    }
    if range.1 <= range.0 {
        return ramp.shallow.to_string();
    }
    crate::export::depth_colors::interpolate_hsl(ramp.shallow, ramp.deep, t)
}

/// Distance colour through the user's colour scale when one is set, else the
/// fixed ramp over `0..max` exactly as before. Unreachable stays grey.
pub(super) fn distance_color(
    value: Option<f64>,
    max: f64,
    scale: Option<&ResolvedScale>,
) -> String {
    let Some(scale) = scale else {
        return distance_gradient_color(value, max);
    };
    let Some(v) = value.filter(|v| v.is_finite() && *v >= 0.0) else {
        return DIST_UNREACHABLE.to_string();
    };
    let t = levels_t(v, scale.range((0.0, if max > 0.0 { max } else { 1.0 })));
    scale.color_at(t).unwrap_or_else(|| distance_ramp_at(t))
}

/// Bin counts of the values an export would colour for `kind`, over the
/// export's own auto range: depth over the same stations and shots the export
/// normalises across, distance from the entrance over the measured network.
pub fn export_value_histogram(
    project: &Project,
    stations: &[crate::models::Station],
    kind: ExportValueKind,
    bins: usize,
    include_reference_legs: bool,
) -> ValueHistogram {
    let ((min, max), counts) = match kind {
        ExportValueKind::Depth => {
            let extent = depth_range(project, stations);
            (
                extent,
                bin_values(stations.iter().map(|s| s.coordinates.z), extent, bins),
            )
        }
        ExportValueKind::Distance => {
            let field = project.distance_field_from_with(
                &project.compute_station_coordinates(),
                None,
                include_reference_legs,
            );
            let extent = (
                0.0,
                if field.max_reachable > 0.0 {
                    field.max_reachable
                } else {
                    1.0
                },
            );
            (
                extent,
                bin_values(field.distances.iter().filter_map(|d| *d), extent, bins),
            )
        }
    };
    ValueHistogram { min, max, counts }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::float_cmp)]
mod tests {
    use super::*;

    fn scale(palette: Option<Vec<&str>>, levels: Option<ExportLevels>) -> ResolvedScale {
        ResolvedScale::new(&ExportColorScale {
            palette: palette.map(|p| p.into_iter().map(String::from).collect()),
            levels,
        })
    }

    #[test]
    fn levels_t_is_linear_at_gamma_one_and_bends_otherwise() {
        assert_eq!(levels_t(5.0, (0.0, 10.0, 1.0)), 0.5);
        assert_eq!(levels_t(-3.0, (0.0, 10.0, 1.0)), 0.0);
        assert_eq!(levels_t(30.0, (0.0, 10.0, 1.0)), 1.0);
        assert!((levels_t(2.5, (0.0, 10.0, 2.0)) - 0.5).abs() < 1e-12);
        assert_eq!(levels_t(4.0, (4.0, 4.0, 1.0)), 0.0);
    }

    #[test]
    fn palette_ends_are_exact_and_the_middle_interpolates() {
        let s = scale(Some(vec!["#000000", "#ffffff"]), None);
        assert_eq!(s.color_at(0.0).unwrap(), "#000000");
        assert_eq!(s.color_at(1.0).unwrap(), "#ffffff");
        assert_eq!(s.color_at(0.5).unwrap(), "#808080");
        assert_eq!(s.color_at(f64::NAN).unwrap(), "#000000");
    }

    #[test]
    fn an_unusable_palette_or_range_falls_back() {
        assert!(scale(Some(vec!["#000000", "black"]), None)
            .color_at(0.0)
            .is_none());
        assert!(scale(Some(vec!["#000000"]), None).color_at(0.0).is_none());
        let inverted = scale(
            None,
            Some(ExportLevels {
                min: 5.0,
                max: 1.0,
                gamma: 1.0,
            }),
        );
        assert_eq!(inverted.range((0.0, 9.0)), (0.0, 9.0, 1.0));
        let zero_gamma = scale(
            None,
            Some(ExportLevels {
                min: 0.0,
                max: 1.0,
                gamma: 0.0,
            }),
        );
        assert_eq!(zero_gamma.range((0.0, 9.0)), (0.0, 9.0, 1.0));
    }

    fn shot(length: f64, depth_in: f64, depth_out: f64) -> crate::models::ShotData {
        crate::models::ShotData {
            azimuth_in: 0.0,
            azimuth_out: 0.0,
            length,
            depth_in,
            depth_out,
            kind: crate::models::ShotKind::Normal,
            ..Default::default()
        }
    }

    fn sloping_project() -> Project {
        let mut project = Project::new("Scale".to_string(), None);
        project.add_section(crate::models::SectionData {
            name: Some("A".to_string()),
            color: "#000000".to_string(),
            shots: vec![shot(10.0, 0.0, 10.0), shot(10.0, 10.0, 20.0)],
            ..Default::default()
        });
        project
    }

    fn render(depth_scale: Option<ExportColorScale>) -> String {
        crate::export::generate_svg_string(
            &sloping_project(),
            &crate::export::ExportImageOptions {
                format: crate::export::ImageFormat::Png,
                color_mode: crate::export::ColorMode::Depth,
                depth_scale,
                ..Default::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn an_empty_scale_renders_exactly_the_default_export() {
        assert_eq!(render(Some(ExportColorScale::default())), render(None));
    }

    #[test]
    fn a_palette_and_range_recolour_the_depth_export() {
        let palette: Vec<String> = vec!["#000000".into(), "#ffffff".into()];
        let full = render(Some(ExportColorScale {
            palette: Some(palette.clone()),
            levels: None,
        }));
        assert!(
            !full.contains("#4ecdc4"),
            "the theme ramp must not be drawn: {full}"
        );
        // A range ending at 5 m clamps everything deeper to the palette end.
        let clamped = render(Some(ExportColorScale {
            palette: Some(palette),
            levels: Some(ExportLevels {
                min: 0.0,
                max: 5.0,
                gamma: 1.0,
            }),
        }));
        let whites = clamped.matches("stroke=\"#ffffff\"").count();
        let all = clamped.matches("<polyline").count();
        assert!(
            whites * 2 > all,
            "most sub-segments sit below 5 m: {whites}/{all}"
        );
    }

    #[test]
    fn distance_keeps_unreachable_grey_under_a_palette() {
        let scale = ResolvedScale::new(&ExportColorScale {
            palette: Some(vec!["#000000".into(), "#ffffff".into()]),
            levels: Some(ExportLevels {
                min: 0.0,
                max: 10.0,
                gamma: 1.0,
            }),
        });
        assert_eq!(distance_color(None, 10.0, Some(&scale)), DIST_UNREACHABLE);
        assert_eq!(distance_color(Some(20.0), 10.0, Some(&scale)), "#ffffff");
        assert_eq!(
            distance_color(Some(5.0), 10.0, None),
            distance_gradient_color(Some(5.0), 10.0)
        );
    }

    #[test]
    fn the_depth_histogram_covers_the_export_range() {
        let project = sloping_project();
        let stations = project.compute_station_coordinates();
        let h = export_value_histogram(&project, &stations, ExportValueKind::Depth, 8, false);
        assert_eq!((h.min, h.max), depth_range(&project, &stations));
        assert_eq!(h.counts.len(), 8);
        assert_eq!(h.counts.iter().sum::<u32>() as usize, stations.len());
    }

    #[test]
    fn histogram_bins_and_clamps() {
        let counts = bin_values(
            [0.0, 1.0, 9.99, 10.0, -5.0, 50.0, f64::NAN].into_iter(),
            (0.0, 10.0),
            10,
        );
        assert_eq!(counts[0], 2); // 0.0 and the clamped -5
        assert_eq!(counts[1], 1);
        assert_eq!(counts[9], 3); // 9.99, 10 and the clamped 50
        assert_eq!(counts.iter().sum::<u32>(), 6);
        assert_eq!(
            bin_values([3.0, 3.0].into_iter(), (3.0, 3.0), 4),
            vec![2, 0, 0, 0]
        );
    }
}
