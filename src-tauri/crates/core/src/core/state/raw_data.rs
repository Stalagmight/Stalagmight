use anyhow::Result;

use crate::persistence::RawTableData;

use super::AppState;

impl AppState {
    /// List raw sqlite table names from the current project database
    pub fn list_raw_table_names(&self) -> Result<Vec<String>> {
        crate::core::stub::not_ported("raw_data::list_raw_table_names")
    }

    /// Read a page of stringified rows from a raw sqlite table in the current project database
    pub fn read_raw_table(&self, table: &str, limit: u32, offset: u32) -> Result<RawTableData> {
        let _ = (&table, &limit, &offset);
        crate::core::stub::not_ported("raw_data::read_raw_table")
    }
}
