use std::fs;
use std::path::PathBuf;
use std::process::Command;

use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

/// URL of the Limine binary release repository (GitHub mirror of Codeberg).
pub(crate) const LIMINE_REPO: &str = "https://github.com/limine-bootloader/limine.git";

/// Git branch to clone for the pre-built binary release.
pub(crate) const LIMINE_BRANCH: &str = "v10.x-binary";

/// The limine.conf written into every ISO image produced by isoman.
///
/// Uses the Limine v10.x configuration format:
/// - `/title` lines open a boot entry.
/// - `protocol: linux` selects the Linux boot protocol.
/// - `path:` points to the kernel inside the ISO.
/// - `cmdline:` passes kernel command-line arguments.
/// - `module_path:` loads the initramfs.
///
/// Two entries are provided: a silent default boot and a serial-console
/// variant useful for headless debugging.
pub(crate) const LIMINE_CONF: &str = r#"timeout: 5
default_entry: 1

/util-mdl
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: quiet net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz

/util-mdl (serial)
    protocol: linux
    path: boot():/boot/vmlinuz
    cmdline: console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0
    module_path: boot():/boot/initramfs.gz
"#;

/// Resolve `raw` to an absolute output path.
///
/// * If `raw` is already absolute it is returned as-is.
/// * Otherwise it is joined to `base` (usually the current working directory).
pub(crate) fn resolve_output(base: &PathBuf, raw: &str) -> PathBuf {
    let p = PathBuf::from(raw);
    if p.is_absolute() { p } else { base.join(p) }
}

fn main() -> miette::Result<()> {
    tracing_subscriber::fmt::init();

    let kernel = PathBuf::from(std::env::var("KERNEL").unwrap_or_else(|_| {
        format!(
            "/boot/vmlinuz-{}",
            Command::new("uname")
                .arg("-r")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default()
        )
    }));

    let initramfs = PathBuf::from(
        std::env::var("INITRAMFS").unwrap_or_else(|_| "os.initramfs.tar.gz".to_string()),
    );

    // Resolve output to an absolute path before we enter the staging dir.
    let output = {
        let raw = std::env::var("OUTPUT").unwrap_or_else(|_| "os.iso".to_string());
        let cwd = std::env::current_dir().into_diagnostic()?;
        resolve_output(&cwd, &raw)
    };

    info!(
        kernel    = %kernel.display(),
        initramfs = %initramfs.display(),
        output    = %output.display(),
        "Starting isoman (Limine)"
    );

    let stage = std::env::temp_dir().join(format!("isoman-{}", std::process::id()));

    // Clean up staging dir on exit regardless of outcome.
    let _cleanup = scopeguard(&stage);

    // ── Clone Limine binary release ───────────────────────────────────────────

    let limine_dir = stage.join("limine-bin");

    info!(branch = LIMINE_BRANCH, "Cloning Limine binary release");
    let clone_out = Command::new("git")
        .args([
            "clone",
            "--branch",
            LIMINE_BRANCH,
            "--depth",
            "1",
            LIMINE_REPO,
            limine_dir
                .to_str()
                .ok_or_else(|| miette::miette!("limine_dir path is not valid UTF-8"))?,
        ])
        .output()
        .into_diagnostic()
        .wrap_err("git not found; install git")?;

    if !clone_out.status.success() {
        bail!(
            "git clone failed (exit {}): {}",
            clone_out.status,
            String::from_utf8_lossy(&clone_out.stderr)
        );
    }

    // ── Build the limine host utility ─────────────────────────────────────────

    info!("Building limine host tool");
    let make_out = Command::new("make")
        .current_dir(&limine_dir)
        .output()
        .into_diagnostic()
        .wrap_err("make not found; install make")?;

    if !make_out.status.success() {
        bail!(
            "make failed (exit {}): {}",
            make_out.status,
            String::from_utf8_lossy(&make_out.stderr)
        );
    }

    // ── Assemble the ISO staging tree ─────────────────────────────────────────

    let iso_root = stage.join("iso-root");
    let boot_limine = iso_root.join("boot").join("limine");
    let efi_boot = iso_root.join("EFI").join("BOOT");

    fs::create_dir_all(&boot_limine).into_diagnostic()?;
    fs::create_dir_all(&efi_boot).into_diagnostic()?;

    info!("Copying kernel");
    fs::copy(&kernel, iso_root.join("boot").join("vmlinuz")).into_diagnostic()?;

    info!("Copying initramfs");
    fs::copy(&initramfs, iso_root.join("boot").join("initramfs.gz")).into_diagnostic()?;

    info!("Writing limine.conf");
    fs::write(boot_limine.join("limine.conf"), LIMINE_CONF).into_diagnostic()?;

    info!("Copying Limine boot files");
    for filename in &[
        "limine-bios.sys",
        "limine-bios-cd.bin",
        "limine-uefi-cd.bin",
    ] {
        fs::copy(limine_dir.join(filename), boot_limine.join(filename))
            .into_diagnostic()
            .wrap_err_with(|| format!("failed to copy Limine file: {filename}"))?;
    }

    fs::copy(limine_dir.join("BOOTX64.EFI"), efi_boot.join("BOOTX64.EFI"))
        .into_diagnostic()
        .wrap_err("failed to copy BOOTX64.EFI")?;

    // ── Create the hybrid ISO with xorriso ────────────────────────────────────

    let output_str = output
        .to_str()
        .ok_or_else(|| miette::miette!("output path is not valid UTF-8"))?;
    let iso_root_str = iso_root
        .to_str()
        .ok_or_else(|| miette::miette!("iso_root path is not valid UTF-8"))?;

    info!("Running xorriso");
    let xorriso_out = Command::new("xorriso")
        .args([
            "-as",
            "mkisofs",
            "-R",
            "-r",
            "-J",
            "-b",
            "boot/limine/limine-bios-cd.bin",
            "-no-emul-boot",
            "-boot-load-size",
            "4",
            "-boot-info-table",
            "-hfsplus",
            "-apm-block-size",
            "2048",
            "--efi-boot",
            "boot/limine/limine-uefi-cd.bin",
            "-efi-boot-part",
            "--efi-boot-image",
            "--protective-msdos-label",
            iso_root_str,
            "-o",
            output_str,
        ])
        .output()
        .into_diagnostic()
        .wrap_err("xorriso not found; install xorriso")?;

    if !xorriso_out.status.success() {
        bail!(
            "xorriso failed (exit {}): {}",
            xorriso_out.status,
            String::from_utf8_lossy(&xorriso_out.stderr)
        );
    }

    // ── Install Limine BIOS boot sectors into the ISO ─────────────────────────

    info!("Running limine bios-install");
    let limine_bin = limine_dir.join("limine");
    let bios_install_out = Command::new(&limine_bin)
        .args(["bios-install", output_str])
        .output()
        .into_diagnostic()
        .wrap_err("failed to run limine bios-install")?;

    if !bios_install_out.status.success() {
        bail!(
            "limine bios-install failed (exit {}): {}",
            bios_install_out.status,
            String::from_utf8_lossy(&bios_install_out.stderr)
        );
    }

    info!(output = %output.display(), "ISO written");
    Ok(())
}

/// Returns a guard that removes the given path when dropped.
///
/// Uses `fs::remove_dir_all`, so a missing path is silently ignored
/// (the `Err` from `remove_dir_all` is discarded via `let _`).
pub(crate) fn scopeguard(path: &PathBuf) -> impl Drop + use<'_> {
    struct Guard<'a>(&'a PathBuf);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0);
        }
    }
    Guard(path)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tempfile::TempDir;

    use super::{LIMINE_BRANCH, LIMINE_CONF, LIMINE_REPO, resolve_output, scopeguard};

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
        // Must not panic even though the path is absent.
        {
            let _guard = scopeguard(&path);
        }
        assert!(!path.exists());
    }

    // ── LIMINE_CONF content ───────────────────────────────────────────────────

    #[test]
    fn limine_conf_contains_default_entry() {
        assert!(
            LIMINE_CONF.contains("/util-mdl\n"),
            "LIMINE_CONF must contain the default 'util-mdl' entry"
        );
    }

    #[test]
    fn limine_conf_contains_serial_entry() {
        assert!(
            LIMINE_CONF.contains("/util-mdl (serial)\n"),
            "LIMINE_CONF must contain the 'util-mdl (serial)' entry"
        );
    }

    #[test]
    fn limine_conf_has_two_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.starts_with('/'))
            .count();
        assert_eq!(count, 2, "expected exactly 2 entry lines, found {count}");
    }

    #[test]
    fn limine_conf_uses_linux_protocol_for_all_entries() {
        let count = LIMINE_CONF
            .lines()
            .filter(|l: &&str| l.trim() == "protocol: linux")
            .count();
        assert_eq!(
            count, 2,
            "expected exactly 2 'protocol: linux' lines, found {count}"
        );
    }

    #[test]
    fn limine_conf_references_kernel_path() {
        assert!(
            LIMINE_CONF.contains("path: boot():/boot/vmlinuz"),
            "LIMINE_CONF must reference the kernel at boot():/boot/vmlinuz"
        );
    }

    #[test]
    fn limine_conf_references_initramfs_path() {
        assert!(
            LIMINE_CONF.contains("module_path: boot():/boot/initramfs.gz"),
            "LIMINE_CONF must reference the initramfs at boot():/boot/initramfs.gz"
        );
    }

    #[test]
    fn limine_conf_serial_entry_has_console_cmdline() {
        let lines: Vec<&str> = LIMINE_CONF.lines().collect();
        let serial_pos = lines
            .iter()
            .position(|l| *l == "/util-mdl (serial)")
            .expect("serial entry must exist in LIMINE_CONF");
        // Collect all lines belonging to the serial entry (until EOF or the
        // next entry header that is not the serial one).
        let serial_section: String = lines[serial_pos..]
            .iter()
            .take_while(|l| !l.starts_with('/') || l.contains("serial"))
            .cloned()
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            serial_section.contains("console=ttyS0"),
            "serial entry cmdline must contain console=ttyS0"
        );
        assert!(
            serial_section.contains("earlyprintk=ttyS0"),
            "serial entry cmdline must contain earlyprintk=ttyS0"
        );
    }

    #[test]
    fn limine_conf_has_timeout_setting() {
        assert!(
            LIMINE_CONF.lines().any(|l: &str| l.starts_with("timeout:")),
            "LIMINE_CONF must have a global 'timeout:' option"
        );
    }

    // ── Limine repo constants ─────────────────────────────────────────────────

    #[test]
    fn limine_repo_is_github_https_url() {
        assert!(
            LIMINE_REPO.starts_with("https://github.com/"),
            "LIMINE_REPO must be a GitHub HTTPS URL, got: {LIMINE_REPO}"
        );
    }

    #[test]
    fn limine_branch_is_binary_release() {
        assert!(
            LIMINE_BRANCH.ends_with("-binary"),
            "LIMINE_BRANCH must be a binary release branch (ending with '-binary'), \
             got: {LIMINE_BRANCH}"
        );
    }

    // ── resolve_output ────────────────────────────────────────────────────────

    #[test]
    fn absolute_output_path_is_kept_as_is() {
        let base = PathBuf::from("/some/cwd");
        let result = resolve_output(&base, "/tmp/my.iso");
        assert_eq!(result, PathBuf::from("/tmp/my.iso"));
    }

    #[test]
    fn relative_output_path_is_joined_to_base() {
        let base = PathBuf::from("/some/cwd");
        let result = resolve_output(&base, "my.iso");
        assert_eq!(result, PathBuf::from("/some/cwd/my.iso"));
    }

    #[test]
    fn relative_output_with_subdir_is_joined_to_base() {
        let base = PathBuf::from("/build");
        let result = resolve_output(&base, "out/images/disk.iso");
        assert_eq!(result, PathBuf::from("/build/out/images/disk.iso"));
    }
}
