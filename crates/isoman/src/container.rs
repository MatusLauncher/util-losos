use std::fs::write;
use std::process::Command;
use std::{env::temp_dir, path::Path};

use cluman::schemas::Mode;
use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

use isoman::schema::ContMode;

/// Build the initramfs container image for the given `mode` and extract
/// `os.initramfs.tar.gz` from it into `output`.
///
/// Steps:
/// 1. `podman build --build-arg MODE=<mode> --no-cache -t <tag> -f <containerfile> <context>`
/// 2. `podman create <tag>` → container ID
/// 3. `podman cp <id>:/os.initramfs.tar.gz <output>`
/// 4. `podman rm <id>`
pub fn build_initramfs(
    context: &Path,
    mode: &Mode,
    output: &Path,
    cache: bool,
) -> miette::Result<()> {
    let cf_path = temp_dir().join("contf");
    let mut cmode = ContMode::new();
    let m = cmode.set_mode(*mode);
    write(&cf_path, m.return_final_contf()).into_diagnostic()?;
    let mode_str = mode.to_string();
    let tag = format!("util-mdl-build-{mode_str}");

    let containerfile_str = cf_path
        .to_str()
        .ok_or_else(|| miette::miette!("Containerfile path is not valid UTF-8"))?;
    let context_str = context
        .to_str()
        .ok_or_else(|| miette::miette!("context path is not valid UTF-8"))?;
    let output_str = output
        .to_str()
        .ok_or_else(|| miette::miette!("output path is not valid UTF-8"))?;

    // ── 1. Build the image ────────────────────────────────────────────────────
    let mode = format!("MODE={mode_str}");
    info!(%mode_str, %tag, "Building container image");
    let args = if cache {
        vec![
            "build",
            "--build-arg",
            &mode,
            "-t",
            &tag,
            "-f",
            containerfile_str,
            context_str,
        ]
    } else {
        vec![
            "build",
            "--no-cache",
            "--build-arg",
            &mode,
            "-t",
            &tag,
            "-f",
            containerfile_str,
            context_str,
        ]
    };
    let build_status = Command::new("podman")
        .args(args)
        // Inherit stdio so build progress is visible to the user.
        .status()
        .into_diagnostic()
        .wrap_err("podman not found; install podman")?;

    if !build_status.success() {
        bail!("podman build failed (exit {})", build_status);
    }

    // ── 2. Create a container so we can copy files out ────────────────────────

    info!(%tag, "Creating ephemeral container to extract initramfs");

    let create_out = Command::new("podman")
        .args(["create", &tag])
        .output()
        .into_diagnostic()
        .wrap_err("podman create failed")?;

    if !create_out.status.success() {
        bail!(
            "podman create failed (exit {}): {}",
            create_out.status,
            String::from_utf8_lossy(&create_out.stderr)
        );
    }

    let container_id = String::from_utf8_lossy(&create_out.stdout)
        .trim()
        .to_string();

    info!(%container_id, "Extracting os.initramfs.tar.gz");

    // ── 3. Copy the initramfs out of the container ────────────────────────────

    let cp_out = Command::new("podman")
        .args([
            "cp",
            &format!("{container_id}:/os.initramfs.tar.gz"),
            output_str,
        ])
        .output()
        .into_diagnostic()
        .wrap_err("podman cp failed")?;

    // Always remove the container, even if the copy failed.
    let rm_out = Command::new("podman")
        .args(["rm", &container_id])
        .output()
        .into_diagnostic()
        .wrap_err("podman rm failed")?;

    if !rm_out.status.success() {
        // Non-fatal — warn but don't abort.
        tracing::warn!(
            %container_id,
            stderr = %String::from_utf8_lossy(&rm_out.stderr),
            "podman rm returned non-zero exit code"
        );
    }

    if !cp_out.status.success() {
        bail!(
            "podman cp failed (exit {}): {}",
            cp_out.status,
            String::from_utf8_lossy(&cp_out.stderr)
        );
    }

    info!(output = %output.display(), "Initramfs extracted");
    Ok(())
}
