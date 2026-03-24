//! QEMU test harness used by `testman`'s integration tests.
//!
//! [`HarnessConfig`] holds the launch parameters for a QEMU instance;
//! [`TestHarness`] wraps the running process and provides a structured way to
//! interact with it during tests.
//!
//! The harness captures QEMU stdout line-by-line on a background thread and
//! exposes [`TestHarness::wait_for`] for sequential boot-stage assertions:
//! each call blocks until a line containing the expected pattern appears, or
//! until the supplied timeout elapses.

use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use miette::{IntoDiagnostic, miette};

/// Configuration for launching a QEMU instance.
///
/// Build via [`HarnessConfig::from_env`] (reads environment variables) or
/// [`Default::default`] (sensible defaults for unit tests).
pub struct HarnessConfig {
    /// Path to the kernel image passed to `-kernel`.
    pub kernel: PathBuf,
    /// Path to the initramfs archive passed to `-initrd`.
    pub initramfs: PathBuf,
    /// Memory size string passed to `-m` (e.g. `"2G"`).
    pub memory: String,
    /// Number of vCPUs passed to `-smp`.
    pub cpus: u32,
    /// If `true`, passes `-enable-kvm` to `qemu-system-x86_64`.
    pub kvm: bool,
}

impl HarnessConfig {
    /// Builds a [`HarnessConfig`] from environment variables.
    ///
    /// | Env var    | Default                             | Description                       |
    /// |------------|-------------------------------------|-----------------------------------|
    /// | `KERNEL`   | `/boot/vmlinuz-$(uname -r)`         | Path to the kernel image          |
    /// | `INITRAMFS`| `os.initramfs.tar.gz`               | Path to the initramfs archive     |
    /// | `MEMORY`   | `"2G"`                              | QEMU `-m` argument                |
    /// | `CPUS`     | `2`                                 | QEMU `-smp` argument              |
    /// | `KVM`      | `"1"` (enabled)                     | Set to `"0"` or `"false"` to disable |
    pub fn from_env() -> Self {
        let kernel = PathBuf::from(std::env::var("KERNEL").unwrap_or_else(|_| {
            format!(
                "/boot/vmlinuz-{}",
                std::process::Command::new("uname")
                    .arg("-r")
                    .output()
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .unwrap_or_default()
            )
        }));
        let initramfs = PathBuf::from(
            std::env::var("INITRAMFS").unwrap_or_else(|_| "os.initramfs.tar.gz".to_string()),
        );
        let memory = std::env::var("MEMORY").unwrap_or_else(|_| "2G".to_string());
        let cpus = std::env::var("CPUS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(2u32);
        let kvm = std::env::var("KVM")
            .map(|v| v != "0" && v.to_lowercase() != "false")
            .unwrap_or(true);
        Self {
            kernel,
            initramfs,
            memory,
            cpus,
            kvm,
        }
    }
}

impl Default for HarnessConfig {
    fn default() -> Self {
        Self {
            kernel: PathBuf::new(),
            initramfs: PathBuf::new(),
            memory: "2G".into(),
            cpus: 2,
            kvm: true,
        }
    }
}

/// A running QEMU instance wrapped as a test harness.
///
/// Spawn with [`TestHarness::start`]. Use [`wait_for`](TestHarness::wait_for)
/// to assert that expected log lines appear within a timeout, and
/// [`shutdown`](TestHarness::shutdown) to kill the process when done.
pub struct TestHarness {
    process: Child,
    rx: Receiver<String>,
    stdin: ChildStdin,
    log: Vec<String>,
}

impl TestHarness {
    /// Launches `qemu-system-x86_64` with the given configuration and returns a
    /// handle to the running instance.
    ///
    /// Spawns a background thread that reads QEMU stdout line-by-line and
    /// forwards each line to an internal `mpsc` channel consumed by
    /// [`wait_for`](Self::wait_for).
    ///
    /// # Errors
    ///
    /// Returns a [`miette::Report`] if QEMU cannot be spawned or if stdin/stdout
    /// pipes cannot be opened.
    pub fn start(config: HarnessConfig) -> miette::Result<Self> {
        let mut cmd = Command::new("qemu-system-x86_64");
        cmd.args([
            "-kernel",
            config
                .kernel
                .to_str()
                .ok_or_else(|| miette!("kernel path is not valid UTF-8"))?,
            "-initrd",
            config
                .initramfs
                .to_str()
                .ok_or_else(|| miette!("initramfs path is not valid UTF-8"))?,
            "-append",
            "console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0",
            "-nographic",
            "-m",
            &config.memory,
            "-smp",
            &config.cpus.to_string(),
            "-netdev",
            "user,id=n0",
            "-device",
            "virtio-net-pci,netdev=n0",
        ]);

        if config.kvm {
            cmd.arg("-enable-kvm");
        }

        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut process = cmd.spawn().into_diagnostic()?;

        let stdin = process
            .stdin
            .take()
            .ok_or_else(|| miette!("failed to open QEMU stdin"))?;
        let stdout = process
            .stdout
            .take()
            .ok_or_else(|| miette!("failed to open QEMU stdout"))?;

        let (tx, rx) = mpsc::channel();

        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(l) => {
                        if tx.send(l).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            process,
            rx,
            stdin,
            log: Vec::new(),
        })
    }

    /// Blocks until a line containing `pattern` is received from QEMU stdout, or
    /// until `timeout` elapses.
    ///
    /// Returns `Ok(true)` if the pattern was seen, `Ok(false)` on timeout, and
    /// `Err` if the QEMU stdout channel was closed unexpectedly.
    ///
    /// All received lines are appended to the internal log regardless of whether
    /// they match, so [`dump_log`](Self::dump_log) always contains the full
    /// output seen so far.
    pub fn wait_for(&mut self, pattern: &str, timeout: Duration) -> miette::Result<bool> {
        let deadline = std::time::Instant::now() + timeout;

        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }

            match self.rx.recv_timeout(remaining) {
                Ok(line) => {
                    self.log.push(line.clone());
                    if line.contains(pattern) {
                        return Ok(true);
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(false),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(miette!("QEMU stdout closed unexpectedly"));
                }
            }
        }
    }

    /// Writes `line` followed by a newline to QEMU's stdin.
    pub fn send(&mut self, line: &str) -> miette::Result<()> {
        writeln!(self.stdin, "{}", line).into_diagnostic()
    }

    /// Returns all lines received from QEMU stdout since the harness was started.
    pub fn dump_log(&self) -> &[String] {
        &self.log
    }

    /// Kills the QEMU process and waits for it to exit. The harness is consumed.
    pub fn shutdown(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}
