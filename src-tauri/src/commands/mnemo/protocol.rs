//! The MNemo v2 serial command line, as far as Stalagmight uses it (#725).
//!
//! Written from the device's public Serial-CLI documentation. Commands are
//! `\n`-terminated text; every reply is raw bytes followed by the text
//! `MN2Over`. Only two commands are ever sent, both read-only:
//! `getfirmwareversion`, then `getdata` (the survey memory, in the same bytes a
//! `.dmp` file holds as text).
//!
//! Nothing here touches a real port: [`pull`] runs over any `Read + Write`, so
//! the tests drive it with a scripted fake device.

use std::io::{self, ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The device's serial speed. Never 1200: on the MNemo's RP2040 board, opening
/// the port at 1200 baud reboots it into its bootloader (firmware-update mode).
pub(super) const BAUD: u32 = 57_600;

/// Text the device sends after every reply.
const END_MARKER: &[u8] = b"MN2Over";

/// The largest `getdata` reply accepted. The device's memory is far smaller; the
/// cap bounds what a misbehaving device can make us buffer, and keeps the saved
/// text (about 5 characters per byte) under the DMP parser's 50 MB file limit.
pub(super) const MAX_REPLY_BYTES: usize = 8 * 1024 * 1024;

/// The longest firmware string kept. It is shown in the UI and logged.
const MAX_FIRMWARE_CHARS: usize = 64;

/// The two commands this module can send. Private on purpose: no caller, and
/// so no IPC argument, can put any other text on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Command {
    FirmwareVersion,
    GetData,
}

impl Command {
    fn line(self) -> &'static [u8] {
        match self {
            Self::FirmwareVersion => b"getfirmwareversion\n",
            Self::GetData => b"getdata\n",
        }
    }
}

/// How long to wait for the device at each stage of one reply.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReplyLimits {
    /// No byte at all within this long: the device is not answering.
    pub(super) first_byte: Duration,
    /// Bytes stopped arriving for this long before the end marker.
    pub(super) idle: Duration,
    /// The whole reply took longer than this.
    pub(super) total: Duration,
}

/// Timing for a whole pull.
#[derive(Debug, Clone, Copy)]
pub(super) struct PullLimits {
    pub(super) firmware: ReplyLimits,
    pub(super) data: ReplyLimits,
    /// After the end marker, wait this long for more bytes before accepting the
    /// reply. Survey bytes can contain `MN2Over` by chance; when data follows
    /// such a match (even after a pause) it stops counting as the end, see
    /// [`reply_end`].
    pub(super) settle: Duration,
    /// Before the first command, read and discard whatever the device is still
    /// sending (the rest of a cancelled transfer) until it has been quiet this long.
    pub(super) drain_quiet: Duration,
    /// Give up draining after this long and send the command anyway.
    pub(super) drain_max: Duration,
}

impl PullLimits {
    /// The timings used with a real device.
    pub(super) const DEVICE: Self = Self {
        firmware: ReplyLimits {
            first_byte: Duration::from_secs(10),
            idle: Duration::from_secs(2),
            total: Duration::from_secs(10),
        },
        data: ReplyLimits {
            first_byte: Duration::from_secs(10),
            // The device can pause for seconds while it reads its memory.
            idle: Duration::from_secs(8),
            total: Duration::from_mins(10),
        },
        settle: Duration::from_millis(250),
        drain_quiet: Duration::from_millis(250),
        drain_max: Duration::from_secs(3),
    };
}

/// Why a pull stopped.
#[derive(Debug)]
pub(super) enum PullError {
    /// Nothing came back to `getfirmwareversion`: off, asleep, or not an MNemo.
    NoAnswer,
    /// The firmware reply never ended with `MN2Over`, so this is not an MNemo
    /// (or not one we understand) and `getdata` was never sent.
    NotMnemo,
    /// `getdata` started but the reply stopped before its end marker.
    Incomplete { received: usize },
    /// `getdata` sent more than [`MAX_REPLY_BYTES`].
    TooLarge,
    /// The user cancelled.
    Cancelled,
    /// The port itself failed (unplugged, I/O error).
    Io(io::Error),
}

impl From<io::Error> for PullError {
    fn from(e: io::Error) -> Self {
        Self::Io(e)
    }
}

/// What a successful pull read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Pulled {
    /// The firmware reply as text, trimmed, control characters removed.
    pub(super) firmware: String,
    /// The `getdata` reply without its end marker: the bytes of a `.dmp` file.
    pub(super) data: Vec<u8>,
}

/// Read the firmware version, then the survey memory.
///
/// `getdata` is only sent once the firmware reply has ended with the device's
/// end marker, so an unrelated serial device never receives it.
/// `on_progress` receives the number of `getdata` bytes read so far.
pub(super) fn pull<P: Read + Write>(
    port: &mut P,
    limits: &PullLimits,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(usize),
) -> Result<Pulled, PullError> {
    drain(port, limits, cancel)?;
    let firmware = exchange(
        port,
        Command::FirmwareVersion,
        &limits.firmware,
        limits.settle,
        cancel,
        &mut |_| {},
    )
    .map_err(|e| match e {
        PullError::Incomplete { .. } | PullError::TooLarge => PullError::NotMnemo,
        other => other,
    })?;

    let data = exchange(
        port,
        Command::GetData,
        &limits.data,
        limits.settle,
        cancel,
        on_progress,
    )
    .map_err(|e| match e {
        PullError::NoAnswer => PullError::Incomplete { received: 0 },
        other => other,
    })?;

    Ok(Pulled {
        firmware: firmware_text(&firmware),
        data,
    })
}

/// Read one chunk, treating a read timeout as "nothing yet".
fn read_some<P: Read>(port: &mut P, chunk: &mut [u8]) -> Result<usize, PullError> {
    match port.read(chunk) {
        Ok(n) => Ok(n),
        Err(e)
            if matches!(
                e.kind(),
                ErrorKind::TimedOut | ErrorKind::WouldBlock | ErrorKind::Interrupted
            ) =>
        {
            Ok(0)
        }
        Err(e) => Err(e.into()),
    }
}

/// Discard bytes until the port has been quiet for `drain_quiet` (or
/// `drain_max` has passed), so a transfer cut short by a cancel or a dropped
/// cable cannot be read as the reply to the next command.
fn drain<P: Read>(port: &mut P, limits: &PullLimits, cancel: &AtomicBool) -> Result<(), PullError> {
    let start = Instant::now();
    let mut quiet_since = start;
    let mut chunk = vec![0u8; 4 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(PullError::Cancelled);
        }
        let now = Instant::now();
        if now.duration_since(quiet_since) >= limits.drain_quiet
            || now.duration_since(start) >= limits.drain_max
        {
            return Ok(());
        }
        if read_some(port, &mut chunk)? > 0 {
            quiet_since = Instant::now();
        } else {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Send one command and read its reply up to the end marker.
fn exchange<P: Read + Write>(
    port: &mut P,
    command: Command,
    limits: &ReplyLimits,
    settle: Duration,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(usize),
) -> Result<Vec<u8>, PullError> {
    port.write_all(command.line())?;
    port.flush()?;

    let cap = match command {
        Command::FirmwareVersion => 4 * 1024,
        Command::GetData => MAX_REPLY_BYTES,
    } + END_MARKER.len();

    let start = Instant::now();
    let mut last_byte_at = start;
    let mut reply: Vec<u8> = Vec::new();
    // Where the latest `MN2Over` in `reply` begins, once one has arrived.
    let mut marker_at: Option<usize> = None;
    let mut chunk = vec![0u8; 16 * 1024];

    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(PullError::Cancelled);
        }
        let now = Instant::now();
        if let Some(end) = marker_at {
            if now.duration_since(last_byte_at) >= settle {
                reply.truncate(end);
                return Ok(reply);
            }
        } else if reply.is_empty() {
            if now.duration_since(start) >= limits.first_byte {
                return Err(PullError::NoAnswer);
            }
        } else if now.duration_since(last_byte_at) >= limits.idle {
            return Err(PullError::Incomplete {
                received: reply.len(),
            });
        }
        if now.duration_since(start) >= limits.total {
            return Err(PullError::Incomplete {
                received: reply.len(),
            });
        }

        let read = read_some(port, &mut chunk)?;
        if read == 0 {
            // A port read normally waits for its own timeout; this keeps a port
            // that returns at once from spinning the CPU.
            std::thread::sleep(Duration::from_millis(5));
            continue;
        }

        reply.extend_from_slice(&chunk[..read]);
        last_byte_at = Instant::now();
        if reply.len() > cap {
            return Err(PullError::TooLarge);
        }
        // Re-derived from the whole tail on every read, so a chance match inside
        // the data stops counting as soon as more data follows it.
        marker_at = reply_end(&reply);
        on_progress(marker_at.unwrap_or(reply.len()));
    }
}

/// The most bytes after `MN2Over` that can still end a reply: line endings, or
/// a short prompt the device might print after it.
const MAX_AFTER_MARKER: usize = 16;

/// Where the reply's data ends: the start of an `MN2Over` near the end of
/// `reply` that is followed only by text-like bytes (printable ASCII,
/// whitespace, NUL padding). Survey data after a chance match is binary and
/// usually long, so it fails this test and the reply keeps going.
fn reply_end(reply: &[u8]) -> Option<usize> {
    let window_start = reply
        .len()
        .saturating_sub(END_MARKER.len() + MAX_AFTER_MARKER);
    let found = reply[window_start..]
        .windows(END_MARKER.len())
        .rposition(|w| w == END_MARKER)?;
    let marker = window_start + found;
    let after = &reply[marker + END_MARKER.len()..];
    after
        .iter()
        .all(|&b| b == 0 || b.is_ascii_graphic() || b.is_ascii_whitespace())
        .then_some(marker)
}

/// The firmware reply as display text: Latin-1, control characters dropped,
/// trimmed, and capped, since it comes from a device and lands in the UI and logs.
fn firmware_text(reply: &[u8]) -> String {
    reply
        .iter()
        .map(|&b| char::from(b))
        .filter(|c| !c.is_control())
        .collect::<String>()
        .trim()
        .chars()
        .take(MAX_FIRMWARE_CHARS)
        .collect()
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    /// What the fake device does next when the host reads.
    enum Step {
        /// Hand back these bytes from one read.
        Bytes(Vec<u8>),
        /// Return a read timeout, as a real port does when nothing arrived.
        Quiet(Duration),
    }

    /// A scripted device. Each command written selects the script for its reply.
    struct FakeMnemo {
        replies: VecDeque<(Vec<u8>, Vec<Step>)>,
        current: VecDeque<Step>,
        written: Vec<u8>,
    }

    impl FakeMnemo {
        fn new(replies: Vec<(&[u8], Vec<Step>)>) -> Self {
            Self {
                replies: replies
                    .into_iter()
                    .map(|(cmd, steps)| (cmd.to_vec(), steps))
                    .collect(),
                current: VecDeque::new(),
                written: Vec::new(),
            }
        }

        fn commands(&self) -> String {
            String::from_utf8(self.written.clone()).unwrap()
        }
    }

    impl Write for FakeMnemo {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.written.extend_from_slice(buf);
            if let Some((expected, steps)) = self.replies.pop_front() {
                assert_eq!(buf, expected.as_slice(), "unexpected command");
                // Appended, not replaced: bytes the device was already sending
                // stay in front of the reply, as on a real line.
                self.current.extend(steps);
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Read for FakeMnemo {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            match self.current.pop_front() {
                Some(Step::Bytes(bytes)) => {
                    assert!(
                        bytes.len() <= buf.len(),
                        "script chunk larger than read buffer"
                    );
                    buf[..bytes.len()].copy_from_slice(&bytes);
                    Ok(bytes.len())
                }
                Some(Step::Quiet(d)) => {
                    std::thread::sleep(d);
                    Err(ErrorKind::TimedOut.into())
                }
                None => {
                    std::thread::sleep(Duration::from_millis(2));
                    Err(ErrorKind::TimedOut.into())
                }
            }
        }
    }

    // Margins wide enough that a test thread descheduled for a while (cargo runs
    // a crate's tests as threads of one process) does not trip a limit.
    const FAST: PullLimits = PullLimits {
        firmware: ReplyLimits {
            first_byte: Duration::from_millis(300),
            idle: Duration::from_millis(300),
            total: Duration::from_secs(3),
        },
        data: ReplyLimits {
            first_byte: Duration::from_millis(300),
            idle: Duration::from_millis(300),
            total: Duration::from_secs(3),
        },
        settle: Duration::from_millis(100),
        drain_quiet: Duration::from_millis(20),
        drain_max: Duration::from_millis(500),
    };

    const FW: &[u8] = b"getfirmwareversion\n";
    const DATA: &[u8] = b"getdata\n";

    fn with_marker(body: &[u8]) -> Vec<u8> {
        [body, END_MARKER].concat()
    }

    fn run(device: &mut FakeMnemo) -> Result<Pulled, PullError> {
        pull(device, &FAST, &AtomicBool::new(false), &mut |_| {})
    }

    #[test]
    fn the_serial_speed_and_commands_are_fixed() {
        assert_eq!(
            BAUD, 57_600,
            "1200 would reboot the device into its bootloader"
        );
        assert_eq!(Command::FirmwareVersion.line(), b"getfirmwareversion\n");
        assert_eq!(Command::GetData.line(), b"getdata\n");
    }

    #[test]
    fn a_pull_reads_firmware_then_data_and_strips_the_markers() {
        let survey = vec![5u8, 68, 89, 101, 200, 0, 7];
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b" v2.4.1\r\n"))]),
            (DATA, vec![Step::Bytes(with_marker(&survey))]),
        ]);
        let pulled = run(&mut device).unwrap();
        assert_eq!(pulled.firmware, "v2.4.1");
        assert_eq!(pulled.data, survey);
        assert_eq!(device.commands(), "getfirmwareversion\ngetdata\n");
    }

    #[test]
    fn the_end_marker_is_found_across_two_reads() {
        let mut device = FakeMnemo::new(vec![
            (
                FW,
                vec![
                    Step::Bytes(b"fw MN2".to_vec()),
                    Step::Bytes(b"Over".to_vec()),
                ],
            ),
            (
                DATA,
                vec![
                    Step::Bytes(vec![5, 1, 2, b'M', b'N']),
                    Step::Bytes(b"2Ov".to_vec()),
                    Step::Bytes(b"er\r\n".to_vec()),
                ],
            ),
        ]);
        let pulled = run(&mut device).unwrap();
        assert_eq!(pulled.firmware, "fw");
        assert_eq!(pulled.data, vec![5, 1, 2]);
    }

    #[test]
    fn marker_bytes_inside_the_data_do_not_end_the_reply_even_across_a_pause() {
        // A chance MN2Over followed by binary data, then the device pauses for
        // longer than `settle` (as it does while reading its memory), then the
        // rest and the real marker. Nothing may be cut at the chance match.
        let mut first = vec![5u8, 9];
        first.extend_from_slice(END_MARKER);
        first.extend_from_slice(&[1, 200]);
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (
                DATA,
                vec![
                    Step::Bytes(first.clone()),
                    Step::Quiet(FAST.settle * 2),
                    Step::Bytes(with_marker(&[3])),
                ],
            ),
        ]);
        let pulled = run(&mut device).unwrap();
        let mut expected = first;
        expected.push(3);
        assert_eq!(pulled.data, expected);
    }

    #[test]
    fn the_reply_may_end_with_line_endings_or_a_short_prompt_after_the_marker() {
        for tail in [&b"\r\n"[..], b"\r\n> ", b"\0\0"] {
            let mut data = with_marker(&[5, 1]);
            data.extend_from_slice(tail);
            let mut device = FakeMnemo::new(vec![
                (FW, vec![Step::Bytes(with_marker(b"fw"))]),
                (DATA, vec![Step::Bytes(data)]),
            ]);
            assert_eq!(run(&mut device).unwrap().data, vec![5, 1], "tail {tail:?}");
        }
    }

    #[test]
    fn bytes_left_over_from_an_earlier_transfer_are_drained_first() {
        // A cancelled pull leaves the device mid-reply, marker included.
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (DATA, vec![Step::Bytes(with_marker(&[5, 1]))]),
        ]);
        device.current = vec![Step::Bytes(with_marker(&[7, 7, 7]))].into();
        let pulled = run(&mut device).unwrap();
        assert_eq!(pulled.firmware, "fw");
        assert_eq!(pulled.data, vec![5, 1]);
    }

    #[test]
    fn a_silent_device_is_no_answer_and_never_gets_getdata() {
        let mut device = FakeMnemo::new(vec![(FW, vec![])]);
        assert!(matches!(run(&mut device), Err(PullError::NoAnswer)));
        assert_eq!(device.commands(), "getfirmwareversion\n");
    }

    #[test]
    fn a_device_that_never_sends_the_marker_is_not_an_mnemo_and_never_gets_getdata() {
        let mut device = FakeMnemo::new(vec![(FW, vec![Step::Bytes(b"AT OK\r\n".to_vec())])]);
        assert!(matches!(run(&mut device), Err(PullError::NotMnemo)));
        assert_eq!(device.commands(), "getfirmwareversion\n");
    }

    #[test]
    fn a_data_reply_that_stops_early_is_incomplete() {
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (DATA, vec![Step::Bytes(vec![5, 1, 2, 3])]),
        ]);
        match run(&mut device) {
            Err(PullError::Incomplete { received }) => assert_eq!(received, 4),
            other => panic!("expected Incomplete, got {other:?}"),
        }
    }

    #[test]
    fn a_data_reply_with_no_bytes_is_incomplete_with_nothing_received() {
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (DATA, vec![]),
        ]);
        assert!(matches!(
            run(&mut device),
            Err(PullError::Incomplete { received: 0 })
        ));
    }

    #[test]
    fn a_reply_over_the_cap_is_refused() {
        let chunk = vec![1u8; 16 * 1024];
        let steps = (0..=(MAX_REPLY_BYTES / chunk.len()))
            .map(|_| Step::Bytes(chunk.clone()))
            .collect();
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (DATA, steps),
        ]);
        let limits = PullLimits {
            data: ReplyLimits {
                total: Duration::from_secs(30),
                ..FAST.data
            },
            ..FAST
        };
        let result = pull(&mut device, &limits, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(result, Err(PullError::TooLarge)), "{result:?}");
    }

    #[test]
    fn cancel_stops_the_pull() {
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (
                DATA,
                vec![
                    Step::Bytes(vec![5, 1]),
                    Step::Quiet(Duration::from_millis(5)),
                ],
            ),
        ]);
        let cancel = AtomicBool::new(false);
        let result = pull(&mut device, &FAST, &cancel, &mut |received| {
            if received > 0 {
                cancel.store(true, Ordering::Relaxed);
            }
        });
        assert!(matches!(result, Err(PullError::Cancelled)), "{result:?}");
    }

    #[test]
    fn progress_counts_data_bytes_and_never_the_marker() {
        let mut device = FakeMnemo::new(vec![
            (FW, vec![Step::Bytes(with_marker(b"fw"))]),
            (
                DATA,
                vec![Step::Bytes(vec![5, 1, 2]), Step::Bytes(with_marker(&[3]))],
            ),
        ]);
        let mut seen = Vec::new();
        pull(&mut device, &FAST, &AtomicBool::new(false), &mut |n| {
            seen.push(n);
        })
        .unwrap();
        assert_eq!(seen, vec![3, 4]);
    }

    #[test]
    fn a_port_error_is_reported_as_io() {
        struct Unplugged;
        impl Read for Unplugged {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                Err(ErrorKind::BrokenPipe.into())
            }
        }
        impl Write for Unplugged {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                Ok(buf.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        let result = pull(&mut Unplugged, &FAST, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(result, Err(PullError::Io(_))), "{result:?}");
    }

    #[test]
    fn firmware_text_is_trimmed_cleaned_and_capped() {
        assert_eq!(firmware_text(b"\r\n  MNemo 2.4 \x07\x1b\r\n"), "MNemo 2.4");
        assert_eq!(firmware_text(&[b'x'; 500]).len(), MAX_FIRMWARE_CHARS);
    }
}
