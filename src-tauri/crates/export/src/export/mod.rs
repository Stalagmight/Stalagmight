mod atomic_write;
pub mod color_scale;
pub mod csv;
mod depth_colors;
pub mod dive_profile;
pub mod image;
pub mod json;
pub mod tml;
mod tml_azimuth;
mod tml_topology;

pub use atomic_write::write_atomic;
pub use csv::*;
pub use dive_profile::*;
pub use image::*;
pub use json::*;
pub use tml::*;

use serde::{Deserialize, Serialize};

/// Common export options
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportOptions {
    /// Include section names
    pub include_section_names: bool,
    /// Include station labels
    pub include_station_labels: bool,
    /// Include shot data (azimuth, length, depth)
    pub include_shot_data: bool,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            include_section_names: true,
            include_station_labels: true,
            include_shot_data: false,
        }
    }
}
