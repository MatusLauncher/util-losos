#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/testman.md"))]
pub mod harness;
pub use harness::{HarnessConfig, TestHarness};

#[cfg(test)]
mod tests {
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use serial_test::serial;

    use super::{HarnessConfig, TestHarness};

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
    #[ignore = "requires QEMU, a kernel, and initramfs — run via launch.sh --test"]
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
    #[ignore = "requires QEMU, a kernel, and initramfs — run via launch.sh --test"]
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
    #[ignore = "requires QEMU, a kernel, and initramfs — run via launch.sh --test"]
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
    #[ignore = "requires QEMU, a kernel, and initramfs — run via launch.sh --test"]
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
    #[ignore = "requires QEMU, a kernel, and initramfs — run via launch.sh --test"]
    fn test_05_dhcp_configured_eth0() {
        let mut h = harness().lock().unwrap();
        assert!(
            h.wait_for("eth0 configured via DHCP", Duration::from_secs(90))
                .expect("harness error"),
            "DHCP configuration not seen within 90s\n{}",
            h.dump_log().join("\n")
        );
    }

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

    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        use std::env;
        let originals: Vec<(&str, Option<String>)> =
            vars.iter().map(|(k, _)| (*k, env::var(k).ok())).collect();
        for (k, v) in vars {
            unsafe { env::set_var(k, v) };
        }
        f();
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
            assert!(!HarnessConfig::from_env().kvm);
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_kvm_one_is_enabled() {
        with_env(&[("KVM", "1")], || {
            assert!(HarnessConfig::from_env().kvm);
        });
    }

    #[test]
    #[serial_test::serial]
    fn from_env_kvm_true_string_is_enabled() {
        with_env(&[("KVM", "true")], || {
            assert!(HarnessConfig::from_env().kvm);
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
