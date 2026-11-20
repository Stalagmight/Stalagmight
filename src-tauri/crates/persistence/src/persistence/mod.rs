pub mod legacy_link;
pub mod migrate;
pub mod plnx;
pub mod sqlite;
pub mod stalx_v2;
pub mod undo_limits;

#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: a failed setup step is a failed test"
)]
mod history_size_gate_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    clippy::panic,
    reason = "test module: a failed setup step is a failed test"
)]
mod history_v2_property_tests;
#[cfg(test)]
#[expect(
    clippy::expect_used,
    reason = "test module: a failed setup step is a failed test"
)]
mod legacy_link_tests;

pub use sqlite::*;
