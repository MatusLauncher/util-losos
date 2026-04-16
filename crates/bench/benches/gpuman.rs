//! Smoke tests for the `gpuman` crate.
//!
//! Exercises:
//! * `ModeOfOperation::from` — executable-name-to-mode dispatch.
//! * `GpuVendor` / `DeviceClass` / `GpuDevice` — Display formatting and vendor
//!   helper methods.
//! * `vendors_present` — vendor deduplication over device lists.
//! * `build_container_spec` — container specification construction from
//!   detected devices and kernel command-line overrides.

use std::collections::HashMap;
use std::hint::black_box;

use gpuman::container::build_container_spec;
use gpuman::detect::{DeviceClass, GpuDevice, GpuVendor, vendors_present};
use gpuman::mode::ModeOfOperation;

// ── helpers ─────────────────────────────────────────────────────────

fn make_device(vendor: GpuVendor, slot: &str) -> GpuDevice {
    GpuDevice {
        vendor,
        class: DeviceClass::Vga,
        pci_slot: slot.to_string(),
        render_node: Some(format!("/dev/dri/renderD{}", slot.len() + 128)),
        accel_node: None,
        sysfs_path: format!("/sys/class/drm/card{}", slot.len()),
        ..Default::default()
    }
}

fn make_device_with_accel(vendor: GpuVendor, slot: &str) -> GpuDevice {
    GpuDevice {
        vendor,
        class: DeviceClass::Npu,
        pci_slot: slot.to_string(),
        render_node: None,
        accel_node: Some(format!("/dev/accel/accel{}", slot.len())),
        sysfs_path: format!("/sys/class/accel/accel{}", slot.len()),
        ..Default::default()
    }
}

fn nvidia_devices(n: usize) -> Vec<GpuDevice> {
    (0..n)
        .map(|i| make_device(GpuVendor::Nvidia, &format!("0000:{i:02x}:00.0")))
        .collect()
}

// ── mode_dispatch ───────────────────────────────────────────────────

mod mode_dispatch {
    use super::*;

    #[test]
    fn gpuman_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("gpuman".to_string())),
            ModeOfOperation::Daemon
        );
    }

    #[test]
    fn gpuctl_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("gpuctl".to_string())),
            ModeOfOperation::Ctl
        );
    }

    #[test]
    fn unknown_bare() {
        assert_eq!(
            black_box(ModeOfOperation::from("nvidia-smi".to_string())),
            ModeOfOperation::Unknown
        );
    }

    #[test]
    fn empty() {
        assert_eq!(
            black_box(ModeOfOperation::from(String::new())),
            ModeOfOperation::Unknown
        );
    }

    #[test]
    fn full_path() {
        assert_eq!(
            black_box(ModeOfOperation::from("/usr/bin/gpuman".to_string())),
            ModeOfOperation::Unknown
        );
    }
}

// ── vendor_display ──────────────────────────────────────────────────

mod vendor_display {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(black_box(GpuVendor::Nvidia.name()), "nvidia");
        assert_eq!(black_box(GpuVendor::Amd.name()), "amd");
        assert_eq!(black_box(GpuVendor::Intel.name()), "intel");
        assert_eq!(black_box(GpuVendor::Unknown(0x1234).name()), "unknown");
    }

    #[test]
    fn display_known() {
        assert_eq!(black_box(format!("{}", GpuVendor::Nvidia)), "NVIDIA");
        assert_eq!(black_box(format!("{}", GpuVendor::Amd)), "AMD");
        assert_eq!(black_box(format!("{}", GpuVendor::Intel)), "Intel");
    }

    #[test]
    fn display_unknown() {
        let s = black_box(format!("{}", GpuVendor::Unknown(0x1234)));
        assert!(s.contains("1234"));
    }
}

// ── device_class_display ────────────────────────────────────────────

mod device_class_display {
    use super::*;

    #[test]
    fn vga() {
        assert_eq!(black_box(format!("{}", DeviceClass::Vga)), "VGA");
    }

    #[test]
    fn display_controller() {
        assert_eq!(
            black_box(format!("{}", DeviceClass::Display)),
            "3D Controller"
        );
    }

    #[test]
    fn npu() {
        assert_eq!(black_box(format!("{}", DeviceClass::Npu)), "NPU");
    }
}

// ── device_display ──────────────────────────────────────────────────

mod device_display {
    use super::*;

    #[test]
    fn with_render_node() {
        let dev = make_device(GpuVendor::Nvidia, "0000:01:00.0");
        let s = black_box(format!("{dev}"));
        assert!(s.contains("NVIDIA"));
        assert!(s.contains("renderD"));
    }

    #[test]
    fn with_accel_node() {
        let dev = make_device_with_accel(GpuVendor::Intel, "0000:02:00.0");
        let s = black_box(format!("{dev}"));
        assert!(s.contains("Intel"));
        assert!(s.contains("accel"));
    }

    #[test]
    fn bare_device() {
        let dev = GpuDevice {
            vendor: GpuVendor::Amd,
            class: DeviceClass::Vga,
            pci_slot: "0000:03:00.0".into(),
            render_node: None,
            accel_node: None,
            sysfs_path: String::new(),
            ..Default::default()
        };
        let s = black_box(format!("{dev}"));
        assert!(s.contains("AMD"));
        assert!(!s.contains("render"));
        assert!(!s.contains("accel"));
    }
}

// ── vendors_present ─────────────────────────────────────────────────

mod vendors_present_tests {
    use super::*;

    #[test]
    fn empty_list() {
        let v = black_box(vendors_present(&[]));
        assert!(v.is_empty());
    }

    #[test]
    fn single_vendor() {
        let devices = nvidia_devices(3);
        let v = black_box(vendors_present(&devices));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn multi_vendor() {
        let devices = vec![
            make_device(GpuVendor::Nvidia, "0000:01:00.0"),
            make_device(GpuVendor::Amd, "0000:02:00.0"),
            make_device(GpuVendor::Intel, "0000:03:00.0"),
        ];
        let v = black_box(vendors_present(&devices));
        assert_eq!(v.len(), 3);
    }

    #[test]
    fn all_duplicates() {
        let devices = nvidia_devices(8);
        let v = black_box(vendors_present(&devices));
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn unknown_vendor_counted() {
        let devices = vec![
            make_device(GpuVendor::Unknown(0x1234), "0000:01:00.0"),
            make_device(GpuVendor::Nvidia, "0000:02:00.0"),
        ];
        let v = black_box(vendors_present(&devices));
        assert_eq!(v.len(), 2);
    }
}

// ── vendors_present_scaling ─────────────────────────────────────────

mod vendors_present_scaling {
    use super::*;

    #[test]
    fn scaling() {
        for n in [1usize, 4, 8, 16, 32, 64] {
            let devices: Vec<GpuDevice> = (0..n)
                .map(|i| {
                    let vendor = match i % 3 {
                        0 => GpuVendor::Nvidia,
                        1 => GpuVendor::Amd,
                        _ => GpuVendor::Intel,
                    };
                    make_device(vendor, &format!("0000:{i:02x}:00.0"))
                })
                .collect();
            let v = black_box(vendors_present(&devices));
            assert!(v.len() <= 3);
        }
    }
}

// ── build_container_spec ────────────────────────────────────────────

mod build_container_spec_tests {
    use super::*;

    #[test]
    fn nvidia_returns_some() {
        let devices = nvidia_devices(1);
        let opts = HashMap::new();
        let spec = black_box(build_container_spec(
            &GpuVendor::Nvidia,
            &devices,
            &opts,
        ));
        assert!(spec.is_some());
    }

    #[test]
    fn amd_returns_some() {
        let devices = vec![make_device(GpuVendor::Amd, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec =
            black_box(build_container_spec(&GpuVendor::Amd, &devices, &opts));
        assert!(spec.is_some());
    }

    #[test]
    fn intel_returns_some() {
        let devices = vec![make_device(GpuVendor::Intel, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec =
            black_box(build_container_spec(&GpuVendor::Intel, &devices, &opts));
        assert!(spec.is_some());
    }

    #[test]
    fn unknown_vendor_returns_none() {
        let devices =
            vec![make_device(GpuVendor::Unknown(0x9999), "0000:01:00.0")];
        let opts = HashMap::new();
        let spec = black_box(build_container_spec(
            &GpuVendor::Unknown(0x9999),
            &devices,
            &opts,
        ));
        assert!(spec.is_none());
    }

    #[test]
    fn no_matching_devices_returns_none() {
        let devices = vec![make_device(GpuVendor::Amd, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec = black_box(build_container_spec(
            &GpuVendor::Nvidia,
            &devices,
            &opts,
        ));
        assert!(spec.is_none());
    }

    #[test]
    fn empty_devices_returns_none() {
        let opts = HashMap::new();
        let spec =
            black_box(build_container_spec(&GpuVendor::Nvidia, &[], &opts));
        assert!(spec.is_none());
    }
}

// ── build_container_spec_cmdline_override ───────────────────────────

mod build_container_spec_cmdline_override {
    use super::*;

    #[test]
    fn nvidia_image_override() {
        let devices = nvidia_devices(1);
        let mut opts = HashMap::new();
        opts.insert(
            "gpu_nvidia_image".to_string(),
            "custom/cuda:latest".to_string(),
        );
        let spec =
            build_container_spec(&GpuVendor::Nvidia, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.image), "custom/cuda:latest");
    }

    #[test]
    fn amd_image_override() {
        let devices = vec![make_device(GpuVendor::Amd, "0000:01:00.0")];
        let mut opts = HashMap::new();
        opts.insert(
            "gpu_amd_image".to_string(),
            "custom/rocm:latest".to_string(),
        );
        let spec =
            build_container_spec(&GpuVendor::Amd, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.image), "custom/rocm:latest");
    }

    #[test]
    fn intel_image_override() {
        let devices = vec![make_device(GpuVendor::Intel, "0000:01:00.0")];
        let mut opts = HashMap::new();
        opts.insert(
            "gpu_intel_image".to_string(),
            "custom/oneapi:latest".to_string(),
        );
        let spec =
            build_container_spec(&GpuVendor::Intel, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.image), "custom/oneapi:latest");
    }

    #[test]
    fn default_image_when_no_override() {
        let devices = nvidia_devices(1);
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Nvidia, &devices, &opts).unwrap();
        assert!(black_box(&spec.image).contains("nvidia/cuda"));
    }
}

// ── build_container_spec_device_nodes ───────────────────────────────

mod build_container_spec_device_nodes {
    use super::*;

    #[test]
    fn render_nodes_collected() {
        let devices = nvidia_devices(2);
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Nvidia, &devices, &opts).unwrap();
        // Each device contributes one render node.
        assert!(black_box(spec.devices.len()) >= 2);
    }

    #[test]
    fn accel_nodes_collected() {
        let devices =
            vec![make_device_with_accel(GpuVendor::Intel, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Intel, &devices, &opts).unwrap();
        assert!(black_box(&spec.devices).iter().any(|d| d.contains("accel")));
    }

    #[test]
    fn mixed_vendor_filters_correctly() {
        let devices = vec![
            make_device(GpuVendor::Nvidia, "0000:01:00.0"),
            make_device(GpuVendor::Amd, "0000:02:00.0"),
            make_device(GpuVendor::Nvidia, "0000:03:00.0"),
        ];
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Nvidia, &devices, &opts).unwrap();
        // Only 2 NVIDIA devices, so 2 render nodes (plus potential NVIDIA
        // control nodes).
        assert!(black_box(spec.devices.len()) >= 2);
    }
}

// ── container_spec_naming ───────────────────────────────────────────

mod container_spec_naming {
    use super::*;

    #[test]
    fn nvidia_name() {
        let devices = nvidia_devices(1);
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Nvidia, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.name), "gpuman-nvidia");
    }

    #[test]
    fn amd_name() {
        let devices = vec![make_device(GpuVendor::Amd, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Amd, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.name), "gpuman-amd");
    }

    #[test]
    fn intel_name() {
        let devices = vec![make_device(GpuVendor::Intel, "0000:01:00.0")];
        let opts = HashMap::new();
        let spec =
            build_container_spec(&GpuVendor::Intel, &devices, &opts).unwrap();
        assert_eq!(black_box(&spec.name), "gpuman-intel");
    }
}
