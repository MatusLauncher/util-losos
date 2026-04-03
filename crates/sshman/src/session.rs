//! PTY allocation, shell spawning, and Landlock policy enforcement.

use std::{
    ffi::CString,
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::PathBuf,
};

use landlock::{AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus};
use miette::{IntoDiagnostic, miette};
use tracing::{info, warn};
use userman::daemon::UserSchema;

/// An active PTY session with a child process.
pub struct PtySession {
    /// Master side of the PTY pair.
    pub master_fd: OwnedFd,
    /// PID of the child shell process.
    pub child_pid: i32,
}

impl PtySession {
    /// Allocate a PTY, fork, and exec `/bin/sh -l` in the child.
    ///
    /// The child process gets Landlock restrictions applied before exec.
    pub fn spawn_shell(
        user: &UserSchema,
        term: &str,
        cols: u32,
        rows: u32,
    ) -> miette::Result<Self> {
        Self::spawn_inner(user, None, term, cols, rows)
    }

    /// Allocate a PTY, fork, and exec `command` in the child.
    pub fn spawn_exec(
        user: &UserSchema,
        command: &str,
        term: &str,
        cols: u32,
        rows: u32,
    ) -> miette::Result<Self> {
        Self::spawn_inner(user, Some(command), term, cols, rows)
    }

    fn spawn_inner(
        user: &UserSchema,
        command: Option<&str>,
        term: &str,
        cols: u32,
        rows: u32,
    ) -> miette::Result<Self> {
        // Open a new PTY master.
        let master_fd = rustix::pty::openpt(rustix::pty::OpenptFlags::RDWR | rustix::pty::OpenptFlags::NOCTTY)
            .into_diagnostic()?;
        rustix::pty::grantpt(&master_fd).into_diagnostic()?;
        rustix::pty::unlockpt(&master_fd).into_diagnostic()?;
        let slave_name = rustix::pty::ptsname(&master_fd, Vec::new()).into_diagnostic()?;

        // Set initial window size on the master.
        set_winsize(master_fd.as_raw_fd(), cols, rows);

        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(miette!("fork() failed"));
        }

        if pid == 0 {
            // ── Child process ────────────────────────────────────────
            // Drop the master fd in the child.
            drop(master_fd);

            // Create a new session.
            unsafe { libc::setsid() };

            // Open the slave PTY.
            let slave_c = CString::new(slave_name.as_bytes()).unwrap();
            let slave_raw = unsafe { libc::open(slave_c.as_ptr(), libc::O_RDWR) };
            if slave_raw < 0 {
                std::process::exit(1);
            }

            // Make it the controlling terminal.
            unsafe { libc::ioctl(slave_raw, libc::TIOCSCTTY, 0) };

            // Dup to stdin/stdout/stderr.
            unsafe {
                libc::dup2(slave_raw, 0);
                libc::dup2(slave_raw, 1);
                libc::dup2(slave_raw, 2);
                if slave_raw > 2 {
                    libc::close(slave_raw);
                }
            }

            // Apply Landlock session policy.
            let _ = apply_session_policy(user.allowed_dirs());

            // Set environment.
            let username = user.name();
            let home = format!("/home/{username}");
            unsafe {
                std::env::set_var("USER", username);
                std::env::set_var("HOME", &home);
                std::env::set_var("PS1", format!("{username}:$PWD$ "));
                std::env::set_var("TERM", term);
            }

            // Exec the requested command or a login shell.
            match command {
                Some(cmd) => {
                    let c_sh = CString::new("/bin/sh").unwrap();
                    let c_flag = CString::new("-c").unwrap();
                    let c_cmd = CString::new(cmd).unwrap();
                    unsafe {
                        libc::execvp(
                            c_sh.as_ptr(),
                            [c_sh.as_ptr(), c_flag.as_ptr(), c_cmd.as_ptr(), std::ptr::null()].as_ptr(),
                        );
                    }
                }
                None => {
                    let c_sh = CString::new("/bin/sh").unwrap();
                    let c_login = CString::new("-l").unwrap();
                    unsafe {
                        libc::execvp(
                            c_sh.as_ptr(),
                            [c_sh.as_ptr(), c_login.as_ptr(), std::ptr::null()].as_ptr(),
                        );
                    }
                }
            }
            // If execvp returns, exit.
            std::process::exit(1);
        }

        // ── Parent process ───────────────────────────────────────
        info!("Spawned shell for user '{}' (pid {pid})", user.name());
        Ok(PtySession {
            master_fd,
            child_pid: pid,
        })
    }

    /// Resize the PTY to the given dimensions.
    pub fn resize(&self, cols: u32, rows: u32) {
        set_winsize(self.master_fd.as_raw_fd(), cols, rows);
    }

    /// Send SIGHUP to the child process.
    pub fn kill(&self) {
        unsafe { libc::kill(self.child_pid, libc::SIGHUP) };
    }

    /// Non-blocking waitpid — returns `true` if child has exited.
    pub fn try_wait(&self) -> bool {
        let mut status = 0i32;
        let ret = unsafe { libc::waitpid(self.child_pid, &mut status, libc::WNOHANG) };
        ret > 0
    }
}

/// Convert the master fd into an async reader/writer via tokio.
///
/// # Safety
/// The caller must ensure `raw_fd` is a valid, open file descriptor.
pub unsafe fn master_to_async(raw_fd: i32) -> std::io::Result<tokio::fs::File> {
    let owned = unsafe { OwnedFd::from_raw_fd(raw_fd) };
    let std_file = std::fs::File::from(owned);
    Ok(tokio::fs::File::from_std(std_file))
}

fn set_winsize(fd: i32, cols: u32, rows: u32) {
    let ws = libc::winsize {
        ws_row: rows as u16,
        ws_col: cols as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    unsafe { libc::ioctl(fd, libc::TIOCSWINSZ, &ws) };
}

/// Install a Landlock filesystem policy restricting `chdir` to the supplied
/// directories.  Replicates perman's `apply_session_policy`.
fn apply_session_policy(allowed_dirs: &[PathBuf]) -> miette::Result<()> {
    if allowed_dirs.is_empty() {
        return Ok(());
    }

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::ReadDir)
        .into_diagnostic()?
        .create()
        .into_diagnostic()?;

    for dir in allowed_dirs {
        let fd = PathFd::new(dir).into_diagnostic()?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::ReadDir))
            .into_diagnostic()?;
    }

    let status = ruleset.restrict_self().into_diagnostic()?;

    if status.ruleset == RulesetStatus::NotEnforced {
        warn!(
            "Landlock not enforced by this kernel — session policy not applied"
        );
    }

    Ok(())
}
