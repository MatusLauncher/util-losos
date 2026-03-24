//! Builds bootable hybrid ISO images (BIOS + UEFI) using the
//! [Limine](https://limine-bootloader.org/) bootloader.
//!
//! Optionally, the initramfs can be built first via `podman build` by
//! delegating to `container::build_initramfs` before the ISO assembly step.
//!
//! # Public surface
//!
//! - [`LIMINE_REPO`] — Git URL of the Limine binary release repository.
//! - [`LIMINE_BRANCH`] — Git branch used when cloning Limine.
//! - [`LIMINE_CONF`] — Default `limine.conf` embedded into the ISO image.
//! - [`resolve_output`] — Resolves a user-supplied output path against a base directory.
//! - [`scopeguard`] — RAII helper that removes a staging directory on drop.
use std::fs;
use std::path::{Path, PathBuf};
pub mod schema;
/// Git URL of the Limine bootloader binary release repository.
pub const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";
/// Git branch name for the Limine binary release (e.g. `"v10.x-binary"`).
pub const LIMINE_BRANCH: &str = "v10.x-binary";
/// Default `limine.conf` written into the ISO's `/boot/limine/` directory.
///
/// Defines two boot entries — a silent default entry and a serial-console
/// entry — both booting `boot():/boot/vmlinuz` with
/// `boot():/boot/initramfs.gz` as the initramfs module.
pub const LIMINE_CONF: &str = r#"timeout: 5
default_entry: 1

/LosOS
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: quiet net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz

/LosOS on the serial port
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz
"#;

/// Resolves `raw` as an output path relative to `base`.
///
/// If `raw` is an absolute path it is returned unchanged.
/// Otherwise `raw` is joined onto `base`.
pub fn resolve_output(base: &Path, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { base.join(p) }
}

/// Returns a RAII guard that recursively removes `path` when dropped.
///
/// Used to clean up staging directories on error. The removal is
/// best-effort — failures are silently ignored.
pub fn scopeguard(path: &Path) -> impl Drop + use<'_> {
    struct Guard<'a>(&'a Path);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0);
        }
    }
    Guard(path)
}

#[cfg(test)]
mod tests {
    use crate::schema::ContMode;
    use crate::{LIMINE_BRANCH, LIMINE_CONF, LIMINE_REPO, resolve_output, scopeguard};
    use cluman::schemas::Mode;
    use std::path::PathBuf;
    use tempfile::TempDir;

    // ── scopeguard ────────────────────────────────────────────────────────────

    #[test]
    fn scopeguard_removes_directory_on_drop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("staging");
        std::fs::create_dir_all(&path).unwrap();
        assert!(path.is_dir(), "directory should exist before drop");
        {
            let _guard = scopeguard(&path);
        }
        assert!(!path.exists(), "directory should be removed after drop");
    }

    #[test]
    fn scopeguard_is_noop_when_path_does_not_exist() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("nonexistent");
        assert!(!path.exists());
        {
            let _guard = scopeguard(&path);
        }
        assert!(!path.exists());
    }

    // ── LIMINE_CONF ───────────────────────────────────────────────────────────

    #[test]
    fn limine_conf_contains_default_entry() {
        assert!(LIMINE_CONF.contains("/LosOS\n"));
    }

    #[test]
    fn limine_conf_contains_serial_entry() {
        assert!(LIMINE_CONF.contains("/LosOS on the serial port\n"));
    }

    #[test]
    fn limine_conf_has_two_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.starts_with('/'))
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn limine_conf_uses_linux_protocol_for_all_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.trim() == "protocol: linux")
            .count();
        assert_eq!(count, 2);
    }

    #[test]
    fn limine_conf_references_kernel_path() {
        assert!(LIMINE_CONF.contains("path: boot():/boot/vmlinuz"));
    }

    #[test]
    fn limine_conf_references_initramfs_path() {
        assert!(LIMINE_CONF.contains("module_path: boot():/boot/initramfs.gz"));
    }

    #[test]
    fn limine_conf_serial_entry_has_console_cmdline() {
        let lines: Vec<&str> = LIMINE_CONF.lines().collect();
        let serial_pos = lines
            .iter()
            .position(|l| *l == "/LosOS on the serial port")
            .expect("serial entry must exist");
        let serial_section: String = lines[serial_pos..]
            .iter()
            .take_while(|l| !l.starts_with('/') || l.contains("serial"))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(serial_section.contains("console=ttyS0"));
        assert!(serial_section.contains("earlyprintk=ttyS0"));
    }

    #[test]
    fn limine_conf_has_timeout_setting() {
        assert!(LIMINE_CONF.lines().any(|l: &str| l.starts_with("timeout:")));
    }

    #[test]
    fn limine_repo_is_github_https_url() {
        assert!(LIMINE_REPO.starts_with("https://github.com/"));
    }

    #[test]
    fn limine_branch_is_binary_release() {
        assert!(LIMINE_BRANCH.ends_with("-binary"));
    }

    // ── resolve_output ────────────────────────────────────────────────────────

    #[test]
    fn absolute_output_path_is_kept_as_is() {
        let base = PathBuf::from("/some/cwd");
        assert_eq!(
            resolve_output(&base, "/tmp/my.iso"),
            PathBuf::from("/tmp/my.iso")
        );
    }

    #[test]
    fn relative_output_path_is_joined_to_base() {
        let base = PathBuf::from("/some/cwd");
        assert_eq!(
            resolve_output(&base, "my.iso"),
            PathBuf::from("/some/cwd/my.iso")
        );
    }

    #[test]
    fn relative_output_with_subdir_is_joined_to_base() {
        let base = PathBuf::from("/build");
        assert_eq!(
            resolve_output(&base, "out/images/disk.iso"),
            PathBuf::from("/build/out/images/disk.iso")
        );
    }

    // ── ContMode ──────────────────────────────────────────────────────────────

    #[test]
    fn contmode_default_bakes_in_default_mode() {
        let cm = ContMode::new();
        let out = cm.return_final_contf();
        let default_mode = Mode::default().to_string();
        assert!(
            out.contains(&format!("ARG MODE={default_mode}")),
            "expected ARG MODE={default_mode} in rendered Containerfile"
        );
    }

    #[test]
    fn contmode_no_bare_arg_mode_in_rendered_output() {
        // After rendering, every occurrence of ARG MODE must carry a value.
        let cm = ContMode::new();
        let out = cm.return_final_contf();
        for line in out.lines() {
            if line.trim().starts_with("ARG MODE") {
                assert!(
                    line.contains('='),
                    "bare 'ARG MODE' without a default found: {line:?}"
                );
            }
        }
    }

    #[test]
    fn contmode_set_mode_client_bakes_in_client() {
        let mut cm = ContMode::new();
        cm.set_mode(Mode::Client);
        let out = cm.return_final_contf();
        assert!(out.contains("ARG MODE=client"));
    }

    #[test]
    fn contmode_set_mode_server_bakes_in_server() {
        let mut cm = ContMode::new();
        cm.set_mode(Mode::Server);
        let out = cm.return_final_contf();
        assert!(out.contains("ARG MODE=server"));
    }

    #[test]
    fn contmode_set_mode_controller_bakes_in_controller() {
        let mut cm = ContMode::new();
        cm.set_mode(Mode::Controller);
        let out = cm.return_final_contf();
        assert!(out.contains("ARG MODE=controller"));
    }

    #[test]
    fn contmode_rendered_containerfile_starts_with_from() {
        let cm = ContMode::new();
        let out = cm.return_final_contf();
        assert!(
            out.lines().any(|l| l.starts_with("FROM ")),
            "rendered Containerfile must contain at least one FROM instruction"
        );
    }

    #[test]
    fn contmode_rendered_containerfile_ends_with_scratch_stage() {
        let cm = ContMode::new();
        let out = cm.return_final_contf();
        assert!(
            out.contains("FROM scratch"),
            "rendered Containerfile must end with a FROM scratch export stage"
        );
    }

    #[test]
    fn contmode_rendered_containerfile_exports_initramfs_artifact() {
        let cm = ContMode::new();
        let out = cm.return_final_contf();
        assert!(out.contains("os.initramfs.tar.gz"));
    }
}
