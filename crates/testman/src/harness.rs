use std::io::{BufRead, BufReader, Write as IoWrite};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::Duration;

use miette::{IntoDiagnostic, miette};

pub struct HarnessConfig {
    pub kernel: PathBuf,
    pub initramfs: PathBuf,
    pub memory: String,
    pub cpus: u32,
    pub kvm: bool,
}

impl HarnessConfig {
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

pub struct TestHarness {
    process: Child,
    rx: Receiver<String>,
    stdin: ChildStdin,
    log: Vec<String>,
}

impl TestHarness {
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

    pub fn send(&mut self, line: &str) -> miette::Result<()> {
        writeln!(self.stdin, "{}", line).into_diagnostic()
    }

    pub fn dump_log(&self) -> &[String] {
        &self.log
    }

    pub fn shutdown(mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

#[cfg(test)]
mod tests {
    use super::HarnessConfig;

    // ── HarnessConfig::default ────────────────────────────────────────────────

    #[test]
    fn default_memory_is_2g() {
        assert_eq!(HarnessConfig::default().memory, "2G");
    }

    #[test]
    fn default_cpus_is_2() {
        assert_eq!(HarnessConfig::default().cpus, 2);
    }

    #[test]
    fn default_kvm_is_true() {
        assert!(HarnessConfig::default().kvm);
    }

    // ── HarnessConfig::from_env ───────────────────────────────────────────────

    /// Helper: run a closure with a temporary set of env-var overrides,
    /// restoring (or removing) the originals afterwards.
    ///
    /// This works safely as long as tests that touch the same env-vars are not
    /// run concurrently.  `serial_test::serial` is used on each test below to
    /// enforce that.
    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        use std::env;

        // Remember original values so we can restore them.
        let originals: Vec<(&str, Option<String>)> =
            vars.iter().map(|(k, _)| (*k, env::var(k).ok())).collect();

        for (k, v) in vars {
            unsafe { env::set_var(k, v) };
        }

        f();

        // Restore originals.
        for (k, original) in &originals {
            match original {
                Some(v) => unsafe { env::set_var(k, v) },
                None => unsafe { env::remove_var(k) },
            }
        }
    }

    #[test]
    #[serial_test::serial]
    fn from_env_memory_cpus_kvm_zero() {
        with_env(&[("MEMORY", "512M"), ("CPUS", "4"), ("KVM", "0")], || {
            let cfg = HarnessConfig::from_env();
            assert_eq!(cfg.memory, "512M");
            assert_eq!(cfg.cpus, 4);
            assert!(!cfg.kvm);
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_kvm_false_string_is_disabled() {
        with_env(&[("KVM", "false")], || {
            let cfg = HarnessConfig::from_env();
            assert!(!cfg.kvm, "KVM=false should produce kvm=false");
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_kvm_one_is_enabled() {
        with_env(&[("KVM", "1")], || {
            let cfg = HarnessConfig::from_env();
            assert!(cfg.kvm, "KVM=1 should produce kvm=true");
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_kvm_true_string_is_enabled() {
        with_env(&[("KVM", "true")], || {
            let cfg = HarnessConfig::from_env();
            assert!(cfg.kvm, "KVM=true should produce kvm=true");
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_cpus_is_parsed_correctly() {
        with_env(&[("CPUS", "8")], || {
            assert_eq!(HarnessConfig::from_env().cpus, 8);
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_memory_is_passed_through_as_string() {
        with_env(&[("MEMORY", "4G")], || {
            assert_eq!(HarnessConfig::from_env().memory, "4G");
        });
    }
}
