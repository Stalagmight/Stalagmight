use anyhow::Result;

use crate::persistence::RawTableData;

use super::AppState;

impl AppState {
    /// List raw sqlite table names from the current project database
    pub fn list_raw_table_names(&self) -> Result<Vec<String>> {
        let db_guard = self.database.lock();
        let Some(db) = db_guard.as_ref() else {
            anyhow::bail!("No project database; save the project to browse raw data");
        };
        db.list_raw_table_names()
    }

    /// Read a page of stringified rows from a raw sqlite table in the current project database
    pub fn read_raw_table(&self, table: &str, limit: u32, offset: u32) -> Result<RawTableData> {
        let db_guard = self.database.lock();
        let Some(db) = db_guard.as_ref() else {
            anyhow::bail!("No project database; save the project to browse raw data");
        };
        db.read_raw_table(table, limit, offset)
    }
}
