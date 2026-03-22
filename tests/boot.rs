use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serial_test::serial;
use testman::{HarnessConfig, TestHarness};

static HARNESS: OnceLock<Mutex<TestHarness>> = OnceLock::new();

fn harness() -> &'static Mutex<TestHarness> {
    HARNESS.get_or_init(|| {
        let _ = tracing_subscriber::fmt::try_init();
        let config = HarnessConfig::from_env();
        Mutex::new(TestHarness::start(config).expect("failed to start QEMU"))
    })
}

#[test]
#[serial]
fn test_01_kernel_boots() {
    let mut h = harness().lock().unwrap();
    assert!(
        h.wait_for("Linux version", Duration::from_secs(15))
            .expect("harness error"),
        "kernel boot message not seen within 15s\n{}",
        h.dump_log().join("\n")
    );
}

#[test]
#[serial]
fn test_02_init_starts() {
    let mut h = harness().lock().unwrap();
    assert!(
        h.wait_for("Mounting", Duration::from_secs(20))
            .expect("harness error"),
        "init Mounting message not seen within 20s\n{}",
        h.dump_log().join("\n")
    );
}

#[test]
#[serial]
fn test_03_filesystems_mounted() {
    let mut h = harness().lock().unwrap();
    assert!(
        h.wait_for("Spawning", Duration::from_secs(30))
            .expect("harness error"),
        "Spawning message not seen within 30s\n{}",
        h.dump_log().join("\n")
    );
}

#[test]
#[serial]
fn test_04_startup_scripts_run() {
    let mut h = harness().lock().unwrap();
    assert!(
        h.wait_for("Spawning /etc/init/start/sh", Duration::from_secs(45))
            .expect("harness error"),
        "startup scripts not seen within 45s\n{}",
        h.dump_log().join("\n")
    );
}

#[test]
#[serial]
fn test_05_dhcp_configured_eth0() {
    let mut h = harness().lock().unwrap();
    assert!(
        h.wait_for("eth0 configured via DHCP", Duration::from_secs(90))
            .expect("harness error"),
        "DHCP configuration not seen within 90s\n{}",
        h.dump_log().join("\n")
    );
}
