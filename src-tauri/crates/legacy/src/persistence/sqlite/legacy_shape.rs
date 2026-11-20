//! NOT part of the frozen 0.6.0 code: the writer's 0.5.5 file shape
//! (`crate::writer::FileShape::Release055`).

use anyhow::Result;

use super::Database;

impl Database {
    /// Remove what 0.5.5 never wrote: the `station_ids` and Sections side
    /// tables and their counters (all added to schema 18 without a bump),
    /// then compact so no free page keeps their rows.
    pub fn strip_unreleased_side_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            "DROP TABLE IF EXISTS station_ids;
             DROP TABLE IF EXISTS cave_section_lines;
             DROP TABLE IF EXISTS cave_sections;
             DELETE FROM project_settings
              WHERE key IN ('next_station_id', 'next_cave_section_id');
             VACUUM;",
        )?;
        Ok(())
    }
}
