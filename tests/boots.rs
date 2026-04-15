//! Integration tests for the bootenv stage-1 initramfs.
//!
//! Tests boot a real QEMU VM and assert that bootenv's structured log output
//! appears in the correct order.  All tests share a single QEMU instance via
//! [`HARNESS`] and run sequentially under [`serial_test::serial`].
//!
//! # Running
//!
//! ```sh
//! # Default (ISO boot, KVM enabled):
//! KERNEL=vmlinuz INITRAMFS=initramfs.gz cargo nextest run --test boots -- --ignored
//!
//! # Without KVM (CI):
//! KVM=0 KERNEL=vmlinuz INITRAMFS=initramfs.gz cargo nextest run --test boots -- --ignored
//! ```
//!
//! Environment variables forwarded to [`testman::HarnessConfig::from_env`]:
//! `KERNEL`, `INITRAMFS`, `ISO`, `MEMORY`, `CPUS`, `KVM`, `TEST_MODE`.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serial_test::serial;
use testman::harness::TestMode;
use testman::{ContainerHarness, Harness, HarnessConfig, TestHarness};

static HARNESS: OnceLock<Mutex<Harness>> = OnceLock::new();

fn harness() -> &'static Mutex<Harness> {
    HARNESS.get_or_init(|| {
        let _ = tracing_subscriber::fmt::try_init();
        let config = HarnessConfig::from_env();
        let h = match config.mode {
            TestMode::Container => Harness::Container(Box::new(
                ContainerHarness::start(&config)
                    .expect("failed to start container harness"),
            )),
            TestMode::Qemu => Harness::Qemu(
                TestHarness::start(config).expect("failed to start QEMU"),
            ),
        };
        Mutex::new(h)
    })
}

/// The kernel must print its version banner very early in boot.
#[test]
#[serial]
#[ignore = "requires QEMU and a bootable image — set KERNEL+INITRAMFS or ISO"]
fn test_01_kernel_boots() {
    let mut h = harness().lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        h.wait_for("Linux version", Duration::from_secs(15))
            .expect("harness error"),
        "kernel boot message not seen within 15s\n{}",
        h.dump_log().join("\n")
    );
}

/// bootenv must log its startup banner before doing any work.
#[test]
#[serial]
#[ignore = "requires QEMU and a bootable image — set KERNEL+INITRAMFS or ISO"]
fn test_02_bootenv_starts() {
    let mut h = harness().lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        h.wait_for("bootenv starting", Duration::from_secs(20))
            .expect("harness error"),
        "bootenv startup message not seen within 20s\n{}",
        h.dump_log().join("\n")
    );
}

/// bootenv must mount virtual filesystems (devtmpfs, proc, sysfs, tmpfs) so
/// that `/proc/cmdline` is readable before parsing kernel parameters.
#[test]
#[serial]
#[ignore = "requires QEMU and a bootable image — set KERNEL+INITRAMFS or ISO"]
fn test_03_virtual_filesystems_mounted() {
    let mut h = harness().lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        h.wait_for("Mounting devtmpfs on /dev", Duration::from_secs(25))
            .expect("harness error"),
        "virtual filesystem mount message not seen within 25s\n{}",
        h.dump_log().join("\n")
    );
}

/// bootenv must hand control to actman (`/bin/init`) as its final act.
/// Seeing this message confirms the full stage-1 boot flow completed.
#[test]
#[serial]
#[ignore = "requires QEMU and a bootable image — set KERNEL+INITRAMFS or ISO"]
fn test_04_actman_handover() {
    let mut h = harness().lock().unwrap_or_else(|e| e.into_inner());
    assert!(
        h.wait_for("Handing over to actman", Duration::from_secs(30))
            .expect("harness error"),
        "actman handover message not seen within 30s\n{}",
        h.dump_log().join("\n")
    );
}

/// After a complete boot, the log must contain no kernel panic lines.
/// This is checked against the accumulated log after all other tests have run.
#[test]
#[serial]
#[ignore = "requires QEMU and a bootable image — set KERNEL+INITRAMFS or ISO"]
fn test_05_no_kernel_panic() {
    let h = harness().lock().unwrap_or_else(|e| e.into_inner());
    let log = h.dump_log().join("\n");
    assert!(
        !log.contains("Kernel panic"),
        "kernel panic detected in boot log\n{log}"
    );
}
