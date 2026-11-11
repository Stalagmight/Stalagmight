//! Diagnostic for #706: what does macOS do when Rust std's SIGSEGV/SIGBUS
//! handler cannot fetch its own first instruction?
//!
//! Not part of the app and never linked into it. The
//! `.github/workflows/diag-706-macos.yml` workflow runs it on a hosted macOS
//! runner, one process per mode. The driver script reads the `FL706 ...`
//! lines this prints, then decides what happened from the process state:
//! dead by which signal, or alive and burning CPU.
//!
//! Modes:
//! - `control`: plain null write. Expect a quick death by SIGSEGV.
//! - `noexec`: take execute permission away from the page holding std's
//!   handler, then do the null write. This models "the handler's code page
//!   cannot be fetched".
//! - `armed`: evict this executable's `__TEXT` pages from memory and tell the
//!   driver. The driver then removes the backing file: it force-detaches the
//!   disk image or rewrites the file in place. Then it touches the trigger
//!   file, and this process faults.
//! - `armed-noevict`: the same without the eviction, to see whether removing
//!   the backing file alone makes the resident pages unusable.
//! - `armed-mlock`: the same as `armed`, but first wire (`mlock`) the
//!   handler's page and the import stubs. This is the proposed mitigation.
//!
//! The armed modes fault with the same null write as `control`. That write
//! sits in a page evicted along with the rest, so the first fault may be its
//! own fetch. Either way the kernel then has to run std's handler.

use std::io::Write;
use std::time::Duration;

fn page_size() -> usize {
    // SAFETY: sysconf has no preconditions.
    let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    usize::try_from(ps).expect("page size")
}

fn page_of(addr: usize) -> usize {
    addr & !(page_size() - 1)
}

fn say(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "FL706 {line}");
    let _ = out.flush();
}

/// The handler std installed for `sig` before `main` ran.
fn installed_handler(sig: libc::c_int) -> usize {
    // SAFETY: zeroed sigaction is a valid out-parameter; a null `act` only reads.
    let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::sigaction(sig, std::ptr::null(), &mut old) };
    assert_eq!(rc, 0, "sigaction query failed");
    old.sa_sigaction
}

/// Whether the page holding `addr` is resident, per mincore.
fn resident(addr: usize) -> String {
    let page = page_of(addr);
    #[cfg(target_os = "macos")]
    {
        let mut v: libc::c_char = 0;
        // SAFETY: one page, one status byte.
        let rc = unsafe { libc::mincore(page as *const libc::c_void, page_size(), &mut v) };
        if rc != 0 {
            return format!("err{}", std::io::Error::last_os_error());
        }
        format!("{}", (v & 1) != 0)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let mut v: libc::c_uchar = 0;
        // SAFETY: one page, one status byte.
        let rc = unsafe { libc::mincore(page as *mut libc::c_void, page_size(), &mut v) };
        if rc != 0 {
            return format!("err{}", std::io::Error::last_os_error());
        }
        format!("{}", (v & 1) != 0)
    }
}

/// The address range of this executable's `__TEXT` segment, and of its
/// `__stubs` section (the import stubs every libc call goes through).
#[cfg(target_os = "macos")]
fn text_layout() -> ((usize, usize), (usize, usize)) {
    // SAFETY: image 0 is the main executable; its header is mapped for the
    // process lifetime and the load commands follow it in memory.
    unsafe {
        let hdr = libc::_dyld_get_image_header(0) as usize;
        let ncmds = *((hdr + 16) as *const u32);
        let mut off = hdr + 32;
        let mut text = (0usize, 0usize);
        let mut stubs = (0usize, 0usize);
        for _ in 0..ncmds {
            let cmd = *(off as *const u32);
            let size = *((off + 4) as *const u32) as usize;
            if cmd == 0x19 {
                let name = std::slice::from_raw_parts((off + 8) as *const u8, 16);
                if name.starts_with(b"__TEXT\0") {
                    let vmaddr = *((off + 24) as *const u64) as usize;
                    let vmsize = *((off + 32) as *const u64) as usize;
                    let slide = hdr - vmaddr;
                    text = (hdr, vmsize);
                    let nsects = *((off + 64) as *const u32) as usize;
                    for s in 0..nsects {
                        let sec = off + 72 + s * 80;
                        let sname = std::slice::from_raw_parts(sec as *const u8, 16);
                        if sname.starts_with(b"__stubs\0") {
                            let addr = *((sec + 32) as *const u64) as usize + slide;
                            let len = *((sec + 40) as *const u64) as usize;
                            stubs = (addr, len);
                        }
                    }
                }
            }
            off += size;
        }
        (text, stubs)
    }
}

#[cfg(not(target_os = "macos"))]
fn text_layout() -> ((usize, usize), (usize, usize)) {
    // Linux dry runs only need `control` and `noexec`.
    ((0, 0), (0, 0))
}

#[inline(never)]
fn null_write() {
    // SAFETY: deliberately faulting; this is the point of the program.
    unsafe { std::ptr::write_volatile(std::hint::black_box(std::ptr::null_mut::<u64>()), 1) }
}

/// Never called before the trigger, so its page is not re-faulted after the
/// eviction. Padded with an unrolled body so it is unlikely to share a page
/// with the wait loop.
#[inline(never)]
#[cold]
extern "C" fn cold_entry(x: u64) -> u64 {
    let mut acc = x;
    macro_rules! r {
        () => {
            acc = std::hint::black_box(acc.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407));
        };
    }
    macro_rules! r16 {
        () => {
            r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!(); r!();
        };
    }
    r16!(); r16!(); r16!(); r16!(); r16!(); r16!(); r16!(); r16!();
    r16!(); r16!(); r16!(); r16!(); r16!(); r16!(); r16!(); r16!();
    acc
}

fn lock_range(start: usize, len: usize, what: &str) {
    let ps = page_size();
    let first = page_of(start);
    let end = (start + len.max(1) + ps - 1) & !(ps - 1);
    // SAFETY: mapped range of our own image; mlock only wires it.
    let rc = unsafe { libc::mlock(first as *const libc::c_void, end - first) };
    let err = if rc == 0 { String::new() } else { format!(" err={}", std::io::Error::last_os_error()) };
    say(&format!("mlock what={what} start={first:#x} len={:#x} rc={rc}{err}", end - first));
}

/// Push every unlocked page of `__TEXT` out of memory. Reports how many pages
/// are still resident afterwards, so the log shows whether eviction worked.
fn evict_text(text: (usize, usize), keep: &[(usize, usize)]) {
    let ps = page_size();
    let (start, len) = text;
    let kept = |p: usize| keep.iter().any(|&(s, l)| p + ps > page_of(s) && p < s + l.max(1));
    let mut ms_fail = 0usize;
    let mut pages = 0usize;
    let mut p = start;
    while p < start + len {
        if !kept(p) {
            pages += 1;
            // SAFETY: page-aligned range inside our own mapped image.
            unsafe {
                if libc::msync(p as *mut libc::c_void, ps, libc::MS_INVALIDATE) != 0 {
                    ms_fail += 1;
                }
                libc::madvise(p as *mut libc::c_void, ps, libc::MADV_DONTNEED);
            }
        }
        p += ps;
    }
    let mut still = 0usize;
    p = start;
    while p < start + len {
        if !kept(p) && resident(p) == "true" {
            still += 1;
        }
        p += ps;
    }
    say(&format!("evict pages={pages} msync_failures={ms_fail} still_resident={still}"));
}

fn trigger_present(path: &std::ffi::CStr) -> bool {
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::access(path.as_ptr(), libc::F_OK) == 0 }
}

/// mincore of one page with nothing but the libc call: 1 resident, 0 not, 2 error.
#[inline(never)]
fn resident_raw(addr: usize) -> u8 {
    let page = page_of(addr);
    #[cfg(target_os = "macos")]
    let (rc, v) = {
        let mut v: libc::c_char = 0;
        // SAFETY: one page, one status byte.
        let rc = unsafe { libc::mincore(page as *const libc::c_void, page_size(), &mut v) };
        (rc, (v & 1) as u8)
    };
    #[cfg(not(target_os = "macos"))]
    let (rc, v) = {
        let mut v: libc::c_uchar = 0;
        // SAFETY: one page, one status byte.
        let rc = unsafe { libc::mincore(page as *mut libc::c_void, page_size(), &mut v) };
        (rc, v & 1)
    };
    if rc != 0 { 2 } else { v }
}

/// Push one page of our own text out of memory (best effort).
#[inline(never)]
fn evict_page(addr: usize) {
    let page = page_of(addr);
    // SAFETY: page-aligned, inside our own mapped image.
    unsafe {
        libc::msync(page as *mut libc::c_void, page_size(), libc::MS_INVALIDATE);
        libc::madvise(page as *mut libc::c_void, page_size(), libc::MADV_DONTNEED);
    }
}

/// The candidate mitigation: keep std's handler but re-install it with
/// SA_NODEFER | SA_RESETHAND. The kernel then resets the disposition to
/// SIG_DFL on entry and leaves the signal unblocked, so a second fault inside
/// the handler takes the default action (a crash) instead of being parked as
/// pending on a blocked signal.
fn apply_resethand() {
    for sig in [libc::SIGSEGV, libc::SIGBUS] {
        // SAFETY: zeroed sigaction is a valid out-parameter; we re-install the
        // handler std put there, with two extra flags.
        unsafe {
            let mut cur: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut cur);
            cur.sa_flags |= libc::SA_NODEFER | libc::SA_RESETHAND;
            let rc = libc::sigaction(sig, &cur, std::ptr::null_mut());
            say(&format!("resethand sig={sig} rc={rc} flags={:#x}", cur.sa_flags));
        }
    }
}

#[inline(never)]
fn recurse(depth: u64) -> u64 {
    let buf = std::hint::black_box([depth; 64]);
    if depth == u64::MAX {
        return 0;
    }
    recurse(depth + 1).wrapping_add(buf[(depth % 64) as usize])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).map_or("control", String::as_str).to_owned();
    let trigger = args.get(2).cloned().unwrap_or_else(|| "/tmp/fl706.trigger".into());
    let resethand = std::env::var_os("FL706_RESETHAND").is_some_and(|v| v == "1");
    if resethand {
        apply_resethand();
    }
    let handler = installed_handler(libc::SIGSEGV);
    let bus_handler = installed_handler(libc::SIGBUS);
    let ((text_start, text_len), stubs) = text_layout();
    let cold = cold_entry as *const () as usize;
    let here = main as *const () as usize;
    say(&format!(
        "pid={} mode={mode} resethand={resethand} page_size={:#x} handler={handler:#x} bus_handler={bus_handler:#x} \
         handler_page={:#x} cold_page={:#x} main_page={:#x} null_write_page={:#x} text={text_start:#x}+{text_len:#x} stubs={:#x}+{:#x}",
        std::process::id(),
        page_size(),
        page_of(handler),
        page_of(cold),
        page_of(here),
        page_of(null_write as *const () as usize),
        stubs.0,
        stubs.1,
    ));
    match mode.as_str() {
        "control" => {
            say("fault=null_write");
            null_write();
        }
        "overflow" => {
            say("fault=recursion");
            let v = recurse(0);
            say(&format!("survived recursion {v}"));
        }
        "noexec" => {
            if page_of(handler) == page_of(here) || page_of(handler) == page_of(null_write as *const () as usize) {
                say("warn=handler shares a page with main/null_write; the first fault is the fetch itself");
            }
            // SAFETY: drops execute permission on one page of our own text.
            let rc = unsafe { libc::mprotect(page_of(handler) as *mut libc::c_void, page_size(), libc::PROT_READ) };
            say(&format!("mprotect rc={rc} errno={}", std::io::Error::last_os_error()));
            say("fault=null_write");
            null_write();
        }
        "armed" | "armed-noevict" | "armed-mlock" => {
            let evict = mode != "armed-noevict";
            let mut keep = vec![(stubs.0, stubs.1)];
            if mode == "armed-mlock" {
                lock_range(handler, 4096, "handler");
                lock_range(stubs.0, stubs.1, "stubs");
                keep.push((handler, 4096));
            }
            let c = std::ffi::CString::new(trigger.clone()).expect("trigger path");
            let res_path = std::ffi::CString::new(format!("{trigger}.res")).expect("res path");
            // SAFETY: valid path; the fd lives until the process dies.
            let fd = unsafe { libc::open(res_path.as_ptr(), libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o644) };
            if evict {
                evict_text((text_start, text_len), &keep);
            }
            say(&format!(
                "residency handler={} cold={} null_write={} stubs={}",
                resident(handler),
                resident(cold),
                resident(null_write as *const () as usize),
                resident(stubs.0)
            ));
            say("state=armed");
            let targets = [handler, null_write as *const () as usize];
            while !trigger_present(&c) {
                // Page-in read-ahead can pull the handler page back in, so keep
                // pushing the two pages that matter out, and record what the
                // last check saw for the driver. Only libc calls and this
                // loop's own (resident) code run here.
                let mut rec = [b'-', b'-', b'\n'];
                for (i, &t) in targets.iter().enumerate() {
                    let mut r = resident_raw(t);
                    if evict && mode != "armed-mlock" && r == 1 {
                        evict_page(t);
                        r = resident_raw(t);
                    } else if evict && mode == "armed-mlock" && i == 1 && r == 1 {
                        evict_page(t);
                        r = resident_raw(t);
                    }
                    rec[i] = b'0' + r;
                }
                // SAFETY: writing 3 bytes from a stack buffer to our own fd.
                unsafe { libc::pwrite(fd, rec.as_ptr().cast(), rec.len(), 0) };
                std::thread::sleep(Duration::from_millis(100));
            }
            // Deliberately no printing before the fault: std's stdout path is
            // evicted too, and the point is what the next page-in does.
            null_write();
            say("survived null_write");
        }
        other => {
            say(&format!("unknown mode {other}"));
            std::process::exit(2);
        }
    }
    say("returned");
}
