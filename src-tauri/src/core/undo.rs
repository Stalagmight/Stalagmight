use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::Command;
use crate::models::{
    GpsCoordinates, LoopClosure, Marker, SectionData, SectionId, ShotData, ShotId, StationRef,
};

/// Default maximum number of undo entries to keep in memory
const DEFAULT_UNDO_HISTORY_LIMIT: usize = 100;

/// Undo/redo stack
#[derive(Debug)]
#[expect(clippy::struct_field_names)]
pub struct UndoStack {
    /// Commands that can be undone
    undo_stack: VecDeque<UndoEntry>,
    /// Commands that can be redone
    redo_stack: VecDeque<UndoEntry>,
    /// Maximum entries to keep in memory
    history_limit: usize,
}

/// Entry in the undo stack
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    /// The command that was executed
    pub command: Command,
    /// Data needed to undo this command
    pub undo_data: UndoData,
    /// Timestamp when the command was executed (None for legacy entries)
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
}

/// Data needed to undo a command
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UndoData {
    /// Simple inverse command (e.g., SetShotReference -> SetShotReference(!), MergeStations -> UnmergeStations)
    InverseCommand(Command),

    /// Previous values for an update operation (JSON serialized)
    PreviousValues {
        section_id: SectionId,
        shot_id: Option<ShotId>,
        /// JSON serialized previous values (ShotUpdates or SectionUpdates)
        values_json: String,
    },

    /// Full section snapshot for restoring deleted sections
    SectionSnapshot {
        section_id: SectionId,
        /// Full section data including all shots
        section_data: SectionData,
    },

    /// Snapshot of a newly created section (for CreateSection undo/redo ID preservation)
    /// Distinct from SectionSnapshot (used by DeleteSection) to avoid ambiguity in redo logic.
    CreatedSectionSnapshot {
        section_id: SectionId,
        /// Full section data including all shots
        section_data: SectionData,
    },

    /// Shot snapshot for restoring deleted shots
    ShotSnapshot {
        section_id: SectionId,
        shot_id: ShotId,
        shot_data: ShotData,
        /// Position in the section's shot list (for re-insertion)
        position: usize,
        /// The deleted shot's stable collaboration UUID, so undo restores the
        /// same identity (cross-references and Ariane matching anchor on it)
        /// rather than minting a fresh one. `None` for legacy persisted entries.
        #[serde(default)]
        uuid: Option<String>,
    },

    /// Snapshot of a newly created shot (for CreateShot undo/redo ID preservation)
    /// Distinct from ShotSnapshot (used by DeleteShot) to avoid ambiguity in redo logic.
    CreatedShotSnapshot {
        section_id: SectionId,
        shot_id: ShotId,
        shot_data: ShotData,
        /// Position in the section's shot list (for re-insertion at correct position)
        position: usize,
        /// Station index from which to shift refs when restoring (redo).
        /// For insert_before: same as position. For insert_after/append: position + 1.
        /// Undo of CreateShot (delete) uses the inverse shift automatically via remove_shot.
        #[serde(default)]
        station_shift_from: u32,
        /// The original command's anchor shot. Redo re-runs the SAME
        /// `insert_shot_after`/`insert_shot_before` (with `shot_id` forced) so the
        /// forward and redo paths share one code path and can't drift apart. Both
        /// `None` means the shot was appended.
        #[serde(default)]
        after_shot_id: Option<ShotId>,
        #[serde(default)]
        before_shot_id: Option<ShotId>,
        /// The shot's stable collaboration UUID at creation. Redo restores it via
        /// `Shot::with_uuid` so the recreated shot keeps the same identity that
        /// cross-references (`AfterShot(uuid)`) and Ariane matching anchor on.
        /// `None` for legacy persisted entries written before this field existed
        /// (redo then mints a fresh UUID, the pre-fix behavior).
        #[serde(default)]
        uuid: Option<String>,
    },

    /// Merge info for unmerging stations
    MergeInfo {
        station_a: StationRef,
        station_b: StationRef,
        /// Closures removed during unmerge (for restoring on undo)
        #[serde(default)]
        removed_closures: Vec<LoopClosure>,
        /// Section offsets before the merge (for restoring positions on undo)
        #[serde(default)]
        previous_offsets: HashMap<SectionId, Option<(f64, f64)>>,
    },

    /// Unmerge info for re-merging stations on undo (opposite of MergeInfo)
    UnmergeInfo {
        station_a: StationRef,
        station_b: StationRef,
        /// Closures removed during unmerge (for restoring on undo)
        #[serde(default)]
        removed_closures: Vec<LoopClosure>,
    },

    /// Loop closure info for undoing CreateLoopClosure
    ClosureInfo {
        /// The full loop closure data to remove on undo
        closure: LoopClosure,
    },

    /// Marker info for undoing AddMarker / UpdateMarker / DeleteMarker
    MarkerInfo { marker: Marker },

    /// Zero datum info for restoring previous GPS and section offsets
    ZeroDatumInfo {
        station_ref: StationRef,
        /// Previous GPS coordinates, None if there was no previous datum
        previous_gps: Option<GpsCoordinates>,
        /// Previous section offsets for affected sections
        /// - None value means the key didn't exist (should be removed on undo)
        /// - Some value means the offset should be restored to that value
        #[serde(default)]
        previous_offsets: HashMap<SectionId, Option<(f64, f64)>>,
    },

    /// Per-section previous offsets for segment moves (captured at execution time)
    /// Unlike InverseCommand, this stores the exact set of affected sections and their
    /// previous offsets so that undo/redo is correct even if the connectivity graph changes.
    SegmentMoveInfo {
        /// Section IDs that were moved (captured at execution time)
        affected_sections: Vec<SectionId>,
        /// Previous offsets for each affected section (None = no offset existed)
        previous_offsets: HashMap<SectionId, Option<(f64, f64)>>,
        /// The offset that was applied (for redo)
        offset: (f64, f64),
    },

    /// Previous shot data and station offset for station moves.
    /// MoveStation modifies adjacent shots' geometry (length/azimuth/depth) directly,
    /// so undo needs to restore the original shot data and station offset.
    StationMoveInfo {
        station_ref: StationRef,
        /// Shot data for the preceding shot (shot at station_index - 1), if any
        prev_shot_before: Option<(ShotId, ShotData)>,
        /// Shot data for the following shot (shot at station_index), if any
        prev_shot_after: Option<(ShotId, ShotData)>,
        /// Previous station offset (None = no offset existed)
        prev_offset: Option<(f64, f64, f64)>,
        /// Previous section offset (None = no offset existed)
        #[serde(default)]
        prev_section_offset: Option<(f64, f64)>,
    },

    /// Full project snapshot for batch operations (JSON serialized)
    ProjectSnapshot(String),

    /// Before and after project snapshots for operations that can't be re-executed
    /// (e.g., batch operations that create sections with dynamic IDs)
    /// - before: Snapshot before the operation (used for undo)
    /// - after: Snapshot after the operation (used for redo)
    ProjectSnapshotPair { before: String, after: String },

    /// No undo data (for commands that can't be undone)
    None,
}

impl UndoData {
    /// Return a short, safe variant name for logging (no payload data)
    pub fn variant_name(&self) -> &'static str {
        match self {
            UndoData::InverseCommand(_) => "InverseCommand",
            UndoData::PreviousValues { .. } => "PreviousValues",
            UndoData::SectionSnapshot { .. } => "SectionSnapshot",
            UndoData::CreatedSectionSnapshot { .. } => "CreatedSectionSnapshot",
            UndoData::ShotSnapshot { .. } => "ShotSnapshot",
            UndoData::CreatedShotSnapshot { .. } => "CreatedShotSnapshot",
            UndoData::MergeInfo { .. } => "MergeInfo",
            UndoData::UnmergeInfo { .. } => "UnmergeInfo",
            UndoData::ClosureInfo { .. } => "ClosureInfo",
            UndoData::MarkerInfo { .. } => "MarkerInfo",
            UndoData::ZeroDatumInfo { .. } => "ZeroDatumInfo",
            UndoData::SegmentMoveInfo { .. } => "SegmentMoveInfo",
            UndoData::StationMoveInfo { .. } => "StationMoveInfo",
            UndoData::ProjectSnapshot(_) => "ProjectSnapshot",
            UndoData::ProjectSnapshotPair { .. } => "ProjectSnapshotPair",
            UndoData::None => "None",
        }
    }
}

impl Default for UndoStack {
    fn default() -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            history_limit: DEFAULT_UNDO_HISTORY_LIMIT,
        }
    }
}

impl UndoStack {
    /// Create a new empty undo stack with default limit
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a new empty undo stack with custom limit
    pub fn new_with_limit(history_limit: usize) -> Self {
        Self {
            undo_stack: VecDeque::new(),
            redo_stack: VecDeque::new(),
            history_limit,
        }
    }

    /// Get the current history limit
    pub fn history_limit(&self) -> usize {
        self.history_limit
    }

    /// Set the history limit
    pub fn set_history_limit(&mut self, limit: usize) {
        self.history_limit = limit;
        // Trim both stacks if needed
        while self.undo_stack.len() > self.history_limit {
            self.undo_stack.pop_front();
        }
        while self.redo_stack.len() > self.history_limit {
            self.redo_stack.pop_front();
        }
    }

    /// Push a command with its undo data to the undo stack
    pub fn push_with_data(&mut self, command: Command, undo_data: UndoData) {
        let entry = UndoEntry {
            command,
            undo_data,
            timestamp: Some(Utc::now()),
        };

        self.undo_stack.push_back(entry);

        // Clear redo stack when new command is executed
        self.redo_stack.clear();

        // Limit stack size
        while self.undo_stack.len() > self.history_limit {
            self.undo_stack.pop_front();
        }
    }

    /// Push a command without undo data (convenience wrapper, timestamps are added automatically)
    pub fn push(&mut self, command: Command) {
        self.push_with_data(command, UndoData::None);
    }

    /// Pop from undo stack and return the entry
    pub fn pop_undo(&mut self) -> Option<UndoEntry> {
        self.undo_stack.pop_back()
    }

    /// Push to redo stack
    pub fn push_redo(&mut self, entry: UndoEntry) {
        self.redo_stack.push_back(entry);
    }

    /// Pop from redo stack
    pub fn pop_redo(&mut self) -> Option<UndoEntry> {
        self.redo_stack.pop_back()
    }

    /// Push back to undo stack (for redo operations)
    pub fn push_undo(&mut self, entry: UndoEntry) {
        self.undo_stack.push_back(entry);
        // Don't clear redo stack when pushing for redo
    }

    /// Check if undo is available
    pub fn can_undo(&self) -> bool {
        !self.undo_stack.is_empty()
    }

    /// Check if redo is available
    pub fn can_redo(&self) -> bool {
        !self.redo_stack.is_empty()
    }

    /// Clear both stacks
    pub fn clear(&mut self) {
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Get the last command that can be undone
    pub fn peek_undo(&self) -> Option<&Command> {
        self.undo_stack.back().map(|e| &e.command)
    }

    /// Get the last command that can be redone
    pub fn peek_redo(&self) -> Option<&Command> {
        self.redo_stack.back().map(|e| &e.command)
    }

    /// Get the undo data for the last undoable command
    pub fn peek_undo_data(&self) -> Option<&UndoData> {
        self.undo_stack.back().map(|e| &e.undo_data)
    }

    /// Get undo entries to persist (last N entries up to persist_limit)
    pub fn entries_to_persist(&self, persist_limit: usize) -> impl Iterator<Item = &UndoEntry> {
        let skip = if self.undo_stack.len() > persist_limit {
            self.undo_stack.len() - persist_limit
        } else {
            0
        };
        self.undo_stack.iter().skip(skip)
    }

    /// Get redo entries to persist (last N entries up to persist_limit)
    pub fn redo_entries_to_persist(
        &self,
        persist_limit: usize,
    ) -> impl Iterator<Item = &UndoEntry> {
        let skip = if self.redo_stack.len() > persist_limit {
            self.redo_stack.len() - persist_limit
        } else {
            0
        };
        self.redo_stack.iter().skip(skip)
    }

    /// Serialize an entry to JSON for persistence
    /// Returns (command_json, undo_data_json, timestamp_iso8601)
    pub fn serialize_entry(entry: &UndoEntry) -> Option<(String, String)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = serde_json::to_string(&entry.undo_data).ok()?;
        Some((command_json, undo_data_json))
    }

    /// Serialize an entry with timestamp for database persistence
    /// Returns (command_json, undo_data_json, timestamp_iso8601)
    pub fn serialize_entry_with_timestamp(
        entry: &UndoEntry,
    ) -> Option<(String, String, Option<String>)> {
        let command_json = serde_json::to_string(&entry.command).ok()?;
        let undo_data_json = serde_json::to_string(&entry.undo_data).ok()?;
        let timestamp_str = entry.timestamp.map(|t| t.to_rfc3339());
        Some((command_json, undo_data_json, timestamp_str))
    }

    /// Deserialize an entry from JSON
    pub fn deserialize_entry(command_json: &str, undo_data_json: &str) -> Option<UndoEntry> {
        let command: Command = match serde_json::from_str(command_json) {
            Ok(command) => command,
            Err(e) => {
                // A command that no longer exists in this build (e.g. a variant
                // removed in a newer version) can't be deserialized. Skip the
                // entry rather than aborting the whole load, but log it so the
                // truncation of undo history is not silent.
                tracing::warn!(
                    error = %e,
                    purpose = "load persisted undo history",
                    "Skipping an undo entry whose command could not be deserialized; its undo step is dropped"
                );
                return None;
            }
        };
        let undo_data: UndoData = serde_json::from_str(undo_data_json).ok()?;
        Some(UndoEntry {
            command,
            undo_data,
            timestamp: None, // Legacy entries without timestamp
        })
    }

    /// Deserialize an entry from JSON with optional timestamp
    pub fn deserialize_entry_with_timestamp(
        command_json: &str,
        undo_data_json: &str,
        timestamp_str: Option<&str>,
    ) -> Option<UndoEntry> {
        let command: Command = match serde_json::from_str(command_json) {
            Ok(command) => command,
            Err(e) => {
                // A command that no longer exists in this build (e.g. a variant
                // removed in a newer version) can't be deserialized. Skip the
                // entry rather than aborting the whole load, but log it so the
                // truncation of undo history is not silent.
                tracing::warn!(
                    error = %e,
                    purpose = "load persisted undo history",
                    "Skipping an undo entry whose command could not be deserialized; its undo step is dropped"
                );
                return None;
            }
        };
        let undo_data: UndoData = serde_json::from_str(undo_data_json).ok()?;
        let timestamp = timestamp_str
            .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&Utc));
        Some(UndoEntry {
            command,
            undo_data,
            timestamp,
        })
    }

    /// Load entries from persisted data
    pub fn load_from_persisted(&mut self, entries: Vec<(String, String)>) {
        self.undo_stack.clear();
        self.redo_stack.clear();

        for (command_json, undo_data_json) in entries {
            if let Some(entry) = Self::deserialize_entry(&command_json, &undo_data_json) {
                self.undo_stack.push_back(entry);
            }
        }

        // Trim to history limit
        while self.undo_stack.len() > self.history_limit {
            self.undo_stack.pop_front();
        }
    }

    /// Load entries from persisted data with timestamps
    pub fn load_from_persisted_with_timestamps(
        &mut self,
        undo_entries: Vec<(String, String, Option<String>)>,
        redo_entries: Vec<(String, String, Option<String>)>,
    ) {
        self.undo_stack.clear();
        self.redo_stack.clear();

        for (command_json, undo_data_json, timestamp_str) in undo_entries {
            if let Some(entry) = Self::deserialize_entry_with_timestamp(
                &command_json,
                &undo_data_json,
                timestamp_str.as_deref(),
            ) {
                self.undo_stack.push_back(entry);
            }
        }

        for (command_json, undo_data_json, timestamp_str) in redo_entries {
            if let Some(entry) = Self::deserialize_entry_with_timestamp(
                &command_json,
                &undo_data_json,
                timestamp_str.as_deref(),
            ) {
                self.redo_stack.push_back(entry);
            }
        }

        // Trim to history limit
        while self.undo_stack.len() > self.history_limit {
            self.undo_stack.pop_front();
        }
        while self.redo_stack.len() > self.history_limit {
            self.redo_stack.pop_front();
        }
    }

    /// Get the number of entries in the undo stack
    pub fn len(&self) -> usize {
        self.undo_stack.len()
    }

    /// Check if the undo stack is empty
    pub fn is_empty(&self) -> bool {
        self.undo_stack.is_empty()
    }

    /// Get the number of entries in the redo stack
    pub fn redo_len(&self) -> usize {
        self.redo_stack.len()
    }

    /// Get an iterator over the undo stack entries (index 0 = most recent)
    /// Returns entries in LIFO order (most recent first)
    pub fn undo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.undo_stack.iter().rev()
    }

    /// Get an iterator over the redo stack entries (index 0 = most recent)
    /// Returns entries in LIFO order (most recent first)
    pub fn redo_entries(&self) -> impl Iterator<Item = &UndoEntry> {
        self.redo_stack.iter().rev()
    }
}

#[cfg(test)]
#[expect(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::float_cmp
)]
mod tests {
    use super::*;
    use crate::models::ShotData;

    /// Helper to create a test command
    fn test_create_section_command(name: &str) -> Command {
        Command::CreateSection {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            shots: vec![],
        }
    }

    /// Helper to create a test delete section command
    fn test_delete_section_command(section_id: SectionId) -> Command {
        Command::DeleteSection { section_id }
    }

    /// Helper to create test section data
    fn test_section_data(name: &str) -> SectionData {
        SectionData {
            name: Some(name.to_string()),
            surveyor: None,
            date: None,
            color: "#ff0000".to_string(),
            direction: None,
            quality: 1.0,
            shots: vec![],
        }
    }

    // === Undo Stack Basic Tests ===

    #[test]
    fn test_new_undo_stack_is_empty() {
        let stack = UndoStack::new();
        assert!(stack.is_empty());
        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_new_with_limit() {
        let stack = UndoStack::new_with_limit(50);
        assert_eq!(stack.history_limit(), 50);
        assert!(stack.is_empty());
    }

    #[test]
    fn test_default_history_limit() {
        let stack = UndoStack::new();
        assert_eq!(stack.history_limit(), 100);
    }

    #[test]
    fn test_push_enables_undo() {
        let mut stack = UndoStack::new();
        let cmd = test_create_section_command("Test");

        stack.push_with_data(cmd, UndoData::None);

        assert!(stack.can_undo());
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_push_multiple_commands() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_section_command("First"), UndoData::None);
        stack.push_with_data(test_create_section_command("Second"), UndoData::None);
        stack.push_with_data(test_create_section_command("Third"), UndoData::None);

        assert_eq!(stack.len(), 3);
        assert!(stack.can_undo());
    }

    #[test]
    fn test_pop_undo_returns_last_command() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_section_command("First"), UndoData::None);
        stack.push_with_data(test_create_section_command("Second"), UndoData::None);

        let entry = stack.pop_undo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Second".to_string()));
        } else {
            panic!("Expected CreateSection command");
        }

        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_pop_undo_empty_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.pop_undo().is_none());
    }

    #[test]
    fn test_peek_undo_does_not_modify_stack() {
        let mut stack = UndoStack::new();
        let cmd = test_create_section_command("Test");

        stack.push_with_data(cmd, UndoData::None);

        let peeked = stack.peek_undo();
        assert!(peeked.is_some());
        assert_eq!(stack.len(), 1); // Stack unchanged
    }

    #[test]
    fn test_clear_removes_all_entries() {
        let mut stack = UndoStack::new();

        stack.push_with_data(test_create_section_command("First"), UndoData::None);
        stack.push_with_data(test_create_section_command("Second"), UndoData::None);
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        stack.clear();

        assert!(!stack.can_undo());
        assert!(!stack.can_redo());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 0);
    }

    // === Redo Stack Tests ===

    #[test]
    fn test_push_redo_enables_redo() {
        let mut stack = UndoStack::new();
        let entry = UndoEntry {
            command: test_create_section_command("Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        stack.push_redo(entry);

        assert!(stack.can_redo());
        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_pop_redo_returns_last_entry() {
        let mut stack = UndoStack::new();

        stack.push_redo(UndoEntry {
            command: test_create_section_command("First"),
            undo_data: UndoData::None,
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Second"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        let entry = stack.pop_redo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Second".to_string()));
        } else {
            panic!("Expected CreateSection command");
        }

        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_pop_redo_empty_returns_none() {
        let mut stack = UndoStack::new();
        assert!(stack.pop_redo().is_none());
    }

    #[test]
    fn test_peek_redo_does_not_modify_stack() {
        let mut stack = UndoStack::new();
        let entry = UndoEntry {
            command: test_create_section_command("Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        stack.push_redo(entry);

        let peeked = stack.peek_redo();
        assert!(peeked.is_some());
        assert_eq!(stack.redo_len(), 1); // Stack unchanged
    }

    #[test]
    fn test_push_clears_redo_stack() {
        let mut stack = UndoStack::new();

        // Add some redo entries
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Redo1"),
            undo_data: UndoData::None,
            timestamp: None,
        });
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Redo2"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        assert!(stack.can_redo());

        // New command should clear redo stack
        stack.push_with_data(test_create_section_command("New"), UndoData::None);

        assert!(!stack.can_redo());
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_push_undo_does_not_clear_redo_stack() {
        let mut stack = UndoStack::new();

        // Add redo entries
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        // push_undo is used for redo operations and should NOT clear redo
        stack.push_undo(UndoEntry {
            command: test_create_section_command("Restored"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        assert!(stack.can_redo()); // Redo stack preserved
    }

    // === History Limit Tests ===

    #[test]
    fn test_history_limit_enforced_on_push() {
        let mut stack = UndoStack::new_with_limit(3);

        for i in 0..5 {
            stack.push_with_data(
                test_create_section_command(&format!("Section {i}")),
                UndoData::None,
            );
        }

        assert_eq!(stack.len(), 3);

        // The oldest entries should be removed
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Section 4".to_string()));
        } else {
            panic!("Expected Section 4");
        }
    }

    #[test]
    fn test_set_history_limit_trims_stack() {
        let mut stack = UndoStack::new();

        for i in 0..10 {
            stack.push_with_data(
                test_create_section_command(&format!("Section {i}")),
                UndoData::None,
            );
        }

        assert_eq!(stack.len(), 10);

        // Reduce limit - should trim oldest entries
        stack.set_history_limit(5);

        assert_eq!(stack.len(), 5);
        assert_eq!(stack.history_limit(), 5);

        // Verify we have the most recent entries
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Section 9".to_string()));
        } else {
            panic!("Expected Section 9");
        }
    }

    #[test]
    fn test_set_history_limit_zero() {
        let mut stack = UndoStack::new();
        stack.push_with_data(test_create_section_command("Test"), UndoData::None);

        stack.set_history_limit(0);

        assert_eq!(stack.len(), 0);
        assert!(!stack.can_undo());
    }

    #[test]
    fn test_set_history_limit_trims_redo_stack() {
        let mut stack = UndoStack::new();

        // Build up a redo stack by pushing then undoing
        for i in 0..10 {
            stack.push_redo(UndoEntry {
                command: test_create_section_command(&format!("Redo {i}")),
                undo_data: UndoData::None,
                timestamp: None,
            });
        }

        assert_eq!(stack.redo_len(), 10);

        // Reduce limit - should trim redo stack too
        stack.set_history_limit(3);

        assert_eq!(stack.redo_len(), 3);
    }

    // === UndoData Tests ===

    #[test]
    fn test_undo_data_inverse_command() {
        let mut stack = UndoStack::new();

        let inverse = Command::DeleteSection { section_id: 42 };
        stack.push_with_data(
            test_create_section_command("Test"),
            UndoData::InverseCommand(inverse),
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::InverseCommand(cmd) = data {
            if let Command::DeleteSection { section_id } = cmd {
                assert_eq!(*section_id, 42);
            } else {
                panic!("Expected DeleteSection");
            }
        } else {
            panic!("Expected InverseCommand");
        }
    }

    #[test]
    fn test_undo_data_section_snapshot() {
        let mut stack = UndoStack::new();

        let section_data = test_section_data("Snapshot Test");
        stack.push_with_data(
            test_delete_section_command(5),
            UndoData::SectionSnapshot {
                section_id: 5,
                section_data,
            },
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::SectionSnapshot {
            section_id,
            section_data: stored_data,
        } = data
        {
            assert_eq!(*section_id, 5);
            assert_eq!(stored_data.name, Some("Snapshot Test".to_string()));
        } else {
            panic!("Expected SectionSnapshot");
        }
    }

    #[test]
    fn test_undo_data_shot_snapshot() {
        let mut stack = UndoStack::new();

        let shot_data = ShotData {
            azimuth_in: 45.0,
            azimuth_out: 225.0,
            length: 10.0,
            depth_in: 5.0,
            depth_out: 6.0,
            ..Default::default()
        };

        stack.push_with_data(
            Command::DeleteShot {
                section_id: 1,
                shot_id: 100,
            },
            UndoData::ShotSnapshot {
                section_id: 1,
                shot_id: 100,
                shot_data,
                position: 2,
                uuid: None,
            },
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::ShotSnapshot {
            section_id,
            shot_id,
            shot_data: stored_data,
            position,
            ..
        } = data
        {
            assert_eq!(*section_id, 1);
            assert_eq!(*shot_id, 100);
            assert_eq!(stored_data.length, 10.0);
            assert_eq!(*position, 2);
        } else {
            panic!("Expected ShotSnapshot");
        }
    }

    #[test]
    fn test_undo_data_merge_info() {
        let mut stack = UndoStack::new();

        stack.push_with_data(
            Command::MergeStations {
                station_a: StationRef::new(1, 0),
                station_b: StationRef::new(2, 3),
            },
            UndoData::MergeInfo {
                station_a: StationRef::new(1, 0),
                station_b: StationRef::new(2, 3),
                removed_closures: Vec::new(),
                previous_offsets: HashMap::new(),
            },
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::MergeInfo {
            station_a,
            station_b,
            removed_closures,
            ..
        } = data
        {
            assert_eq!(station_a.section_id, 1);
            assert_eq!(station_a.station_index, 0);
            assert_eq!(station_b.section_id, 2);
            assert_eq!(station_b.station_index, 3);
            assert!(removed_closures.is_empty());
        } else {
            panic!("Expected MergeInfo");
        }
    }

    #[test]
    fn test_undo_data_zero_datum_info() {
        let mut stack = UndoStack::new();

        let gps = GpsCoordinates::new(45.0, -73.5, 100.0).unwrap();
        let mut offsets = HashMap::new();
        offsets.insert(1, Some((10.0, 20.0)));
        offsets.insert(2, None); // Key didn't exist before
        stack.push_with_data(
            Command::SetZeroDatum {
                station_ref: StationRef::new(1, 0),
                coordinates: gps,
            },
            UndoData::ZeroDatumInfo {
                station_ref: StationRef::new(1, 0),
                previous_gps: Some(GpsCoordinates::new(40.0, -70.0, 50.0).unwrap()),
                previous_offsets: offsets,
            },
        );

        let data = stack.peek_undo_data().unwrap();
        if let UndoData::ZeroDatumInfo {
            station_ref,
            previous_gps,
            previous_offsets,
        } = data
        {
            assert_eq!(station_ref.section_id, 1);
            let prev = previous_gps.as_ref().unwrap();
            assert_eq!(prev.latitude, 40.0);
            assert_eq!(previous_offsets.get(&1), Some(&Some((10.0, 20.0))));
            assert_eq!(previous_offsets.get(&2), Some(&None));
        } else {
            panic!("Expected ZeroDatumInfo");
        }
    }

    // === Serialization / Persistence Tests ===

    #[test]
    fn test_serialize_entry_basic() {
        let entry = UndoEntry {
            command: test_create_section_command("Serialize Test"),
            undo_data: UndoData::None,
            timestamp: None,
        };

        let result = UndoStack::serialize_entry(&entry);
        assert!(result.is_some());

        let (cmd_json, undo_json) = result.unwrap();
        assert!(cmd_json.contains("Serialize Test"));
        assert!(undo_json.contains("None"));
    }

    #[test]
    fn test_deserialize_entry_basic() {
        let entry = UndoEntry {
            command: test_create_section_command("Deserialize Test"),
            undo_data: UndoData::InverseCommand(test_delete_section_command(42)),
            timestamp: None,
        };

        let (cmd_json, undo_json) = UndoStack::serialize_entry(&entry).unwrap();
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json).unwrap();

        if let Command::CreateSection { name, .. } = restored.command {
            assert_eq!(name, Some("Deserialize Test".to_string()));
        } else {
            panic!("Expected CreateSection");
        }

        if let UndoData::InverseCommand(Command::DeleteSection { section_id }) = restored.undo_data
        {
            assert_eq!(section_id, 42);
        } else {
            panic!("Expected InverseCommand with DeleteSection");
        }
    }

    #[test]
    fn test_deserialize_entry_invalid_json() {
        let result = UndoStack::deserialize_entry("not valid json", "{}");
        assert!(result.is_none());

        let result = UndoStack::deserialize_entry("{}", "not valid json");
        assert!(result.is_none());
    }

    #[test]
    fn test_serialize_entry_with_section_snapshot() {
        let entry = UndoEntry {
            command: test_delete_section_command(5),
            undo_data: UndoData::SectionSnapshot {
                section_id: 5,
                section_data: test_section_data("Snapshot"),
            },
            timestamp: None,
        };

        let result = UndoStack::serialize_entry(&entry);
        assert!(result.is_some());

        let (cmd_json, undo_json) = result.unwrap();
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json).unwrap();

        if let UndoData::SectionSnapshot {
            section_id,
            section_data,
        } = restored.undo_data
        {
            assert_eq!(section_id, 5);
            assert_eq!(section_data.name, Some("Snapshot".to_string()));
        } else {
            panic!("Expected SectionSnapshot");
        }
    }

    #[test]
    fn test_load_from_persisted() {
        let mut stack = UndoStack::new();

        // Create serialized entries
        let entries: Vec<(String, String)> = (0..3)
            .map(|i| {
                let entry = UndoEntry {
                    command: test_create_section_command(&format!("Section {i}")),
                    undo_data: UndoData::InverseCommand(test_delete_section_command(i as u32)),
                    timestamp: None,
                };
                UndoStack::serialize_entry(&entry).unwrap()
            })
            .collect();

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 3);
        assert!(stack.can_undo());
        assert!(!stack.can_redo()); // Redo should be cleared

        // Verify entries loaded in correct order
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Section 2".to_string()));
        } else {
            panic!("Expected Section 2");
        }
    }

    #[test]
    fn test_load_from_persisted_respects_limit() {
        let mut stack = UndoStack::new_with_limit(2);

        // Create more entries than the limit
        let entries: Vec<(String, String)> = (0..5)
            .map(|i| {
                let entry = UndoEntry {
                    command: test_create_section_command(&format!("Section {i}")),
                    undo_data: UndoData::None,
                    timestamp: None,
                };
                UndoStack::serialize_entry(&entry).unwrap()
            })
            .collect();

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 2);

        // Should have the most recent entries
        let entry = stack.pop_undo().unwrap();
        if let Command::CreateSection { name, .. } = entry.command {
            assert_eq!(name, Some("Section 4".to_string()));
        } else {
            panic!("Expected Section 4");
        }
    }

    #[test]
    fn test_load_from_persisted_clears_existing() {
        let mut stack = UndoStack::new();

        // Add some entries
        stack.push_with_data(test_create_section_command("Existing"), UndoData::None);
        stack.push_redo(UndoEntry {
            command: test_create_section_command("Existing Redo"),
            undo_data: UndoData::None,
            timestamp: None,
        });

        // Load persisted data should clear existing
        let entries: Vec<(String, String)> = vec![UndoStack::serialize_entry(&UndoEntry {
            command: test_create_section_command("Loaded"),
            undo_data: UndoData::None,
            timestamp: None,
        })
        .unwrap()];

        stack.load_from_persisted(entries);

        assert_eq!(stack.len(), 1);
        assert_eq!(stack.redo_len(), 0);
    }

    #[test]
    fn test_load_from_persisted_skips_invalid() {
        let mut stack = UndoStack::new();

        let valid_entry = UndoStack::serialize_entry(&UndoEntry {
            command: test_create_section_command("Valid"),
            undo_data: UndoData::None,
            timestamp: None,
        })
        .unwrap();

        let entries = vec![
            ("invalid json".to_string(), "{}".to_string()),
            valid_entry,
            ("{}".to_string(), "also invalid".to_string()),
        ];

        stack.load_from_persisted(entries);

        // Only the valid entry should be loaded
        assert_eq!(stack.len(), 1);
    }

    #[test]
    fn test_entries_to_persist_all_entries() {
        let mut stack = UndoStack::new();

        for i in 0..5 {
            stack.push_with_data(
                test_create_section_command(&format!("Section {i}")),
                UndoData::None,
            );
        }

        let persisted: Vec<_> = stack.entries_to_persist(10).collect();
        assert_eq!(persisted.len(), 5);
    }

    #[test]
    fn test_entries_to_persist_limited() {
        let mut stack = UndoStack::new();

        for i in 0..10 {
            stack.push_with_data(
                test_create_section_command(&format!("Section {i}")),
                UndoData::None,
            );
        }

        let persisted: Vec<_> = stack.entries_to_persist(3).collect();
        assert_eq!(persisted.len(), 3);

        // Should be the most recent 3
        if let Command::CreateSection { name, .. } = &persisted[0].command {
            assert_eq!(*name, Some("Section 7".to_string()));
        } else {
            panic!("Expected Section 7");
        }
    }

    // === Complex Operation Sequences ===

    #[test]
    fn test_undo_redo_cycle() {
        let mut stack = UndoStack::new();

        // Push command
        let entry = UndoEntry {
            command: test_create_section_command("Test"),
            undo_data: UndoData::InverseCommand(test_delete_section_command(1)),
            timestamp: None,
        };
        stack.push_with_data(entry.command.clone(), entry.undo_data);

        assert!(stack.can_undo());
        assert!(!stack.can_redo());

        // Simulate undo: pop from undo, push to redo
        let undone = stack.pop_undo().unwrap();
        stack.push_redo(undone);

        assert!(!stack.can_undo());
        assert!(stack.can_redo());

        // Simulate redo: pop from redo, push to undo
        let redone = stack.pop_redo().unwrap();
        stack.push_undo(redone);

        assert!(stack.can_undo());
        assert!(!stack.can_redo());
    }

    #[test]
    fn test_multiple_undo_redo_cycles() {
        let mut stack = UndoStack::new();

        // Push 3 commands
        for i in 0..3 {
            stack.push_with_data(
                test_create_section_command(&format!("Section {i}")),
                UndoData::InverseCommand(test_delete_section_command(i as u32)),
            );
        }

        assert_eq!(stack.len(), 3);

        // Undo all 3
        for _ in 0..3 {
            let entry = stack.pop_undo().unwrap();
            stack.push_redo(entry);
        }

        assert_eq!(stack.len(), 0);
        assert_eq!(stack.redo_len(), 3);

        // Redo 2
        for _ in 0..2 {
            let entry = stack.pop_redo().unwrap();
            stack.push_undo(entry);
        }

        assert_eq!(stack.len(), 2);
        assert_eq!(stack.redo_len(), 1);
    }

    #[test]
    fn test_new_command_after_undo_clears_redo() {
        let mut stack = UndoStack::new();

        // Create -> Undo -> New action (should clear redo)
        stack.push_with_data(test_create_section_command("Original"), UndoData::None);

        let entry = stack.pop_undo().unwrap();
        stack.push_redo(entry);

        assert!(stack.can_redo());

        // New command should clear redo
        stack.push_with_data(test_create_section_command("New"), UndoData::None);

        assert!(!stack.can_redo());
        assert!(stack.can_undo());
    }

    #[test]
    fn test_roundtrip_serialization_all_undo_data_types() {
        // Test that all UndoData variants can be serialized and deserialized

        let test_cases: Vec<UndoEntry> = vec![
            UndoEntry {
                command: test_create_section_command("None"),
                undo_data: UndoData::None,
                timestamp: None,
            },
            UndoEntry {
                command: test_create_section_command("Inverse"),
                undo_data: UndoData::InverseCommand(test_delete_section_command(1)),
                timestamp: None,
            },
            UndoEntry {
                command: test_delete_section_command(2),
                undo_data: UndoData::SectionSnapshot {
                    section_id: 2,
                    section_data: test_section_data("Snapshot"),
                },
                timestamp: None,
            },
            UndoEntry {
                command: test_create_section_command("Created Snapshot"),
                undo_data: UndoData::CreatedSectionSnapshot {
                    section_id: 3,
                    section_data: test_section_data("Created"),
                },
                timestamp: None,
            },
            UndoEntry {
                command: Command::DeleteShot {
                    section_id: 1,
                    shot_id: 100,
                },
                undo_data: UndoData::ShotSnapshot {
                    section_id: 1,
                    shot_id: 100,
                    shot_data: ShotData {
                        length: 5.0,
                        ..Default::default()
                    },
                    position: 0,
                    uuid: None,
                },
                timestamp: None,
            },
            UndoEntry {
                command: Command::CreateShot {
                    section_id: 1,
                    shot: ShotData {
                        length: 8.0,
                        ..Default::default()
                    },
                    after_shot_id: None,
                    before_shot_id: None,
                },
                undo_data: UndoData::CreatedShotSnapshot {
                    section_id: 1,
                    shot_id: 200,
                    shot_data: ShotData {
                        length: 8.0,
                        ..Default::default()
                    },
                    position: 0,
                    station_shift_from: 1,
                    after_shot_id: None,
                    before_shot_id: None,
                    uuid: None,
                },
                timestamp: None,
            },
            UndoEntry {
                command: Command::SetZeroDatum {
                    station_ref: StationRef::new(1, 0),
                    coordinates: GpsCoordinates::new(45.0, -73.0, 100.0).unwrap(),
                },
                undo_data: UndoData::ZeroDatumInfo {
                    station_ref: StationRef::new(1, 0),
                    previous_gps: None,
                    previous_offsets: HashMap::new(),
                },
                timestamp: None,
            },
            UndoEntry {
                command: Command::MergeStations {
                    station_a: StationRef::new(1, 0),
                    station_b: StationRef::new(2, 1),
                },
                undo_data: UndoData::MergeInfo {
                    station_a: StationRef::new(1, 0),
                    station_b: StationRef::new(2, 1),
                    removed_closures: Vec::new(),
                    previous_offsets: HashMap::new(),
                },
                timestamp: None,
            },
            UndoEntry {
                command: Command::Batch { commands: vec![] },
                undo_data: UndoData::ProjectSnapshot("{}".to_string()),
                timestamp: None,
            },
            UndoEntry {
                command: Command::UpdateShot {
                    section_id: 1,
                    shot_id: 100,
                    updates: crate::models::ShotUpdates {
                        length: Some(10.0),
                        ..Default::default()
                    },
                },
                undo_data: UndoData::PreviousValues {
                    section_id: 1,
                    shot_id: Some(100),
                    values_json: r#"{"length":5.0}"#.to_string(),
                },
                timestamp: None,
            },
        ];

        for (i, original) in test_cases.iter().enumerate() {
            let serialized = UndoStack::serialize_entry(original);
            assert!(serialized.is_some(), "Failed to serialize test case {i}");

            let (cmd_json, undo_json) = serialized.unwrap();
            let deserialized = UndoStack::deserialize_entry(&cmd_json, &undo_json);
            assert!(
                deserialized.is_some(),
                "Failed to deserialize test case {i}"
            );
        }
    }

    #[test]
    fn test_undo_data_closure_info_serialization() {
        let closure = LoopClosure {
            id: 1,
            station_a: StationRef::new(1, 0).into(),
            station_b: StationRef::new(2, 3).into(),
            path_length: 45.5,
            misclosure_distance: 1.2,
            error_percentage: 2.6,
            horizontal_misclosure: 1.0,
            vertical_misclosure: 0.7,
            strategy: crate::models::ClosureStrategy::ProportionalByLength,
            is_aligned: false,
        };

        let entry = UndoEntry {
            command: Command::CreateLoopClosure {
                station_a: StationRef::new(1, 0),
                station_b: StationRef::new(2, 3),
                closure_strategy: crate::models::ClosureStrategy::ProportionalByLength,
            },
            undo_data: UndoData::ClosureInfo { closure },
            timestamp: None,
        };

        // Serialize
        let (cmd_json, undo_json) =
            UndoStack::serialize_entry(&entry).expect("ClosureInfo should serialize");

        // Deserialize
        let restored = UndoStack::deserialize_entry(&cmd_json, &undo_json)
            .expect("ClosureInfo should deserialize");

        // Verify roundtrip
        match &restored.undo_data {
            UndoData::ClosureInfo {
                closure: restored_closure,
            } => {
                // Pure serialization roundtrip (no project / no reanchor): the
                // transitional Index anchors survive verbatim, so compare keys directly.
                assert_eq!(restored_closure.station_a, StationRef::new(1, 0).into());
                assert_eq!(restored_closure.station_b, StationRef::new(2, 3).into());
                assert!((restored_closure.path_length - 45.5).abs() < 1e-6);
                assert!((restored_closure.misclosure_distance - 1.2).abs() < 1e-6);
                assert!((restored_closure.error_percentage - 2.6).abs() < 1e-6);
                assert!((restored_closure.horizontal_misclosure - 1.0).abs() < 1e-6);
                assert!((restored_closure.vertical_misclosure - 0.7).abs() < 1e-6);
            }
            other => panic!("Expected ClosureInfo, got {}", other.variant_name()),
        }

        match &restored.command {
            Command::CreateLoopClosure {
                station_a,
                station_b,
                ..
            } => {
                assert_eq!(*station_a, StationRef::new(1, 0));
                assert_eq!(*station_b, StationRef::new(2, 3));
            }
            _ => panic!("Expected CreateLoopClosure command"),
        }
    }
}
