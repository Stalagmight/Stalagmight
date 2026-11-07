//! The memory-probe IPC payload (`get_memory_stats`, the in-app debug memory
//! panel): RSS / VM stats for the backend process and its WebKit children.
//!
//! The probe itself lives in the app crate (`commands::memory`); the types live
//! here so their ts-rs export tests write into `src-tauri/bindings/` like every
//! other member crate's (#576 RV5-RL-1). Declared in the app crate, their
//! `export_to` resolved against `src-tauri/` and every `cargo test` wrote two
//! files outside the checkout. `pnpm gen:ipc-types` writes the frontend copies
//! in `src/types/ipc/`.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
    #[ts(type = "number")]
    pub rss_kb: u64,
    #[ts(type = "number")]
    pub vm_size_kb: u64,
    #[ts(type = "number")]
    pub vm_peak_kb: u64,
    #[ts(type = "number")]
    pub vm_rss_anon_kb: u64,
    #[ts(type = "number")]
    pub vm_rss_file_kb: u64,
    #[ts(type = "number")]
    pub vm_rss_shmem_kb: u64,
    /// Heuristic role: "backend" | "webkit-web" | "webkit-network" | "webkit-gpu" | "webkit-other".
    pub role: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export, export_to = "../../bindings/")]
pub struct MemoryStats {
    pub backend: ProcessMemory,
    pub webkit_processes: Vec<ProcessMemory>,
    #[ts(type = "number")]
    pub total_rss_kb: u64,
    /// True if WebKit children were discovered via /proc; false on platforms
    /// where we can't enumerate them (macOS, Windows). In that case the
    /// caller should fall back to whatever in-process introspection it has.
    pub webkit_enumeration_supported: bool,
}
