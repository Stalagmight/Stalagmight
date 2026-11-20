mod atomic_write;
pub mod color_scale;
pub mod csv;
mod depth_colors;
pub mod dive_profile;
pub mod image;
mod image_labels;
mod image_sketch;
pub mod json;
pub mod survey;
mod tml;
mod tml_azimuth;

pub use atomic_write::write_atomic;
pub use csv::*;
pub use dive_profile::*;
pub use image::*;
pub use json::*;

use serde::{Deserialize, Serialize};

/// Common export options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    /// Include line names
    pub include_line_names: bool,
    /// Include station labels
    pub include_station_labels: bool,
    /// Include shot data (azimuth, length, depth)
    pub include_shot_data: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_line_names: true,
            include_station_labels: true,
            include_shot_data: false,
        }
    }
}
