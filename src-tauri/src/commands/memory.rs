//! Memory-probe IPC.
//!
//! Returns RSS / VM stats for the backend process and for every WebKit child
//! process spawned by this app (WebKitWebProcess, WebKitNetworkProcess,
//! WebKitGPUProcess on Linux). On non-Linux platforms returns the backend
//! stats only.
//!
//! Used by the in-app debug memory panel to diagnose WebProcess bloat.

use crate::core::CommandError;

// The payload types live in the models crate, whose ts-rs export tests write
// into `src-tauri/bindings/` (#576 RV5-RL-1); re-exported under the old path
// for `gen_ipc_types` and the probes below.
pub use crate::models::process_memory::{MemoryStats, ProcessMemory};

#[tauri::command]
pub async fn get_memory_stats() -> Result<MemoryStats, CommandError> {
    crate::commands::pool::run_stateless(|| {
        #[cfg(target_os = "linux")]
        {
            linux::collect().map_err(|e| CommandError::failed(e.to_string()))
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
    })
    .await
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

        // Read our own euid from /proc to avoid a libc dependency.
        let our_uid = read_name_ppid_uid(our_pid)
            .map(|(_, _, u)| u)
            .unwrap_or(u32::MAX);
        let table: Vec<ProcEntry> = fs::read_dir("/proc")?
            .filter_map(|r| r.ok())
            .filter_map(|d| {
                let name = d.file_name();
                let s = name.to_string_lossy();
                s.parse::<u32>().ok()
            })
            .filter_map(|pid| {
                let (name, ppid, uid) = read_name_ppid_uid(pid).ok()?;
                Some(ProcEntry {
                    pid,
                    name,
                    ppid,
                    uid,
                })
            })
            .collect();

        let mut webkit = Vec::new();
        for (pid, name) in select_webkit(our_pid, our_uid, &table) {
            let role = classify_webkit(&name);
            if let Ok(mut pm) = read_process(pid, role) {
                pm.name = name;
                webkit.push(pm);
            }
        }

        let total_rss_kb = backend.rss_kb + webkit.iter().map(|p| p.rss_kb).sum::<u64>();
        Ok(MemoryStats {
            backend,
            webkit_processes: webkit,
            total_rss_kb,
            webkit_enumeration_supported: true,
        })
    }

    /// One `/proc/<pid>/status` row: what [`select_webkit`] filters on.
    pub(super) struct ProcEntry {
        pub(super) pid: u32,
        pub(super) name: String,
        pub(super) ppid: u32,
        pub(super) uid: u32,
    }

    /// The child-process filter, over a process table read from /proc: the
    /// `(pid, name)` of every process to report, sorted by pid.
    ///
    /// Our own process is never reported. A process of another uid is never
    /// reported. A same-uid process is reported when its name starts with
    /// `WebKit` - a child or grandchild of ours (chased through the PPid
    /// links, up to 3 passes so a grandchild listed before its parent is still
    /// reached), and, as the fallback for sandboxed setups where WebKit's
    /// processes are reparented, any same-uid `WebKit*` process.
    pub(super) fn select_webkit(
        our_pid: u32,
        our_uid: u32,
        table: &[ProcEntry],
    ) -> Vec<(u32, String)> {
        let mut descendants: std::collections::HashSet<u32> = std::collections::HashSet::new();
        descendants.insert(our_pid);
        let mut selected = Vec::new();

        // We do up to 3 passes so grandchildren are picked up even if the
        // directory listing order is parent-after-child.
        for _ in 0..3 {
            for entry in table {
                if descendants.contains(&entry.pid) {
                    continue;
                }
                if entry.uid != our_uid {
                    continue;
                }
                if descendants.contains(&entry.ppid) || entry.name.starts_with("WebKit") {
                    descendants.insert(entry.pid);
                    if entry.pid == our_pid {
                        continue;
                    }
                    if entry.name.starts_with("WebKit") {
                        selected.push((entry.pid, entry.name.clone()));
                    }
                }
            }
        }

        // De-dupe by pid (multi-pass could insert twice).
        selected.sort_by_key(|(pid, _)| *pid);
        selected.dedup_by_key(|(pid, _)| *pid);
        selected
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
        let stats = tauri::async_runtime::block_on(get_memory_stats()).unwrap();
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

    fn entry(pid: u32, name: &str, ppid: u32, uid: u32) -> linux::ProcEntry {
        linux::ProcEntry {
            pid,
            name: name.to_string(),
            ppid,
            uid,
        }
    }

    /// The filter over a synthetic process table, so the verdict does not
    /// depend on what else runs on the host (#576 W25-NF3: the live version
    /// of this test failed wherever the same user ran a WebKit process, e.g.
    /// a WebKitWebDriver of the agent harness).
    #[test]
    fn does_not_include_unrelated_processes() {
        let (us, uid) = (100, 1000);
        let table = [
            // Listed before its parent: only a later pass reaches it.
            entry(104, "WebKitGPUProcess", 103, uid),
            entry(us, "stalagmight", 1, uid),
            entry(101, "WebKitWebProcess", us, uid),
            entry(102, "WebKitNetworkProcess", us, uid),
            // Our non-WebKit child: chased for its children, never reported.
            entry(103, "bash", us, uid),
            // Another user's WebKit process.
            entry(200, "WebKitWebProcess", 1, uid + 1),
            // An unrelated process of ours that is not WebKit.
            entry(201, "firefox", 1, uid),
            // Another user's child of ours (a setuid helper).
            entry(202, "WebKitWebProcess", us, uid + 1),
        ];
        assert_eq!(
            linux::select_webkit(us, uid, &table),
            vec![
                (101, "WebKitWebProcess".to_string()),
                (102, "WebKitNetworkProcess".to_string()),
                (104, "WebKitGPUProcess".to_string()),
            ]
        );
    }

    /// The documented fallback for sandboxed setups: a same-uid `WebKit*`
    /// process counts although no PPid chain leads to us. This is why the
    /// filter cannot be tested against the host's live process list.
    #[test]
    fn reparented_same_user_webkit_process_is_reported() {
        let table = [
            entry(100, "stalagmight", 1, 1000),
            entry(300, "WebKitWebDriver", 1, 1000),
        ];
        assert_eq!(
            linux::select_webkit(100, 1000, &table),
            vec![(300, "WebKitWebDriver".to_string())]
        );
    }

    /// Kills the spawned test processes however the test ends.
    struct Reap(Vec<u32>);

    impl Drop for Reap {
        fn drop(&mut self) {
            for pid in &self.0 {
                let _ = std::process::Command::new("kill")
                    .args(["-9", &pid.to_string()])
                    .status();
            }
        }
    }

    fn proc_status_field(pid: u32, field: &str) -> Option<String> {
        let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
        status
            .lines()
            .find_map(|line| line.strip_prefix(field).map(|v| v.trim().to_string()))
    }

    /// The live path over a process tree this test spawns itself: a child
    /// that names itself `WebKitW33Child` and its non-WebKit `sleep` child.
    /// It asserts on those two pids only, so other processes on the host
    /// cannot change the verdict.
    #[test]
    fn reports_the_webkit_child_it_spawned_and_not_its_plain_grandchild() {
        // `printf` is a shell builtin, so `/proc/self/comm` is the shell's
        // own; `sleep 30 & wait` keeps the shell alive with a child.
        let mut child = std::process::Command::new("sh")
            .args([
                "-c",
                "printf WebKitW33Child > /proc/self/comm; sleep 30 & wait",
            ])
            .spawn()
            .unwrap();
        let shell = child.id();
        let mut reap = Reap(vec![shell]);

        let mut grandchild = None;
        for _ in 0..200 {
            let renamed = proc_status_field(shell, "Name:").as_deref() == Some("WebKitW33Child");
            grandchild = std::fs::read_dir("/proc")
                .unwrap()
                .filter_map(|d| d.ok()?.file_name().to_str()?.parse::<u32>().ok())
                .find(|pid| {
                    proc_status_field(*pid, "PPid:").as_deref() == Some(shell.to_string().as_str())
                });
            if renamed && grandchild.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            proc_status_field(shell, "Name:").as_deref(),
            Some("WebKitW33Child"),
            "the spawned shell never renamed itself"
        );
        let grandchild = grandchild.unwrap();
        reap.0.push(grandchild);

        let stats = tauri::async_runtime::block_on(get_memory_stats()).unwrap();
        let reported = |pid: u32| stats.webkit_processes.iter().find(|p| p.pid == pid);
        let ours = reported(shell).unwrap();
        assert_eq!(ours.name, "WebKitW33Child");
        assert_eq!(ours.role, "webkit-web");
        assert!(
            reported(grandchild).is_none(),
            "the non-WebKit grandchild {grandchild} was reported: {:?}",
            stats.webkit_processes
        );
        assert!(reported(std::process::id()).is_none());
        assert_eq!(
            stats.total_rss_kb,
            stats.backend.rss_kb + stats.webkit_processes.iter().map(|p| p.rss_kb).sum::<u64>()
        );

        drop(reap);
        let _ = child.wait();
    }
}
