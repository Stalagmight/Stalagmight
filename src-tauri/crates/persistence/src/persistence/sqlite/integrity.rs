use anyhow::Result;

use super::Database;

impl Database {
    /// Run PRAGMA quick_check to detect database corruption.
    /// This is a fast subset of integrity_check that skips index verification.
    /// Called automatically on project open per SPEC 14.4.
    pub(super) fn validate_integrity(&self) -> Result<()> {
        tracing::debug!("Running PRAGMA quick_check");
        let result: String = self
            .conn
            .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
        if result != "ok" {
            tracing::error!(result = %result, "Database integrity check failed");
            anyhow::bail!("Database integrity check failed: {result}");
        }
        tracing::debug!("PRAGMA quick_check passed");
        Ok(())
    }

    /// Run full PRAGMA integrity_check for on-demand deep validation.
    /// This is slower than quick_check but also verifies index consistency.
    /// Returns a list of error messages, or empty vec if database is healthy.
    pub fn full_integrity_check(&self) -> Result<Vec<String>> {
        tracing::info!("Running PRAGMA integrity_check (full)");
        let mut stmt = self.conn.prepare("PRAGMA integrity_check")?;
        let mut rows = stmt.query([])?;
        let mut errors = Vec::new();

        while let Some(row) = rows.next()? {
            let msg: String = row.get(0)?;
            if msg != "ok" {
                errors.push(msg);
            }
        }

        if errors.is_empty() {
            tracing::info!("Full integrity check passed");
        } else {
            tracing::error!(
                error_count = errors.len(),
                "Full integrity check found errors"
            );
        }

        Ok(errors)
    }

    /// Verify there are no foreign key violations.
    pub(super) fn validate_foreign_keys(&self) -> Result<()> {
        let mut stmt = self.conn.prepare("PRAGMA foreign_key_check")?;
        let mut rows = stmt.query([])?;
        let mut count = 0usize;
        let mut samples = Vec::new();

        while let Some(row) = rows.next()? {
            count += 1;
            if samples.len() < 3 {
                let table: String = row.get(0)?;
                let rowid: i64 = row.get(1)?;
                let parent: String = row.get(2)?;
                let fkid: i64 = row.get(3)?;
                samples.push(format!("{table} rowid={rowid} parent={parent} fkid={fkid}"));
            }
        }

        if count > 0 {
            anyhow::bail!(
                "Foreign key violations detected ({}). Sample: {}",
                count,
                samples.join("; ")
            );
        }

        Ok(())
    }
}
