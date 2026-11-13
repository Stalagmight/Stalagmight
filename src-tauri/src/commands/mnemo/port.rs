//! Real serial ports for the MNemo pull (#725). Not built on Android or iOS.

use std::io;
use std::time::Duration;

use serialport::{
    ClearBuffer, DataBits, FlowControl, Parity, SerialPort, SerialPortType, StopBits,
};

use super::protocol::BAUD;

/// How long one read waits before returning a timeout. Short, so a cancel or a
/// stage timeout in the protocol loop is noticed promptly.
const READ_TIMEOUT: Duration = Duration::from_millis(200);

/// A USB serial port as the OS lists it.
#[derive(Debug, Clone)]
pub(super) struct UsbSerialPort {
    pub(super) name: String,
    pub(super) vid: u16,
    pub(super) pid: u16,
    pub(super) manufacturer: Option<String>,
    pub(super) product: Option<String>,
}

/// The USB serial ports present now. Other serial ports (built-in, Bluetooth,
/// PCI) are left out: the MNemo only ever appears as a USB device.
pub(super) fn usb_ports() -> Result<Vec<UsbSerialPort>, serialport::Error> {
    // Without libudev the crate scans /sys/class/tty and panics if it is
    // missing (some containers); no sysfs means no USB serial ports to offer.
    #[cfg(target_os = "linux")]
    if !std::path::Path::new("/sys/class/tty").is_dir() {
        return Ok(Vec::new());
    }

    Ok(serialport::available_ports()?
        .into_iter()
        .filter(|p| is_callout_device(&p.port_name))
        .filter_map(|p| match p.port_type {
            SerialPortType::UsbPort(usb) => Some(UsbSerialPort {
                name: p.port_name,
                vid: usb.vid,
                pid: usb.pid,
                manufacturer: usb.manufacturer,
                product: usb.product,
            }),
            _ => None,
        })
        .collect())
}

/// macOS lists each serial device twice. Opening `/dev/tty.*` waits for the
/// carrier-detect line, which a USB device may never raise; `/dev/cu.*` does not.
fn is_callout_device(name: &str) -> bool {
    !cfg!(target_os = "macos") || name.starts_with("/dev/cu.")
}

/// Why a port could not be opened.
#[derive(Debug)]
pub(super) enum OpenError {
    /// Another program holds the port (on Unix the crate reports the
    /// exclusive-open `EBUSY` as "no device"; on Windows an in-use COM port is
    /// "access denied").
    Busy,
    /// The OS refused access. On Linux this means the user is not in the
    /// group that owns the device (`dialout` or `uucp`).
    PermissionDenied,
    Other(serialport::Error),
}

/// Open `name` raw, 8N1, at [`BAUD`], exclusively, and drop anything already
/// buffered so the first bytes read belong to the first command's reply.
pub(super) fn open(name: &str) -> Result<Box<dyn SerialPort>, OpenError> {
    let port = serialport::new(name, BAUD)
        .data_bits(DataBits::Eight)
        .parity(Parity::None)
        .stop_bits(StopBits::One)
        .flow_control(FlowControl::None)
        .timeout(READ_TIMEOUT)
        .exclusive(true)
        .open()
        .map_err(classify_open_error)?;
    port.clear(ClearBuffer::All).map_err(OpenError::Other)?;
    Ok(port)
}

fn classify_open_error(e: serialport::Error) -> OpenError {
    match e.kind() {
        serialport::ErrorKind::NoDevice => OpenError::Busy,
        serialport::ErrorKind::Io(io::ErrorKind::PermissionDenied) => {
            if cfg!(windows) {
                OpenError::Busy
            } else {
                OpenError::PermissionDenied
            }
        }
        serialport::ErrorKind::Io(io::ErrorKind::WouldBlock | io::ErrorKind::ResourceBusy) => {
            OpenError::Busy
        }
        _ => OpenError::Other(e),
    }
}

#[cfg(all(test, unix))]
#[expect(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    //! The protocol over a real pseudo-terminal pair: the host end is the same
    //! `TTYPort` type a USB device opens as, so this exercises the crate's real
    //! read timeouts against the protocol loop.

    use super::super::protocol::{pull, PullError, PullLimits, ReplyLimits};
    use serialport::{SerialPort, TTYPort};
    use std::io::{Read, Write};
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    const LIMITS: PullLimits = PullLimits {
        firmware: ReplyLimits {
            first_byte: Duration::from_secs(2),
            idle: Duration::from_millis(500),
            total: Duration::from_secs(5),
        },
        data: ReplyLimits {
            first_byte: Duration::from_secs(2),
            idle: Duration::from_millis(500),
            total: Duration::from_secs(5),
        },
        settle: Duration::from_millis(50),
    };

    /// Read one `\n`-terminated command from the device end.
    fn read_command(device: &mut TTYPort) -> String {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        while line.last() != Some(&b'\n') {
            match device.read(&mut byte) {
                Ok(1) => line.push(byte[0]),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => panic!("device read failed: {e}"),
            }
        }
        String::from_utf8(line).unwrap()
    }

    #[test]
    fn a_pull_over_a_real_tty_reads_both_replies() {
        let (mut host, mut device) = TTYPort::pair().expect("pty pair");
        host.set_timeout(Duration::from_millis(50)).unwrap();
        device.set_timeout(Duration::from_millis(50)).unwrap();

        let survey: Vec<u8> = (0..=255).cycle().take(40_000).collect();
        let expected = survey.clone();
        let fake = std::thread::spawn(move || {
            assert_eq!(read_command(&mut device), "getfirmwareversion\n");
            device.write_all(b"MNemo test fw\r\nMN2Over").unwrap();
            assert_eq!(read_command(&mut device), "getdata\n");
            for chunk in survey.chunks(4096) {
                device.write_all(chunk).unwrap();
            }
            device.write_all(b"MN2Over").unwrap();
            // Keep the device end open until the host has read everything.
            std::thread::sleep(Duration::from_millis(500));
        });

        let pulled = pull(&mut host, &LIMITS, &AtomicBool::new(false), &mut |_| {}).unwrap();
        fake.join().unwrap();
        assert_eq!(pulled.firmware, "MNemo test fw");
        assert_eq!(pulled.data, expected);
    }

    #[test]
    fn a_silent_tty_times_out_as_no_answer() {
        let (mut host, _device) = TTYPort::pair().expect("pty pair");
        host.set_timeout(Duration::from_millis(50)).unwrap();
        let limits = PullLimits {
            firmware: ReplyLimits {
                first_byte: Duration::from_millis(300),
                ..LIMITS.firmware
            },
            ..LIMITS
        };
        let result = pull(&mut host, &limits, &AtomicBool::new(false), &mut |_| {});
        assert!(matches!(result, Err(PullError::NoAnswer)), "{result:?}");
    }
}
