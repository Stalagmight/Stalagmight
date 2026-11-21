//! Display settings (per-machine view toggles): the file's `local.display`
//! member, held in memory (`FileState`) and made durable at once by a
//! local-only save (plan 6.5 mode 4). Ungated: view state, never pushed.

use anyhow::Result;
use std::collections::HashMap;

use super::AppState;

impl AppState {
    /// The open project's display settings.
    pub fn load_display_settings(&self) -> Result<HashMap<String, String>> {
        Ok(self.with_file(|file| {
            file.display_settings()
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()
        }))
    }

    /// Set one display setting. Durable at once on a version 2 file (a
    /// local-only save); a legacy or never-saved project keeps it in memory
    /// for its first save, without making the project unsaved (a 0.6.0 file
    /// is converted only by a save the user asked for, demo issue I-6).
    pub fn save_display_setting(&self, key: &str, value: &str) -> Result<()> {
        let changed = self.with_file(|file| {
            let display = &mut file.extras.local.display;
            if display.get(key).map(String::as_str) == Some(value) {
                return false;
            }
            display.insert(key.to_string(), value.to_string());
            true
        });
        if changed {
            self.persist_display_state()
                .map_err(|e| anyhow::anyhow!("{e}"))?;
        }
        Ok(())
    }

    /// One display setting.
    pub fn get_display_setting(&self, key: &str) -> Result<Option<String>> {
        Ok(self.with_file(|file| file.display_settings().get(key).cloned()))
    }
}
