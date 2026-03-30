//! GSI (Generic System Image) builder — produces Android-compatible boot images
//! for flashing via Fastboot or Samsung Odin.
//!
//! # Artifacts
//!
//! | Format   | Tool       | Output                  |
//! |----------|------------|-------------------------|
//! | Fastboot | `fastboot` | `boot.img`              |
//! | Odin     | Odin/heimdall | `AP_losos.tar.md5`   |
//!
//! Both formats share a common `boot.img` built by [`build_boot_img`] using
//! `mkbootimg`.  The Odin variant additionally wraps the image in a
//! `.tar.md5` archive as expected by Samsung's Odin flash tool.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use isoman::GSI_CMDLINE;
use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

/// Build an Android boot image from a kernel and initramfs using `mkbootimg`.
///
/// The image is written to `<stage>/boot.img` and its path is returned.
///
/// # Errors
///
/// Returns a [`miette::Report`] if `mkbootimg` is not found on `PATH` or exits
/// with a non-zero status code.
fn build_boot_img(kernel: &Path, initramfs: &Path, stage: &Path) -> miette::Result<PathBuf> {
    let output = stage.join("boot.img");

    let kernel_str = kernel
        .to_str()
        .ok_or_else(|| miette::miette!("kernel path is not valid UTF-8"))?;
    let initramfs_str = initramfs
        .to_str()
        .ok_or_else(|| miette::miette!("initramfs path is not valid UTF-8"))?;
    let output_str = output
        .to_str()
        .ok_or_else(|| miette::miette!("boot.img output path is not valid UTF-8"))?;

    info!(kernel = %kernel.display(), initramfs = %initramfs.display(), "Building boot.img with mkbootimg");

    let mkbootimg_out = Command::new("mkbootimg")
        .args([
            "--kernel",
            kernel_str,
            "--ramdisk",
            initramfs_str,
            "--cmdline",
            GSI_CMDLINE,
            "--output",
            output_str,
        ])
        .output()
        .into_diagnostic()
        .wrap_err("mkbootimg not found; install mkbootimg")?;

    if !mkbootimg_out.status.success() {
        bail!(
            "mkbootimg failed (exit {}): {}",
            mkbootimg_out.status,
            String::from_utf8_lossy(&mkbootimg_out.stderr)
        );
    }

    info!(output = %output.display(), "boot.img written");
    Ok(output)
}

/// Produce a Fastboot-compatible GSI: a raw `boot.img` copied to `output`.
///
/// The `boot.img` is built from `kernel` and `initramfs` via [`build_boot_img`]
/// and then copied to `output`.
///
/// # Errors
///
/// Returns a [`miette::Report`] if `mkbootimg` fails or the copy fails.
pub(crate) fn build_gsi_fastboot(
    kernel: &Path,
    initramfs: &Path,
    output: &Path,
    stage: &Path,
) -> miette::Result<()> {
    let boot_img = build_boot_img(kernel, initramfs, stage)?;

    info!(output = %output.display(), "Copying boot.img to fastboot output");
    fs::copy(&boot_img, output)
        .into_diagnostic()
        .wrap_err("failed to copy boot.img to fastboot output")?;

    info!(output = %output.display(), "Fastboot GSI written");
    Ok(())
}

/// Produce an Odin-compatible GSI: a `.tar.md5` archive wrapping `boot.img`.
///
/// Steps:
/// 1. Build `boot.img` via [`build_boot_img`].
/// 2. Create a tar archive containing `boot.img` (`tar -cf`).
/// 3. Append the MD5 checksum of the archive to itself (`md5sum >> archive`).
/// 4. Copy the result to `output`.
///
/// The resulting file can be flashed with Samsung Odin or `heimdall`.
///
/// # Errors
///
/// Returns a [`miette::Report`] if `mkbootimg`, `tar`, or `md5sum` fail.
pub(crate) fn build_gsi_odin(
    kernel: &Path,
    initramfs: &Path,
    output: &Path,
    stage: &Path,
) -> miette::Result<()> {
    let boot_img = build_boot_img(kernel, initramfs, stage)?;
    let tar_path = stage.join("AP_losos.tar");

    let boot_img_str = boot_img
        .to_str()
        .ok_or_else(|| miette::miette!("boot.img path is not valid UTF-8"))?;
    let tar_str = tar_path
        .to_str()
        .ok_or_else(|| miette::miette!("tar path is not valid UTF-8"))?;

    // ── Create tar archive ────────────────────────────────────────────────────

    info!("Creating Odin tar archive");
    let tar_out = Command::new("tar")
        .args(["-cf", tar_str, "-C", stage.to_str().unwrap(), "boot.img"])
        .output()
        .into_diagnostic()
        .wrap_err("tar not found; install tar")?;

    if !tar_out.status.success() {
        bail!(
            "tar failed (exit {}): {}",
            tar_out.status,
            String::from_utf8_lossy(&tar_out.stderr)
        );
    }

    // Suppress "boot.img" unused-variable warning from above
    let _ = boot_img_str;

    // ── Append MD5 checksum to the archive ────────────────────────────────────
    //
    // Odin expects the MD5 of the tar appended as a trailing line in the format:
    //   <hash>  <filename>\n
    // This is exactly what `md5sum <file> >> <file>` produces.

    info!("Appending MD5 checksum to Odin archive");
    let md5_out = Command::new("md5sum")
        .arg(tar_str)
        .output()
        .into_diagnostic()
        .wrap_err("md5sum not found")?;

    if !md5_out.status.success() {
        bail!(
            "md5sum failed (exit {}): {}",
            md5_out.status,
            String::from_utf8_lossy(&md5_out.stderr)
        );
    }

    let md5_line = String::from_utf8_lossy(&md5_out.stdout);
    use std::io::Write as _;
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&tar_path)
        .into_diagnostic()
        .wrap_err("failed to open tar archive for MD5 append")?;
    f.write_all(md5_line.as_bytes())
        .into_diagnostic()
        .wrap_err("failed to append MD5 to archive")?;

    // ── Copy to final output path ─────────────────────────────────────────────

    info!(output = %output.display(), "Copying Odin archive to output");
    fs::copy(&tar_path, output)
        .into_diagnostic()
        .wrap_err("failed to copy Odin archive to output")?;

    info!(output = %output.display(), "Odin GSI written");
    Ok(())
}
