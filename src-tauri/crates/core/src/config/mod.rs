mod atomic_write;
mod column_widths;
pub mod defaults;
mod general;
mod panels;
mod speleodb_file;
mod types;
mod webkit;

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests;

pub use crate::models::config::*;
// `pub`, not `pub(crate)`: the root crate's config commands normalize widths
// through this, and they live across the crate boundary now.
pub use column_widths::normalize_column_widths;
pub use panels::*;
// SpeleoDB connection settings live with their runtime consumer
// (`crate::speleodb::config`); re-exported here so `crate::config::X` keeps working.
pub use crate::speleodb::config::*;
pub use types::*;
pub use webkit::*;
