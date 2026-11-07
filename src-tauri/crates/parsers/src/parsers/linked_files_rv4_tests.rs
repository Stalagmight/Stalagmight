//! #576 RV4-R-P4: Win32 treats a reserved device name followed by `:`
//! (`COM1:`, `CON:`) as the device, and `nul:x` (the alternate-data-stream
//! form) too. Relative links and a Walls `.NAME` already refuse any `:`, so
//! only an ABSOLUTE link - a Compass `#file C:\x\COM1:`, an absolute Walls
//! `.PATH` component - could carry one to `exists()` on Windows. The check is
//! made from the text alone, so it holds on every platform.

use std::path::Path;

use super::linked_files::{is_dos_device_name, resolve_link};

#[test]
fn a_device_name_followed_by_a_colon_is_a_device() {
    let names = [
        "COM1:",
        "con:",
        "CON:",
        "nul:x",
        "LPT1:stream",
        "PRN.:",
        "aux .dat:x",
    ];
    let seen: Vec<_> = names.iter().map(|n| (*n, is_dos_device_name(n))).collect();
    for (name, device) in &seen {
        println!("RV4-R-P4: is_dos_device_name({name:?}) = {device}");
    }
    for (name, device) in seen {
        assert!(device, "{name:?} opens the device on Windows");
    }
}

#[test]
fn a_colon_after_an_ordinary_name_is_not_a_device() {
    for name in ["C:", "a:b", "CONSOLE:x", "COM10:", "NULL:", "x:COM1"] {
        assert!(!is_dos_device_name(name), "{name:?}");
    }
}

#[test]
fn an_absolute_link_through_a_device_with_a_colon_is_refused() {
    let root = Path::new("/proj");
    let refused = resolve_link(root, root, "/proj/sub/COM1:");
    println!("RV4-R-P4: resolve_link(\"/proj/sub/COM1:\") = {refused:?}");
    assert_eq!(
        refused,
        Err("it names a Windows device (CON, NUL, COM1 ...), not a file")
    );
}
