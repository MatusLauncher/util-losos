use std::fs;
use std::path::PathBuf;
use std::process::Command;

use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

/// The grub.cfg written into every ISO image produced by isoman.
///
/// Contains two menu entries: a silent default boot and a serial-console
/// variant useful for headless debugging.
pub(crate) const GRUB_CFG: &str = r#"set default=0
set timeout=5

menuentry "util-mdl" {
    linux  /boot/vmlinuz quiet net.ifnames=0 biosdevname=0
    initrd /boot/initramfs.gz
}

menuentry "util-mdl (serial)" {
    linux  /boot/vmlinuz console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0
    initrd /boot/initramfs.gz
}
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
    // NOTE: main() delegates to the extracted helpers above so that the core
    // logic remains unit-testable without spawning processes.
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

    // Resolve output to an absolute path before we change context to the staging dir.
    let output = {
        let raw = std::env::var("OUTPUT").unwrap_or_else(|_| "os.iso".to_string());
        let cwd = std::env::current_dir().into_diagnostic()?;
        resolve_output(&cwd, &raw)
    };

    info!(
        kernel = %kernel.display(),
        initramfs = %initramfs.display(),
        output = %output.display(),
        "Starting isoman"
    );

    let stage = std::env::temp_dir().join(format!("isoman-{}", std::process::id()));

    // Clean up staging dir on exit regardless of outcome.
    let _cleanup = scopeguard(&stage);

    let grub_dir = stage.join("boot").join("grub");
    fs::create_dir_all(&grub_dir).into_diagnostic()?;

    info!("Copying kernel");
    fs::copy(&kernel, stage.join("boot").join("vmlinuz")).into_diagnostic()?;

    info!("Copying initramfs");
    fs::copy(&initramfs, stage.join("boot").join("initramfs.gz")).into_diagnostic()?;

    info!("Writing grub.cfg");
    fs::write(grub_dir.join("grub.cfg"), GRUB_CFG).into_diagnostic()?;

    info!("Running grub-mkrescue");
    let out = Command::new("grub-mkrescue")
        .args([
            "-o",
            output
                .to_str()
                .ok_or_else(|| miette::miette!("output path is not valid UTF-8"))?,
            stage
                .to_str()
                .ok_or_else(|| miette::miette!("stage path is not valid UTF-8"))?,
        ])
        .output()
        .into_diagnostic()
        .wrap_err("grub-mkrescue not found; install grub2-common")?;

    if !out.status.success() {
        bail!(
            "grub-mkrescue failed (exit {}): {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
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

    use super::{GRUB_CFG, resolve_output, scopeguard};

    // ── scopeguard ────────────────────────────────────────────────────────────

    #[test]
    fn scopeguard_removes_directory_on_drop() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("staging");
        std::fs::create_dir_all(&path).unwrap();
        assert!(path.is_dir(), "directory should exist before drop");
        {
            let _guard = scopeguard(&path);
            // guard dropped here
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

    // ── GRUB_CFG content ──────────────────────────────────────────────────────

    #[test]
    fn grub_cfg_contains_default_menuentry() {
        assert!(
            GRUB_CFG.contains("menuentry \"util-mdl\""),
            "GRUB_CFG must contain the default 'util-mdl' menuentry"
        );
    }

    #[test]
    fn grub_cfg_contains_serial_menuentry() {
        assert!(
            GRUB_CFG.contains("menuentry \"util-mdl (serial)\""),
            "GRUB_CFG must contain the serial 'util-mdl (serial)' menuentry"
        );
    }

    #[test]
    fn grub_cfg_has_two_menuentry_lines() {
        let count = GRUB_CFG
            .lines()
            .filter(|l| l.starts_with("menuentry"))
            .count();
        assert_eq!(
            count, 2,
            "expected exactly 2 menuentry lines, found {count}"
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
