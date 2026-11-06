//! [#576 FZ-PERSIST B] The historical DDL the old-schema fuzzer rebuilds files from.
//!
//! Each statement constant is ONE `CREATE` statement exactly as an `init_schema`
//! wrote it, byte for byte (the text SQLite keeps in `sqlite_master.sql`),
//! extracted from the commit named in its comment with
//! `git show <sha>:src-tauri/src/persistence/sqlite.rs` (from v8's last build on,
//! `.../sqlite/schema.rs`). A statement several versions shared is kept once.
//! [`ERAS`] lists, per version, the statements its builds ran, in their order;
//! where a version's DDL changed during its lifetime, its first and its last
//! build are separate eras, and versions that shared a DDL share its list. Never
//! edit these to make a test pass: they are what files in the wild hold.

use super::Era;

/// `project`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const PROJECT_V2: &str = r"CREATE TABLE IF NOT EXISTS project (
                id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                location TEXT,
                created_at TEXT NOT NULL,
                modified_at TEXT NOT NULL,
                schema_version INTEGER NOT NULL,
                next_closure_id INTEGER NOT NULL DEFAULT 1
            );";
/// `sections`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v14 (293ffdaf9).
const SECTIONS_V2: &str = r"CREATE TABLE IF NOT EXISTS sections (
                id INTEGER PRIMARY KEY,
                name TEXT,
                surveyor TEXT,
                date TEXT,
                color TEXT NOT NULL,
                direction TEXT NOT NULL DEFAULT 'in',
                quality REAL NOT NULL DEFAULT 1.0,
                created_at TEXT NOT NULL
            );";
/// `shots`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v3 (2919b2c43).
const SHOTS_V2: &str = r"CREATE TABLE IF NOT EXISTS shots (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                azimuth_in REAL NOT NULL,
                azimuth_out REAL NOT NULL,
                length REAL NOT NULL,
                depth_in REAL NOT NULL,
                depth_out REAL NOT NULL,
                pitch_in REAL,
                pitch_out REAL,
                left_distance REAL,
                right_distance REAL,
                up_distance REAL,
                down_distance REAL,
                comment TEXT,
                is_virtual INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
                UNIQUE(section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `station_merges`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const STATION_MERGES_V2: &str = r"CREATE TABLE IF NOT EXISTS station_merges (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                merged_at TEXT NOT NULL,
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `zero_datums`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const ZERO_DATUMS_V2: &str = r"CREATE TABLE IF NOT EXISTS zero_datums (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                latitude REAL NOT NULL,
                longitude REAL NOT NULL,
                depth_reference REAL NOT NULL,
                UNIQUE(section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `section_offsets`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const SECTION_OFFSETS_V2: &str = r"CREATE TABLE IF NOT EXISTS section_offsets (
                section_id INTEGER PRIMARY KEY,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `station_offsets`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const STATION_OFFSETS_V2: &str = r"CREATE TABLE IF NOT EXISTS station_offsets (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                offset_x REAL NOT NULL,
                offset_y REAL NOT NULL,
                offset_z REAL NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `undo_stack`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v2 (ae5641a9a).
const UNDO_STACK_V2: &str = r"CREATE TABLE IF NOT EXISTS undo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json TEXT NOT NULL,
                undo_data_json TEXT NOT NULL,
                executed_at TEXT NOT NULL
            );";
/// `redo_stack`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v2 (ae5641a9a).
const REDO_STACK_V2: &str = r"CREATE TABLE IF NOT EXISTS redo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json TEXT NOT NULL,
                undo_data_json TEXT NOT NULL,
                executed_at TEXT NOT NULL
            );";
/// `loop_closures`, as `init_schema` wrote it from v2 (ce4bd1a8a) only.
const LOOP_CLOSURES_V2: &str = r"CREATE TABLE IF NOT EXISTS loop_closures (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                path_length REAL NOT NULL,
                misclosure_distance REAL NOT NULL,
                error_percentage REAL NOT NULL,
                horizontal_misclosure REAL NOT NULL,
                vertical_misclosure REAL NOT NULL,
                closure_id INTEGER NOT NULL DEFAULT 0,
                strategy TEXT NOT NULL DEFAULT 'ProportionalByLength',
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `display_settings`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const DISPLAY_SETTINGS_V2: &str = r"CREATE TABLE IF NOT EXISTS display_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );";
/// `resolved_warnings`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v7 (e728a22bf).
const RESOLVED_WARNINGS_V2: &str = r"CREATE TABLE IF NOT EXISTS resolved_warnings (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                shot_id INTEGER NOT NULL,
                warning_code TEXT NOT NULL,
                resolved_at TEXT NOT NULL,
                resolved_comment TEXT,
                UNIQUE(section_id, shot_id, warning_code),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `imported_file_hashes`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const IMPORTED_FILE_HASHES_V2: &str = r"CREATE TABLE IF NOT EXISTS imported_file_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            );";
/// `idx_shots_section`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const IDX_SHOTS_SECTION_V2: &str =
    r"CREATE INDEX IF NOT EXISTS idx_shots_section ON shots(section_id);";
/// `idx_undo_sequence`, as `init_schema` wrote it from v2 (ce4bd1a8a) to v17 (a9b8a2d6f).
const IDX_UNDO_SEQUENCE_V2: &str =
    r"CREATE INDEX IF NOT EXISTS idx_undo_sequence ON undo_stack(sequence);";
/// `loop_closures`, as `init_schema` wrote it from v2 (ae5641a9a) to v17 (a9b8a2d6f).
const LOOP_CLOSURES_V2_LATE: &str = r"CREATE TABLE IF NOT EXISTS loop_closures (
                id INTEGER PRIMARY KEY,
                station_a_section INTEGER NOT NULL,
                station_a_index INTEGER NOT NULL,
                station_b_section INTEGER NOT NULL,
                station_b_index INTEGER NOT NULL,
                path_length REAL NOT NULL,
                misclosure_distance REAL NOT NULL,
                error_percentage REAL NOT NULL,
                horizontal_misclosure REAL NOT NULL,
                vertical_misclosure REAL NOT NULL,
                closure_id INTEGER NOT NULL DEFAULT 0,
                strategy TEXT NOT NULL DEFAULT 'Batch',
                is_aligned INTEGER NOT NULL DEFAULT 0,
                FOREIGN KEY (station_a_section) REFERENCES sections(id) ON DELETE CASCADE,
                FOREIGN KEY (station_b_section) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `undo_stack`, as `init_schema` wrote it from v3 (837483054) to v17 (a9b8a2d6f).
const UNDO_STACK_V3: &str = r"CREATE TABLE IF NOT EXISTS undo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json BLOB NOT NULL,
                undo_data_json BLOB NOT NULL,
                executed_at TEXT NOT NULL
            );";
/// `redo_stack`, as `init_schema` wrote it from v3 (837483054) to v17 (a9b8a2d6f).
const REDO_STACK_V3: &str = r"CREATE TABLE IF NOT EXISTS redo_stack (
                id INTEGER PRIMARY KEY,
                sequence INTEGER NOT NULL,
                command_json BLOB NOT NULL,
                undo_data_json BLOB NOT NULL,
                executed_at TEXT NOT NULL
            );";
/// `station_comments`, as `init_schema` wrote it from v3 (2919b2c43) to v17 (a9b8a2d6f).
const STATION_COMMENTS_V3_LATE: &str = r"CREATE TABLE IF NOT EXISTS station_comments (
                section_id INTEGER NOT NULL,
                station_index INTEGER NOT NULL,
                comment TEXT NOT NULL,
                PRIMARY KEY (section_id, station_index),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `shots`, as `init_schema` wrote it from v4 (45b524a7e) to v5 (83cb32e34).
const SHOTS_V4: &str = r"CREATE TABLE IF NOT EXISTS shots (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                azimuth_in REAL NOT NULL,
                azimuth_out REAL NOT NULL,
                length REAL NOT NULL,
                depth_in REAL NOT NULL,
                depth_out REAL NOT NULL,
                pitch_in REAL,
                pitch_out REAL,
                left_distance REAL,
                right_distance REAL,
                up_distance REAL,
                down_distance REAL,
                comment TEXT,
                is_reference INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
                UNIQUE(section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `imported_section_hashes`, as `init_schema` wrote it from v4 (a570fe532) to v17 (a9b8a2d6f).
const IMPORTED_SECTION_HASHES_V4_LATE: &str = r"CREATE TABLE IF NOT EXISTS imported_section_hashes (
                id INTEGER PRIMARY KEY,
                hash TEXT NOT NULL UNIQUE
            );";
/// `collab_state`, as `init_schema` wrote it from v5 (c77242881) to v14 (964a83280).
const COLLAB_STATE_V5: &str = r"CREATE TABLE IF NOT EXISTS collab_state (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                state_json TEXT NOT NULL
            );";
/// `shots`, as `init_schema` wrote it from v6 (424ef3b80) to v12 (9c57e9099).
const SHOTS_V6: &str = r"CREATE TABLE IF NOT EXISTS shots (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                azimuth_in REAL NOT NULL,
                azimuth_out REAL NOT NULL,
                length REAL NOT NULL,
                depth_in REAL NOT NULL,
                depth_out REAL NOT NULL,
                pitch_in REAL,
                pitch_out REAL,
                left_distance REAL,
                right_distance REAL,
                up_distance REAL,
                down_distance REAL,
                comment TEXT,
                is_reference INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
                uuid TEXT,
                UNIQUE(section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `resolved_warnings`, as `init_schema` wrote it from v8 (dab6a3d7b) to v17 (a9b8a2d6f).
const RESOLVED_WARNINGS_V8: &str = r"CREATE TABLE IF NOT EXISTS resolved_warnings (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                shot_id INTEGER NOT NULL,
                warning_code TEXT NOT NULL,
                resolved_at TEXT NOT NULL,
                resolved_comment TEXT,
                shot_uuid TEXT,
                UNIQUE(section_id, shot_id, warning_code),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `project_settings`, as `init_schema` wrote it from v8 (766ae32bd) to v17 (a9b8a2d6f).
const PROJECT_SETTINGS_V8_LATE: &str = r"CREATE TABLE IF NOT EXISTS project_settings (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );";
/// `speleo_link`, as `init_schema` wrote it from v9 (9e994ed97) to v9 (c914df9e2).
const SPELEO_LINK_V9: &str = r"CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0
            );";
/// `speleo_link`, as `init_schema` wrote it from v10 (0d05a04a0) to v10 (0c2c055e3).
const SPELEO_LINK_V10: &str = r"CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                last_edit_at TEXT
            );";
/// `speleo_link`, as `init_schema` wrote it from v11 (5145e09df) to v15 (21d27ed3b).
const SPELEO_LINK_V11: &str = r"CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                last_edit_at TEXT,
                last_verify_at TEXT
            );";
/// `shots`, as `init_schema` wrote it from v13 (938281f8e) to v17 (a9b8a2d6f).
const SHOTS_V13: &str = r"CREATE TABLE IF NOT EXISTS shots (
                id INTEGER PRIMARY KEY,
                section_id INTEGER NOT NULL,
                sequence_order INTEGER NOT NULL,
                azimuth_in REAL NOT NULL,
                azimuth_out REAL NOT NULL,
                length REAL NOT NULL,
                depth_in REAL NOT NULL,
                depth_out REAL NOT NULL,
                pitch_in REAL,
                pitch_out REAL,
                left_distance REAL,
                right_distance REAL,
                up_distance REAL,
                down_distance REAL,
                comment TEXT,
                kind INTEGER NOT NULL DEFAULT 0,
                marker_index INTEGER,
                temperature REAL,
                uuid TEXT,
                UNIQUE(section_id, sequence_order),
                FOREIGN KEY (section_id) REFERENCES sections(id) ON DELETE CASCADE
            );";
/// `sections`, as `init_schema` wrote it from v14 (964a83280) to v17 (a9b8a2d6f).
const SECTIONS_V14_LATE: &str = r"CREATE TABLE IF NOT EXISTS sections (
                id INTEGER PRIMARY KEY,
                name TEXT,
                surveyor TEXT,
                date TEXT,
                color TEXT NOT NULL,
                direction TEXT NOT NULL DEFAULT 'in',
                quality REAL NOT NULL DEFAULT 1.0,
                reverse_parity INTEGER NOT NULL DEFAULT 0,
                gauge_shot_uuid TEXT,
                gauge_neighbour_uuid TEXT,
                created_at TEXT NOT NULL
            );";
/// `speleo_link`, as `init_schema` wrote it from v16 (238ab7058) to v17 (a9b8a2d6f).
const SPELEO_LINK_V16: &str = r"CREATE TABLE IF NOT EXISTS speleo_link (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                endpoint TEXT NOT NULL,
                project_id TEXT NOT NULL,
                project_name TEXT NOT NULL,
                base_commit_sha TEXT,
                has_unpushed_edits INTEGER NOT NULL DEFAULT 0,
                last_edit_at TEXT,
                last_verify_at TEXT,
                undo_mode TEXT
            );";
/// `speleo_undo_chain`, as `init_schema` wrote it from v16 (238ab7058) to v17 (a9b8a2d6f).
const SPELEO_UNDO_CHAIN_V16: &str = r"CREATE TABLE IF NOT EXISTS speleo_undo_chain (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                unwound_from_sha TEXT NOT NULL
            );";
/// `speleo_undo_watermark`, as `init_schema` wrote it from v16 (994d70965) to v17 (a9b8a2d6f).
const SPELEO_UNDO_WATERMARK_V16_LATE: &str = r"CREATE TABLE IF NOT EXISTS speleo_undo_watermark (
                id INTEGER PRIMARY KEY CHECK (id = 1),
                inherited_through TEXT NOT NULL
            );";

/// `init_schema` of v2-first.
const DDL_V2: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V2,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    UNDO_STACK_V2,
    REDO_STACK_V2,
    LOOP_CLOSURES_V2,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v2-last.
const DDL_V2_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V2,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    UNDO_STACK_V2,
    REDO_STACK_V2,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v3-first.
const DDL_V3: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V2,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v3-last.
const DDL_V3_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V2,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v4-first.
const DDL_V4: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V4,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v4-last.
const DDL_V4_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V4,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v5-first, v5-last.
const DDL_V5: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V4,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v6-first, v6-last, v7-first, v7-last.
const DDL_V6: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V2,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v8-first.
const DDL_V8: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v8-last.
const DDL_V8_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v9-first, v9-last.
const DDL_V9: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    SPELEO_LINK_V9,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v10-first, v10-last.
const DDL_V10: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    SPELEO_LINK_V10,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v11-first, v11-last, v12-first, v12-last.
const DDL_V11: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V6,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    SPELEO_LINK_V11,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v13-first, v13-last, v14-first.
const DDL_V13: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V2,
    SHOTS_V13,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    SPELEO_LINK_V11,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v14-last.
const DDL_V14_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V14_LATE,
    SHOTS_V13,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    COLLAB_STATE_V5,
    SPELEO_LINK_V11,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v15-first, v15-last.
const DDL_V15: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V14_LATE,
    SHOTS_V13,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    SPELEO_LINK_V11,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v16-first.
const DDL_V16: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V14_LATE,
    SHOTS_V13,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    SPELEO_LINK_V16,
    SPELEO_UNDO_CHAIN_V16,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];
/// `init_schema` of v16-last, v17-first, v17-last.
const DDL_V16_LATE: &[&str] = &[
    PROJECT_V2,
    SECTIONS_V14_LATE,
    SHOTS_V13,
    STATION_MERGES_V2,
    ZERO_DATUMS_V2,
    SECTION_OFFSETS_V2,
    STATION_OFFSETS_V2,
    STATION_COMMENTS_V3_LATE,
    SPELEO_LINK_V16,
    SPELEO_UNDO_CHAIN_V16,
    SPELEO_UNDO_WATERMARK_V16_LATE,
    UNDO_STACK_V3,
    REDO_STACK_V3,
    LOOP_CLOSURES_V2_LATE,
    DISPLAY_SETTINGS_V2,
    PROJECT_SETTINGS_V8_LATE,
    RESOLVED_WARNINGS_V8,
    IMPORTED_FILE_HASHES_V2,
    IMPORTED_SECTION_HASHES_V4_LATE,
    IDX_SHOTS_SECTION_V2,
    IDX_UNDO_SEQUENCE_V2,
];

/// Every era: (schema version, which builds, the commits whose `init_schema` it is, DDL).
pub(super) const ERAS: &[Era] = &[
    Era {
        version: 2,
        builds: "first build",
        commits: "ce4bd1a8a",
        ddl: DDL_V2,
    },
    Era {
        version: 2,
        builds: "last build",
        commits: "ae5641a9a",
        ddl: DDL_V2_LATE,
    },
    Era {
        version: 3,
        builds: "first build",
        commits: "837483054",
        ddl: DDL_V3,
    },
    Era {
        version: 3,
        builds: "last build",
        commits: "2919b2c43",
        ddl: DDL_V3_LATE,
    },
    Era {
        version: 4,
        builds: "first build",
        commits: "45b524a7e",
        ddl: DDL_V4,
    },
    Era {
        version: 4,
        builds: "last build",
        commits: "a570fe532",
        ddl: DDL_V4_LATE,
    },
    Era {
        version: 5,
        builds: "every build",
        commits: "c77242881..83cb32e34",
        ddl: DDL_V5,
    },
    Era {
        version: 6,
        builds: "every build",
        commits: "424ef3b80..579faf72c",
        ddl: DDL_V6,
    },
    Era {
        version: 7,
        builds: "every build",
        commits: "dafb7ca8f..e728a22bf",
        ddl: DDL_V6,
    },
    Era {
        version: 8,
        builds: "first build",
        commits: "dab6a3d7b",
        ddl: DDL_V8,
    },
    Era {
        version: 8,
        builds: "last build",
        commits: "766ae32bd",
        ddl: DDL_V8_LATE,
    },
    Era {
        version: 9,
        builds: "every build",
        commits: "9e994ed97..c914df9e2",
        ddl: DDL_V9,
    },
    Era {
        version: 10,
        builds: "every build",
        commits: "0d05a04a0..0c2c055e3",
        ddl: DDL_V10,
    },
    Era {
        version: 11,
        builds: "every build",
        commits: "5145e09df..a757017a4",
        ddl: DDL_V11,
    },
    Era {
        version: 12,
        builds: "every build",
        commits: "9e2a868c7..9c57e9099",
        ddl: DDL_V11,
    },
    Era {
        version: 13,
        builds: "every build",
        commits: "938281f8e..2b4b74a85",
        ddl: DDL_V13,
    },
    Era {
        version: 14,
        builds: "first build",
        commits: "293ffdaf9",
        ddl: DDL_V13,
    },
    Era {
        version: 14,
        builds: "last build",
        commits: "964a83280",
        ddl: DDL_V14_LATE,
    },
    Era {
        version: 15,
        builds: "every build",
        commits: "7ff017569..21d27ed3b",
        ddl: DDL_V15,
    },
    Era {
        version: 16,
        builds: "first build",
        commits: "238ab7058",
        ddl: DDL_V16,
    },
    Era {
        version: 16,
        builds: "last build",
        commits: "994d70965",
        ddl: DDL_V16_LATE,
    },
    Era {
        version: 17,
        builds: "every build",
        commits: "814494187..a9b8a2d6f",
        ddl: DDL_V16_LATE,
    },
];
