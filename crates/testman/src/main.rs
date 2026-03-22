use std::path::PathBuf;
use std::time::Duration;

use testman::suite::TestResult;
use testman::{HarnessConfig, TestSuite};
use tracing::info;

fn main() -> miette::Result<()> {
    tracing_subscriber::fmt::init();

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

    info!(
        kernel = %kernel.display(),
        initramfs = %initramfs.display(),
        memory,
        cpus,
        kvm,
        "Starting testman"
    );

    let config = HarnessConfig {
        kernel,
        initramfs,
        memory,
        cpus,
        kvm,
    };

    let suite = TestSuite::new()
        .test("kernel boots", |h| {
            match h.wait_for("Linux version", Duration::from_secs(15)) {
                Ok(true) => TestResult::Pass,
                Ok(false) => TestResult::Timeout,
                Err(e) => TestResult::Fail(e.to_string()),
            }
        })
        .test("init starts", |h| {
            match h.wait_for("Mounting", Duration::from_secs(20)) {
                Ok(true) => TestResult::Pass,
                Ok(false) => TestResult::Timeout,
                Err(e) => TestResult::Fail(e.to_string()),
            }
        })
        .test("filesystems mounted", |h| {
            match h.wait_for("Spawning", Duration::from_secs(30)) {
                Ok(true) => TestResult::Pass,
                Ok(false) => TestResult::Timeout,
                Err(e) => TestResult::Fail(e.to_string()),
            }
        })
        .test("startup scripts run", |h| {
            match h.wait_for("Spawning /etc/init/start/sh", Duration::from_secs(45)) {
                Ok(true) => TestResult::Pass,
                Ok(false) => TestResult::Timeout,
                Err(e) => TestResult::Fail(e.to_string()),
            }
        })
        .test("dhcp configured eth0", |h| {
            match h.wait_for("eth0 configured via DHCP", Duration::from_secs(90)) {
                Ok(true) => TestResult::Pass,
                Ok(false) => TestResult::Timeout,
                Err(e) => TestResult::Fail(e.to_string()),
            }
        });

    let report = suite.run(config)?;
    report.print();

    if report.has_failures() {
        std::process::exit(1);
    }

    Ok(())
}
