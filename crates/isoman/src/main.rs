use std::fs;
use std::path::PathBuf;
use std::process::Command;

use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

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

    // Resolve output to an absolute path before we change context to the staging dir.
    let output = {
        let raw = std::env::var("OUTPUT").unwrap_or_else(|_| "os.iso".to_string());
        let p = PathBuf::from(&raw);
        if p.is_absolute() {
            p
        } else {
            std::env::current_dir().into_diagnostic()?.join(p)
        }
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
    fs::write(
        grub_dir.join("grub.cfg"),
        r#"set default=0
set timeout=5

menuentry "util-mdl" {
    linux  /boot/vmlinuz quiet net.ifnames=0 biosdevname=0
    initrd /boot/initramfs.gz
}

menuentry "util-mdl (serial)" {
    linux  /boot/vmlinuz console=ttyS0 earlyprintk=ttyS0 net.ifnames=0 biosdevname=0
    initrd /boot/initramfs.gz
}
"#,
    )
    .into_diagnostic()?;

    info!("Running grub-mkrescue");
    let out = Command::new("grub-mkrescue")
        .args([
            "-o",
            output.to_str().ok_or_else(|| miette::miette!("output path is not valid UTF-8"))?,
            stage.to_str().ok_or_else(|| miette::miette!("stage path is not valid UTF-8"))?,
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
fn scopeguard(path: &PathBuf) -> impl Drop + use<'_> {
    struct Guard<'a>(&'a PathBuf);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(self.0);
        }
    }
    Guard(path)
}
