//! `perman` — permission enforcement via `LD_PRELOAD`.
//!
//! Compiles as a `cdylib` meant to be injected with `LD_PRELOAD`.  It
//! intercepts the C `chdir(2)` syscall wrapper and validates the destination
//! against the calling user's `allowed_dirs` list stored in the userman
//! daemon before forwarding to the real libc `chdir`.
//!
//! The daemon address is read once from the kernel command line (`usvc_ip`)
//! and cached in [`USVC_IP`].  If the daemon is unreachable the call is
//! **denied** (fail-closed).

use std::{
    ffi::{CStr, c_char, c_int},
    mem,
    net::{IpAddr, Ipv4Addr},
    path::Path,
    str::FromStr,
    sync::LazyLock,
};

use actman::cmdline::CmdLineOptions;
use userman::daemon::UserAPI;

static USVC_IP: LazyLock<IpAddr> = LazyLock::new(|| {
    match CmdLineOptions::new()
        .ok()
        .and_then(|c| c.opts().get("usvc_ip").cloned())
    {
        Some(addr) => IpAddr::from_str(&addr).unwrap_or_else(|_| Ipv4Addr::LOCALHOST.into()),
        None => Ipv4Addr::LOCALHOST.into(),
    }
});

/// Determine the username of the calling process.
///
/// Tries `getlogin()` first (covers TTY-attached sessions), then falls back
/// to the `LOGNAME` and `USER` environment variables in that order.
fn current_username() -> Option<String> {
    // SAFETY: getlogin() returns a pointer to static storage; we copy it immediately.
    let ptr = unsafe { libc::getlogin() };
    if !ptr.is_null() {
        return unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .ok()
            .map(str::to_owned);
    }
    std::env::var("LOGNAME")
        .ok()
        .or_else(|| std::env::var("USER").ok())
}

/// Returns `true` if `path` is permitted for `username` according to the
/// userman daemon.  An empty `allowed_dirs` list means no restriction.
/// If the daemon cannot be reached the call is denied (fail-closed).
fn path_is_allowed(path: &Path, username: &str) -> bool {
    let mut api = UserAPI::new();
    api.set_addr(*USVC_IP);
    match api.user(username) {
        Ok(schema) => {
            let dirs = schema.allowed_dirs();
            dirs.is_empty() || dirs.iter().any(|d| path.starts_with(d))
        }
        Err(_) => false,
    }
}

/// LD_PRELOAD intercept for `chdir(2)`.
///
/// Validates the destination against the calling user's `allowed_dirs` before
/// delegating to the real libc `chdir` via `dlsym(RTLD_NEXT)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn chdir(path: *const c_char) -> c_int {
    if path.is_null() {
        unsafe { *libc::__errno_location() = libc::EINVAL };
        return -1;
    }

    let path_cstr = unsafe { CStr::from_ptr(path) };
    let path_str = match path_cstr.to_str() {
        Ok(s) => s,
        Err(_) => {
            unsafe { *libc::__errno_location() = libc::EINVAL };
            return -1;
        }
    };

    if let Some(username) = current_username() {
        if !path_is_allowed(Path::new(path_str), &username) {
            unsafe { *libc::__errno_location() = libc::EACCES };
            return -1;
        }
    }

    // Resolve the real chdir through the dynamic linker.
    let sym = unsafe { libc::dlsym(libc::RTLD_NEXT, b"chdir\0".as_ptr() as *const c_char) };
    if sym.is_null() {
        unsafe { *libc::__errno_location() = libc::ENOSYS };
        return -1;
    }
    // SAFETY: dlsym(RTLD_NEXT, "chdir") returns the next "chdir" symbol in the
    // dynamic-linker chain, which has exactly this signature per POSIX.
    let real_chdir: unsafe extern "C" fn(*const c_char) -> c_int = unsafe { mem::transmute(sym) };
    unsafe { real_chdir(path) }
}
