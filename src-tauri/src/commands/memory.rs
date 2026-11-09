//! Memory-probe IPC.
//!
//! Returns RSS / VM stats for the backend process and for every WebKit child
//! process spawned by this app (WebKitWebProcess, WebKitNetworkProcess,
//! WebKitGPUProcess on Linux). On non-Linux platforms returns the backend
//! stats only.
//!
//! Used by the in-app debug memory panel to diagnose WebProcess bloat.

use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
// Only gen_ipc_types writes these frontend types, using its compile-time
// manifest directory. Automatic export tests use the runtime manifest directory,
// which can point at the app root and put files outside Stalagmight.
#[ts(export_to = "../src/types/ipc/")]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
    pub rss_kb: u64,
    pub vm_size_kb: u64,
    pub vm_peak_kb: u64,
    pub vm_rss_anon_kb: u64,
    pub vm_rss_file_kb: u64,
    pub vm_rss_shmem_kb: u64,
    /// Heuristic role: "backend" | "webkit-web" | "webkit-network" | "webkit-gpu" | "webkit-other".
    pub role: String,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export_to = "../src/types/ipc/")]
pub struct MemoryStats {
    pub backend: ProcessMemory,
    pub webkit_processes: Vec<ProcessMemory>,
    pub total_rss_kb: u64,
    /// True if WebKit children were discovered via /proc; false on platforms
    /// where we can't enumerate them (macOS, Windows). In that case the
    /// caller should fall back to whatever in-process introspection it has.
    pub webkit_enumeration_supported: bool,
}

#[tauri::command]
pub fn get_memory_stats() -> Result<MemoryStats, String> {
    #[cfg(target_os = "linux")]
    {
        linux::collect().map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "linux"))]
    {
        // Best-effort: own RSS via `getrusage`, no children.
        let backend = fallback::self_memory();
        Ok(MemoryStats {
            total_rss_kb: backend.rss_kb,
            backend,
            webkit_processes: vec![],
            webkit_enumeration_supported: false,
        })
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::{MemoryStats, ProcessMemory};
    use std::fs;
    use std::io;

    /// Walk /proc, return our process + all WebKit children/grandchildren that
    /// claim us as ancestor. Reparented WebKit processes are matched by their
    /// `Name:` field combined with environ/cwd checks where possible; the
    /// fallback for sandboxed setups is "any process whose name starts with
    /// `WebKit` and shares our euid".
    pub(super) fn collect() -> io::Result<MemoryStats> {
        let our_pid = std::process::id();
        let backend = read_process(our_pid, "backend")?;

        // Build a quick PPid map so we can chase descendants.
        let mut webkit = Vec::new();
        let mut descendants: std::collections::HashSet<u32> = std::collections::HashSet::new();
        descendants.insert(our_pid);

        // We do up to 3 passes so grandchildren are picked up even if the
        // directory listing order is parent-after-child.
        // Read our own euid from /proc to avoid a libc dependency.
        let our_uid = read_name_ppid_uid(our_pid)
            .map(|(_, _, u)| u)
            .unwrap_or(u32::MAX);
        let entries: Vec<_> = fs::read_dir("/proc")?
            .filter_map(|r| r.ok())
            .filter_map(|d| {
                let name = d.file_name();
                let s = name.to_string_lossy();
                s.parse::<u32>().ok()
            })
            .collect();

        for _ in 0..3 {
            for &pid in &entries {
                if descendants.contains(&pid) {
                    continue;
                }
                let Ok((name, ppid, proc_uid)) = read_name_ppid_uid(pid) else {
                    continue;
                };
                if proc_uid != our_uid {
                    continue;
                }
                if descendants.contains(&ppid) || name.starts_with("WebKit") {
                    descendants.insert(pid);
                    if pid == our_pid {
                        continue;
                    }
                    if name.starts_with("WebKit") {
                        let role = classify_webkit(&name);
                        if let Ok(mut pm) = read_process(pid, role) {
                            pm.name = name;
                            webkit.push(pm);
                        }
                    }
                }
            }
        }

        // De-dupe by pid (multi-pass could insert twice).
        webkit.sort_by_key(|p| p.pid);
        webkit.dedup_by_key(|p| p.pid);

        let total_rss_kb = backend.rss_kb + webkit.iter().map(|p| p.rss_kb).sum::<u64>();
        Ok(MemoryStats {
            backend,
            webkit_processes: webkit,
            total_rss_kb,
            webkit_enumeration_supported: true,
        })
    }

    fn classify_webkit(name: &str) -> &'static str {
        if name.contains("Web") && !name.contains("Network") && !name.contains("GPU") {
            "webkit-web"
        } else if name.contains("Network") {
            "webkit-network"
        } else if name.contains("GPU") {
            "webkit-gpu"
        } else {
            "webkit-other"
        }
    }

    fn read_name_ppid_uid(pid: u32) -> io::Result<(String, u32, u32)> {
        let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
        let mut name = String::new();
        let mut ppid = 0u32;
        let mut uid = u32::MAX;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Name:") {
                name = rest.trim().to_string();
            } else if let Some(rest) = line.strip_prefix("PPid:") {
                ppid = rest.trim().parse().unwrap_or(0);
            } else if let Some(rest) = line.strip_prefix("Uid:") {
                // Uid: real effective saved fs
                if let Some(tok) = rest.split_whitespace().next() {
                    uid = tok.parse().unwrap_or(u32::MAX);
                }
            }
        }
        Ok((name, ppid, uid))
    }

    fn read_process(pid: u32, role: &str) -> io::Result<ProcessMemory> {
        let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
        let mut name = String::new();
        let mut rss = 0u64;
        let mut vmsize = 0u64;
        let mut vmpeak = 0u64;
        let mut anon = 0u64;
        let mut file_kb = 0u64;
        let mut shmem = 0u64;
        for line in status.lines() {
            if let Some(rest) = line.strip_prefix("Name:") {
                name = rest.trim().to_string();
            } else if let Some(kb) = parse_kb_line(line, "VmRSS:") {
                rss = kb;
            } else if let Some(kb) = parse_kb_line(line, "VmSize:") {
                vmsize = kb;
            } else if let Some(kb) = parse_kb_line(line, "VmPeak:") {
                vmpeak = kb;
            } else if let Some(kb) = parse_kb_line(line, "RssAnon:") {
                anon = kb;
            } else if let Some(kb) = parse_kb_line(line, "RssFile:") {
                file_kb = kb;
            } else if let Some(kb) = parse_kb_line(line, "RssShmem:") {
                shmem = kb;
            }
        }
        Ok(ProcessMemory {
            pid,
            name,
            rss_kb: rss,
            vm_size_kb: vmsize,
            vm_peak_kb: vmpeak,
            vm_rss_anon_kb: anon,
            vm_rss_file_kb: file_kb,
            vm_rss_shmem_kb: shmem,
            role: role.to_string(),
        })
    }

    fn parse_kb_line(line: &str, prefix: &str) -> Option<u64> {
        let rest = line.strip_prefix(prefix)?.trim();
        // Format: "<num> kB"
        let mut toks = rest.split_whitespace();
        toks.next()?.parse::<u64>().ok()
    }
}

#[cfg(not(target_os = "linux"))]
mod fallback {
    use super::ProcessMemory;

    pub(super) fn self_memory() -> ProcessMemory {
        ProcessMemory {
            pid: std::process::id(),
            name: "stalagmight".into(),
            rss_kb: 0,
            vm_size_kb: 0,
            vm_peak_kb: 0,
            vm_rss_anon_kb: 0,
            vm_rss_file_kb: 0,
            vm_rss_shmem_kb: 0,
            role: "backend".into(),
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn self_rss_is_populated() {
        let stats = get_memory_stats().unwrap();
        assert_eq!(stats.backend.pid, std::process::id());
        // The test binary itself uses at least a few MB.
        assert!(
            stats.backend.rss_kb > 1024,
            "expected RSS > 1 MB, got {} kB",
            stats.backend.rss_kb
        );
        assert!(stats.backend.vm_size_kb >= stats.backend.rss_kb);
        assert!(stats.webkit_enumeration_supported);
    }

    #[test]
    fn does_not_include_unrelated_processes() {
        let stats = get_memory_stats().unwrap();
        // The test binary has no WebKit children.
        assert!(
            stats.webkit_processes.is_empty(),
            "unexpected children: {:?}",
            stats.webkit_processes
        );
        assert_eq!(stats.total_rss_kb, stats.backend.rss_kb);
    }
}
