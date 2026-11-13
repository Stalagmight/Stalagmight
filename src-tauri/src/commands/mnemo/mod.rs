//! Import from MNemo over USB (#725).
//!
//! Reads the survey memory of a USB-connected MNemo v2 and saves it as a
//! `.dmp` file. The frontend then opens that file through the normal import
//! route, so the read-only guard, the no-project path, duplicate-section
//! detection and the DMP wizard all apply unchanged. The file is written before
//! anything is imported, so the data never exists only in memory.

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod port;
#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod protocol;

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Serialize;
use tauri::{AppHandle, State};
use ts_rs::TS;

/// Event carrying [`MnemoPullProgress`] while `getdata` streams in.
pub const PULL_PROGRESS_EVENT: &str = "mnemo-pull-progress";

/// Folder, inside the app's local data directory, that keeps every dump.
const DUMPS_DIR: &str = "MNemo downloads";

/// One pull at a time, and the flag that cancels it.
#[derive(Default)]
pub struct MnemoPullState {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

/// Held by the thread doing the pull, so the slot frees only when the port is
/// closed, not when the IPC future is dropped (a webview reload mid-pull).
/// Clearing `cancel` at the END of a pull, never at the start, means a cancel
/// that reaches the backend just before its pull starts still stops it.
struct PullSlot {
    running: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
}

impl Drop for PullSlot {
    fn drop(&mut self) {
        self.cancel.store(false, Ordering::Release);
        self.running.store(false, Ordering::Release);
    }
}

/// A USB serial port that might be an MNemo.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MnemoPort {
    /// The OS name to pass back to [`pull_mnemo`]: `/dev/cu.usbmodem101`, `COM4`, `/dev/ttyACM0`.
    pub name: String,
    pub vid: u16,
    pub pid: u16,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    /// The USB description matches an MNemo. The pull still checks the device's
    /// reply before reading any data, so this only decides what to preselect.
    pub likely_mnemo: bool,
}

/// Bytes of the `getdata` reply read so far. The device does not announce a size.
#[derive(Debug, Clone, Copy, Serialize, TS)]
pub struct MnemoPullProgress {
    pub received: u32,
}

/// A finished pull. The dump is saved whether or not it parses.
#[derive(Debug, Clone, Serialize, TS)]
pub struct MnemoPullResult {
    /// Absolute path of the saved `.dmp`; open it to import.
    pub path: String,
    pub file_name: String,
    pub firmware: String,
    pub bytes: u32,
    pub sections: u32,
    pub shots: u32,
    /// Sections found that could not be read (for example a corrupted date).
    pub skipped_sections: u32,
    /// Bytes after the last complete section: usually a final section with no end marker.
    pub unread_trailing_bytes: u32,
    /// Why nothing in the dump could be read, when that is the case. The file is
    /// still saved, so it can be attached to a bug report.
    pub unreadable_reason: Option<String>,
}

/// Why a pull did not produce a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MnemoError {
    /// This build cannot reach serial ports (Android).
    Unsupported,
    /// The chosen port is no longer connected.
    PortNotFound,
    /// Another program (such as the vendor's tool) has the port open.
    PortBusy,
    /// The OS refused access; on Linux, the user is not in `dialout` or `uucp`.
    PermissionDenied,
    /// Nothing answered: the device is off, asleep, or not in its main menu.
    NoAnswer,
    /// Something answered, but not as an MNemo does. No data was requested.
    NotMnemo,
    /// The survey data stopped before its end.
    Incomplete {
        received: u32,
    },
    /// The device sent more than any MNemo memory holds.
    TooLarge,
    Cancelled,
    /// A pull is already running.
    AlreadyRunning,
    Failed {
        message: String,
    },
}

/// The USB serial ports present now, MNemo-looking ones first.
#[tauri::command]
pub async fn list_mnemo_ports() -> Result<Vec<MnemoPort>, MnemoError> {
    tauri::async_runtime::spawn_blocking(blocking::list_ports)
        .await
        .map_err(|e| MnemoError::Failed {
            message: e.to_string(),
        })?
}

/// Read the MNemo on `port` and save its memory as a `.dmp` file.
#[tauri::command]
pub async fn pull_mnemo(
    app: AppHandle,
    state: State<'_, MnemoPullState>,
    port: String,
) -> Result<MnemoPullResult, MnemoError> {
    if state
        .running
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(MnemoError::AlreadyRunning);
    }
    let slot = PullSlot {
        running: Arc::clone(&state.running),
        cancel: Arc::clone(&state.cancel),
    };

    tauri::async_runtime::spawn_blocking(move || {
        let slot = slot;
        blocking::pull_and_save(&app, &port, &slot.cancel)
    })
    .await
    .map_err(|e| MnemoError::Failed {
        message: e.to_string(),
    })?
}

/// Ask a pull to stop. A running pull stops within one port read (about
/// 200 ms); one about to start stops at its first check.
#[tauri::command]
pub fn cancel_mnemo_pull(state: State<'_, MnemoPullState>) {
    state.cancel.store(true, Ordering::Release);
}

/// Whether a USB description looks like an MNemo. Seen on a real MNemo v2: it
/// enumerates as its board, an Arduino "Nano RP2040 Connect" (vendor 0x2341,
/// product 0x005E), with no MNemo name. A bare board of that model matches
/// too; the firmware reply check in the pull is what tells them apart.
fn is_likely_mnemo(vid: u16, pid: u16, manufacturer: Option<&str>, product: Option<&str>) -> bool {
    const NANO_RP2040_CONNECT: (u16, u16) = (0x2341, 0x005E);
    let names_mnemo = [manufacturer, product]
        .into_iter()
        .flatten()
        .any(|s| s.to_ascii_lowercase().contains("mnemo"));
    names_mnemo || (vid, pid) == NANO_RP2040_CONNECT
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod blocking {
    //! The parts that touch the port; run on a blocking thread.

    use super::port::{self, OpenError};
    use super::protocol::{self, PullError, PullLimits};
    use super::{
        is_likely_mnemo, save_dump, MnemoError, MnemoPort, MnemoPullProgress, MnemoPullResult,
        PULL_PROGRESS_EVENT,
    };
    use crate::parsers::{parse_dmp_bytes, DmpReadLoss};
    use std::sync::atomic::AtomicBool;
    use tauri::{AppHandle, Emitter};

    /// Emit progress at most once per this many bytes; at 57600 baud a read
    /// returns a few dozen bytes, and the UI needs no event per read.
    const PROGRESS_STEP: usize = 2048;

    pub(super) fn list_ports() -> Result<Vec<MnemoPort>, MnemoError> {
        let mut ports: Vec<MnemoPort> = port::usb_ports()
            .map_err(|e| MnemoError::Failed {
                message: e.to_string(),
            })?
            .into_iter()
            .map(|p| MnemoPort {
                likely_mnemo: is_likely_mnemo(
                    p.vid,
                    p.pid,
                    p.manufacturer.as_deref(),
                    p.product.as_deref(),
                ),
                name: p.name,
                vid: p.vid,
                pid: p.pid,
                manufacturer: p.manufacturer,
                product: p.product,
            })
            .collect();
        ports.sort_by(|a, b| {
            b.likely_mnemo
                .cmp(&a.likely_mnemo)
                .then(a.name.cmp(&b.name))
        });
        Ok(ports)
    }

    pub(super) fn pull_and_save(
        app: &AppHandle,
        port_name: &str,
        cancel: &AtomicBool,
    ) -> Result<MnemoPullResult, MnemoError> {
        // Only a port the OS lists as USB serial right now can be opened, so the
        // IPC argument cannot point this at any other device or file.
        let listed = port::usb_ports().map_err(|e| MnemoError::Failed {
            message: e.to_string(),
        })?;
        if !listed.iter().any(|p| p.name == port_name) {
            return Err(MnemoError::PortNotFound);
        }

        let pulled = {
            let mut serial = port::open(port_name).map_err(|e| match e {
                OpenError::Busy => MnemoError::PortBusy,
                OpenError::Gone => MnemoError::PortNotFound,
                OpenError::PermissionDenied => MnemoError::PermissionDenied,
                OpenError::Other(e) => MnemoError::Failed {
                    message: e.to_string(),
                },
            })?;
            let mut emitted = 0usize;
            protocol::pull(&mut serial, &PullLimits::DEVICE, cancel, &mut |received| {
                if received >= emitted + PROGRESS_STEP {
                    emitted = received;
                    emit_progress(app, received);
                }
            })
            .map_err(pull_error)?
            // The port closes here, before any file work.
        };
        emit_progress(app, pulled.data.len());

        let dir = crate::app_dirs::data_local_dir()
            .filter(|d| d.is_absolute())
            .map(|d| d.join(super::DUMPS_DIR))
            .ok_or_else(|| MnemoError::Failed {
                message: "The app data folder is not available".into(),
            })?;
        let path =
            save_dump(&dir, chrono::Local::now().naive_local(), &pulled.data).map_err(|e| {
                MnemoError::Failed {
                    message: format!("Could not save the MNemo data: {e}"),
                }
            })?;

        let parsed = parse_dmp_bytes(&pulled.data);
        let (sections, shots, loss, unreadable_reason) = match &parsed {
            Ok((file, loss)) => (
                file.sections.len(),
                file.sections.iter().map(|s| s.shots.len()).sum::<usize>(),
                *loss,
                None,
            ),
            Err(e) => (0, 0, DmpReadLoss::default(), Some(e.to_string())),
        };

        tracing::info!(
            firmware = %pulled.firmware,
            bytes = pulled.data.len(),
            sections,
            skipped_sections = loss.skipped_sections,
            unread_trailing_bytes = loss.unread_trailing_bytes,
            readable = unreadable_reason.is_none(),
            purpose = "import from MNemo",
            "Saved MNemo memory"
        );

        Ok(MnemoPullResult {
            file_name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path: path.to_string_lossy().into_owned(),
            firmware: pulled.firmware,
            bytes: to_u32(pulled.data.len()),
            sections: to_u32(sections),
            shots: to_u32(shots),
            skipped_sections: to_u32(loss.skipped_sections),
            unread_trailing_bytes: to_u32(loss.unread_trailing_bytes),
            unreadable_reason,
        })
    }

    fn pull_error(e: PullError) -> MnemoError {
        tracing::warn!(error = ?e, purpose = "import from MNemo", "MNemo pull stopped");
        match e {
            PullError::NoAnswer => MnemoError::NoAnswer,
            PullError::NotMnemo => MnemoError::NotMnemo,
            PullError::Incomplete { received } => MnemoError::Incomplete {
                received: to_u32(received),
            },
            PullError::TooLarge => MnemoError::TooLarge,
            PullError::Cancelled => MnemoError::Cancelled,
            PullError::Io(e) => MnemoError::Failed {
                message: e.to_string(),
            },
        }
    }

    fn emit_progress(app: &AppHandle, received: usize) {
        let payload = MnemoPullProgress {
            received: to_u32(received),
        };
        if let Err(e) = app.emit(PULL_PROGRESS_EVENT, payload) {
            // A dropped progress tick is cosmetic; the pull carries on.
            tracing::warn!(error = %e, purpose = "report MNemo pull progress", "Emit failed");
        }
    }

    /// Every count here is bounded by `protocol::MAX_REPLY_BYTES` (8 MiB).
    fn to_u32(n: usize) -> u32 {
        u32::try_from(n).unwrap_or(u32::MAX)
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod blocking {
    use super::{MnemoError, MnemoPort, MnemoPullResult};
    use std::sync::atomic::AtomicBool;
    use tauri::AppHandle;

    pub(super) fn list_ports() -> Result<Vec<MnemoPort>, MnemoError> {
        Err(MnemoError::Unsupported)
    }

    pub(super) fn pull_and_save(
        _app: &AppHandle,
        _port_name: &str,
        _cancel: &AtomicBool,
    ) -> Result<MnemoPullResult, MnemoError> {
        Err(MnemoError::Unsupported)
    }
}

/// Write `data` in the `.dmp` text form to a new file in `dir`, named for
/// `when`. Never overwrites: a name already taken gets `-2`, `-3`, ...
///
/// The text is written and fsynced under a `.part` name first and only then
/// linked to its `.dmp` name, so a crash or power loss mid-write cannot leave a
/// truncated `.dmp` (a cut inside `-12;` would still parse, as a different
/// byte). `hard_link` fails rather than replace an existing name, which is what
/// keeps "never overwrites" true without a check-then-rename race.
fn save_dump(dir: &Path, when: chrono::NaiveDateTime, data: &[u8]) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let stem = format!("MNemo-{}", when.format("%Y-%m-%d_%H%M%S"));
    let part = dir.join(format!(".{stem}.part"));
    let written = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&part)
        .and_then(|file| write_and_sync(file, crate::parsers::to_dmp_text(data).as_bytes()));
    if let Err(e) = written {
        let _ = fs::remove_file(&part);
        return Err(e);
    }

    let claimed = claim_name(dir, &stem, &part);
    let _ = fs::remove_file(&part);
    claimed
}

/// Give the finished `part` file the first free `.dmp` name.
fn claim_name(dir: &Path, stem: &str, part: &Path) -> io::Result<PathBuf> {
    for n in 1..=100 {
        let name = if n == 1 {
            format!("{stem}.dmp")
        } else {
            format!("{stem}-{n}.dmp")
        };
        let path = dir.join(name);
        match fs::hard_link(part, &path) {
            Ok(()) => return Ok(path),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        format!("too many MNemo downloads named {stem} in {}", dir.display()),
    ))
}

fn write_and_sync(mut file: File, bytes: &[u8]) -> io::Result<()> {
    file.write_all(bytes)?;
    file.sync_all()
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn when() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 11, 13)
            .unwrap()
            .and_hms_opt(14, 30, 5)
            .unwrap()
    }

    #[test]
    fn a_dump_is_saved_as_dmp_text_named_for_the_time() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("nested").join(DUMPS_DIR);
        let path = save_dump(&target, when(), &[5, 200, 0]).unwrap();
        assert_eq!(path.file_name().unwrap(), "MNemo-2026-11-13_143005.dmp");
        assert_eq!(fs::read_to_string(&path).unwrap(), "5;-56;0;");
        let names: Vec<_> = fs::read_dir(&target)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(
            names,
            vec!["MNemo-2026-11-13_143005.dmp"],
            "no .part file left behind"
        );
    }

    #[test]
    fn a_dump_never_overwrites_an_earlier_one() {
        let dir = tempfile::tempdir().unwrap();
        let first = save_dump(dir.path(), when(), &[1]).unwrap();
        let second = save_dump(dir.path(), when(), &[2]).unwrap();
        let third = save_dump(dir.path(), when(), &[3]).unwrap();
        assert_eq!(second.file_name().unwrap(), "MNemo-2026-11-13_143005-2.dmp");
        assert_eq!(third.file_name().unwrap(), "MNemo-2026-11-13_143005-3.dmp");
        assert_eq!(fs::read_to_string(first).unwrap(), "1;");
        assert_eq!(fs::read_to_string(second).unwrap(), "2;");
    }

    #[test]
    fn mnemo_ports_are_recognised_by_name_or_vendor() {
        assert!(is_likely_mnemo(
            0x1234,
            0x0001,
            Some("Example Instruments"),
            Some("MNemo V2")
        ));
        // What a real MNemo v2 reports.
        assert!(is_likely_mnemo(
            0x2341,
            0x005E,
            Some("Arduino"),
            Some("Nano RP2040 Connect")
        ));
        // Another Arduino board, and a plain Raspberry Pi RP2040, are not.
        assert!(!is_likely_mnemo(
            0x2341,
            0x0043,
            Some("Arduino"),
            Some("Uno")
        ));
        assert!(!is_likely_mnemo(0x2E8A, 0x000A, None, None));
        assert!(!is_likely_mnemo(
            0x0403,
            0x6001,
            Some("FTDI"),
            Some("FT232R USB UART")
        ));
    }

    #[test]
    fn errors_serialize_with_a_kind_tag() {
        let json = serde_json::to_value(MnemoError::Incomplete { received: 7 }).unwrap();
        assert_eq!(
            json,
            serde_json::json!({"kind": "incomplete", "received": 7})
        );
        let json = serde_json::to_value(MnemoError::PortBusy).unwrap();
        assert_eq!(json, serde_json::json!({"kind": "port_busy"}));
    }

    /// Hardware check, run by hand with a real MNemo connected:
    /// `STALAGMIGHT_MNEMO_PORT=/dev/cu.usbmodem1301 STALAGMIGHT_MNEMO_OUT=<dir>
    ///  cargo test -p stalagmight --lib mnemo_hardware -- --ignored`
    /// Runs the real listing, open, pull and save, and leaves the `.dmp` in the
    /// out dir to compare with an independent read of the same device.
    #[cfg(not(any(target_os = "android", target_os = "ios")))]
    #[test]
    #[ignore = "needs a real MNemo; set STALAGMIGHT_MNEMO_PORT and STALAGMIGHT_MNEMO_OUT"]
    fn mnemo_hardware_pull_saves_a_readable_dump() {
        use std::sync::atomic::AtomicBool;
        let port_name = std::env::var("STALAGMIGHT_MNEMO_PORT").unwrap();
        let out = PathBuf::from(std::env::var("STALAGMIGHT_MNEMO_OUT").unwrap());

        let listed = port::usb_ports().unwrap();
        let entry = listed.iter().find(|p| p.name == port_name).unwrap();
        assert!(
            is_likely_mnemo(
                entry.vid,
                entry.pid,
                entry.manufacturer.as_deref(),
                entry.product.as_deref()
            ),
            "a real MNemo must be recognised: {entry:?}"
        );

        let mut serial = port::open(&port_name).unwrap();
        let pulled = protocol::pull(
            &mut serial,
            &protocol::PullLimits::DEVICE,
            &AtomicBool::new(false),
            &mut |_| {},
        )
        .unwrap();
        drop(serial);

        let path = save_dump(&out, chrono::Local::now().naive_local(), &pulled.data).unwrap();
        let (file, loss) = crate::parsers::parse_dmp_bytes(&pulled.data).unwrap();
        fs::write(
            out.join("summary.txt"),
            format!(
                "firmware={}\nbytes={}\nsections={}\nshots={}\nloss={loss:?}\nfile={}\n",
                pulled.firmware,
                pulled.data.len(),
                file.sections.len(),
                file.sections.iter().map(|s| s.shots.len()).sum::<usize>(),
                path.display()
            ),
        )
        .unwrap();
    }
}
