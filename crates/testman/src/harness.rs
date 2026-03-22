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
