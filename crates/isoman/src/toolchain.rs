//! Zig-based cross-compilation toolchain for building `perman` as a musl cdylib.
//!
//! # Steps orchestrated by [`build_perman`]
//!
//! 1. [`ensure_zig`]          — downloads and caches the Zig compiler under
//!                              `~/.cache/isoman/zig/<version>/`.
//! 2. [`write_zigcc_wrapper`] — writes a `zigcc` shell script that forwards to
//!                              `zig cc -target x86_64-linux-musl`.
//! 3. [`ensure_musl_target`]  — runs `rustup target add x86_64-unknown-linux-musl`.
//! 4. [`run_cargo_build`]     — runs `cargo build -p perman --release
//!                              --target x86_64-unknown-linux-musl` with the
//!                              appropriate `CARGO_TARGET_*` and `CC_*` env vars.
//! 5. Copies `libperman.so` to the caller-specified output path.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use miette::{Context, IntoDiagnostic, bail};
use tracing::info;

/// Build `perman` as a musl cdylib and copy the resulting shared library to `out`.
///
/// # Parameters
/// - `perman_workspace` — root of the `util-mdl-user` Cargo workspace that
///   contains the `perman` crate member.
/// - `out`             — destination path for `libperman.so`.
/// - `zig_version`     — Zig release tag (e.g. `"0.13.0"`).
pub fn build_perman(
    perman_workspace: &Path,
    out: &Path,
    zig_version: &str,
) -> miette::Result<()> {
    let home = std::env::var("HOME")
        .into_diagnostic()
        .wrap_err("HOME env-var is not set")?;
    let cache_dir = PathBuf::from(home).join(".cache/isoman/zig");

    let zig_bin = ensure_zig(&cache_dir, zig_version)?;
    let zigcc = cache_dir.join(zig_version).join("zigcc");
    write_zigcc_wrapper(&zigcc, &zig_bin)?;
    ensure_musl_target()?;
    run_cargo_build(perman_workspace, &zigcc)?;

    let so_src = perman_workspace
        .join("target/x86_64-unknown-linux-musl/release/libperman.so");
    fs::copy(&so_src, out)
        .into_diagnostic()
        .wrap_err_with(|| {
            format!(
                "failed to copy libperman.so from {} — \
                 verify perman has crate-type = [\"cdylib\"] and the build succeeded",
                so_src.display()
            )
        })?;

    info!(output = %out.display(), "libperman.so written");
    Ok(())
}

/// Download and extract the Zig compiler into
/// `<cache_dir>/<version>/zig-linux-x86_64-<version>/`.
///
/// Returns the path to the `zig` binary.  If the binary already exists the
/// download is skipped entirely (cache hit).
fn ensure_zig(cache_dir: &Path, version: &str) -> miette::Result<PathBuf> {
    let versioned_dir = cache_dir.join(version);
    let extract_dir = versioned_dir.join(format!("zig-linux-x86_64-{version}"));
    let zig_bin = extract_dir.join("zig");

    if zig_bin.is_file() {
        info!(version, "Zig already cached, skipping download");
        return Ok(zig_bin);
    }

    fs::create_dir_all(&versioned_dir)
        .into_diagnostic()
        .wrap_err("failed to create Zig cache directory")?;

    let tarball_name = format!("zig-linux-x86_64-{version}.tar.xz");
    let tarball = versioned_dir.join(&tarball_name);
    let url = format!(
        "https://ziglang.org/download/{version}/zig-linux-x86_64-{version}.tar.xz"
    );

    info!(version, %url, "Downloading Zig");
    let status = Command::new("curl")
        .args(["-L", "-o", tarball.to_str().unwrap_or_default(), url.as_str()])
        .status()
        .into_diagnostic()
        .wrap_err("curl not found; install curl to download the Zig toolchain")?;
    if !status.success() {
        bail!("curl exited with {status} while downloading Zig {version}");
    }

    info!("Extracting Zig tarball");
    let status = Command::new("tar")
        .args([
            "-xJf",
            tarball.to_str().unwrap_or_default(),
            "-C",
            versioned_dir.to_str().unwrap_or_default(),
        ])
        .status()
        .into_diagnostic()
        .wrap_err("tar not found; install tar to extract the Zig toolchain")?;
    if !status.success() {
        bail!("tar exited with {status} while extracting Zig {version}");
    }

    Ok(zig_bin)
}

/// Write a `zigcc` wrapper shell script at `wrapper_path` and make it executable.
///
/// The script forwards all arguments to `zig cc -target x86_64-linux-musl`,
/// letting Cargo use it as the MUSL C compiler and linker.
fn write_zigcc_wrapper(wrapper_path: &Path, zig_bin: &Path) -> miette::Result<()> {
    let zig_str = zig_bin
        .to_str()
        .ok_or_else(|| miette::miette!("zig binary path is not valid UTF-8"))?;
    let script = format!("#!/bin/sh\nexec \"{zig_str}\" cc -target x86_64-linux-musl \"$@\"\n");

    fs::write(wrapper_path, &script)
        .into_diagnostic()
        .wrap_err_with(|| format!("failed to write zigcc wrapper to {}", wrapper_path.display()))?;

    fs::set_permissions(wrapper_path, fs::Permissions::from_mode(0o755))
        .into_diagnostic()
        .wrap_err("failed to chmod zigcc wrapper")?;

    info!(path = %wrapper_path.display(), "zigcc wrapper written");
    Ok(())
}

/// Ensure the `x86_64-unknown-linux-musl` rustup target is installed.
///
/// `rustup target add` is idempotent — it exits 0 whether the target was
/// already present or was freshly installed.
fn ensure_musl_target() -> miette::Result<()> {
    info!("Ensuring rustup target x86_64-unknown-linux-musl");
    let status = Command::new("rustup")
        .args(["target", "add", "x86_64-unknown-linux-musl"])
        .status()
        .into_diagnostic()
        .wrap_err("rustup not found; install rustup to manage Rust targets")?;
    if !status.success() {
        bail!("rustup target add failed with {status}");
    }
    Ok(())
}

/// Run `cargo build -p perman --release --target x86_64-unknown-linux-musl`
/// in `workspace` with `zigcc` as both the MUSL linker and C compiler.
fn run_cargo_build(workspace: &Path, zigcc: &Path) -> miette::Result<()> {
    let zigcc_str = zigcc
        .to_str()
        .ok_or_else(|| miette::miette!("zigcc path is not valid UTF-8"))?;

    info!(workspace = %workspace.display(), "Running cargo build -p perman");
    let status = Command::new("cargo")
        .args([
            "build",
            "-p",
            "perman",
            "--release",
            "--target",
            "x86_64-unknown-linux-musl",
        ])
        .current_dir(workspace)
        .env("CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER", zigcc_str)
        .env("CC_x86_64_unknown_linux_musl", zigcc_str)
        .status()
        .into_diagnostic()
        .wrap_err("cargo not found; install the Rust toolchain")?;
    if !status.success() {
        bail!("cargo build -p perman failed with {status}");
    }
    Ok(())
}
