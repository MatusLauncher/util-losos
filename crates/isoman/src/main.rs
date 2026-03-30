//! CLI entry point for `isoman` — builds a bootable hybrid ISO and, optionally, the initramfs that goes inside it.

mod build;
mod container;
mod gsi;

use std::{env::current_dir, path::PathBuf, process::Command, str::FromStr};

use clap::{Parser, ValueEnum};
use cluman::schemas::Mode;
use isoman::{GSI_FASTBOOT_DEFAULT, GSI_ODIN_DEFAULT, resolve_output};
use miette::IntoDiagnostic;
use tracing::info;
use tracing_subscriber::fmt;
use walkdir::WalkDir;

/// Which GSI output format(s) to produce.
#[derive(Debug, Clone, ValueEnum)]
enum GsiFormat {
    /// Fastboot-compatible `boot.img`.
    Fastboot,
    /// Samsung Odin-compatible `.tar.md5` archive.
    Odin,
    /// Both Fastboot and Odin outputs.
    All,
}
/// Build a bootable hybrid ISO image using the Limine bootloader.
///
/// When `--build` is supplied the initramfs is produced by generating the
/// Containerfile from the embedded template (see `isoman::schema`), writing it
/// to a temporary path, and invoking `podman build`.  The chosen `--mode` is
/// baked into the template before the file is written, so no `--build-arg` is
/// needed.  The resulting `os.initramfs.tar.gz` is then used as the initramfs
/// for the ISO assembly step.
///
/// When `--build` is omitted a pre-existing initramfs archive must be supplied
/// via `--initramfs` (or the `INITRAMFS` env-var).
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the kernel image.
    ///
    /// Defaults to the running kernel (/boot/vmlinuz-<uname -r>) when omitted.
    #[arg(short, long, env = "KERNEL")]
    kernel: Option<PathBuf>,

    /// Path to the initramfs archive.
    ///
    /// Ignored when `--build` is set (the archive is produced automatically
    /// and stored in a temporary location).
    #[arg(short, long, env = "INITRAMFS", default_value = "os.initramfs.tar.gz")]
    initramfs: PathBuf,

    /// Destination path for the produced ISO file.
    #[arg(short, long, env = "OUTPUT", default_value = "os.iso")]
    output: String,

    /// Build the initramfs from source via `podman build` before assembling
    /// the ISO.  Requires podman to be installed.  The Containerfile is
    /// generated from the template embedded in `isoman::schema` — no external
    /// Containerfile is needed.
    #[arg(long, default_value_t = false)]
    build: bool,

    /// cluman operating mode to embed in the initramfs image.
    ///
    /// Accepted values: `client`, `server`, `controller`.
    /// Only meaningful when `--build` is set.
    #[arg(
        short,
        long,
        env,
        default_value = "client",
        value_parser = parse_mode
    )]
    mode: Mode,

    /// Build context directory passed to `podman build`.
    ///
    /// Defaults to the current working directory when omitted.
    /// Only meaningful when `--build` is set.
    #[arg(long, env = "BUILD_CONTEXT")]
    build_context: Option<PathBuf>,

    /// When `--build` is set, also copy the produced initramfs archive to
    /// this path so it can be kept as a CI artifact independently of the ISO.
    ///
    /// Only meaningful when `--build` is set.
    #[arg(long, env = "INITRAMFS_OUT")]
    initramfs_out: Option<PathBuf>,
    /// Flag to enable Docker layer caching during the `podman build` step.
    ///
    /// When set, previously cached layers are reused to speed up subsequent
    /// builds.  Only meaningful when `--build` is set.
    #[arg(long, default_value_t = true)]
    with_cache: bool,

    /// Build a GSI (Generic System Image) instead of a bootable ISO.
    ///
    /// Uses `mkbootimg` to bundle the kernel and initramfs into an Android boot
    /// image, then packages it for the requested flash tool(s).
    #[arg(long, default_value_t = false)]
    gsi: bool,

    /// Which GSI format(s) to emit.
    ///
    /// Only meaningful when `--gsi` is set.
    #[arg(long, default_value = "all", requires = "gsi")]
    gsi_format: GsiFormat,

    /// Destination path for the Fastboot `boot.img` artifact.
    ///
    /// Only meaningful when `--gsi` is set.
    #[arg(long, env = "FASTBOOT_OUT", default_value = GSI_FASTBOOT_DEFAULT, requires = "gsi")]
    fastboot_out: String,

    /// Destination path for the Odin `.tar.md5` artifact.
    ///
    /// Only meaningful when `--gsi` is set.
    #[arg(long, env = "ODIN_OUT", default_value = GSI_ODIN_DEFAULT, requires = "gsi")]
    odin_out: String,
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    Mode::from_str(s).map_err(|e| e.to_string())
}

/// Locate the running kernel image under `/boot` without requiring it to sit
/// at the root of that directory.
///
/// Standard distros (`/boot/vmlinuz-<release>`) are handled by a fast exact
/// probe.  Immutable distros that store kernels in subdirectories are handled
/// by a recursive walk that matches on the **filename** only:
///
/// | Distro | Typical path |
/// |---|---|
/// | Fedora / Debian / Ubuntu | `/boot/vmlinuz-<release>` |
/// | Silverblue / CoreOS | `/boot/ostree/<deployment>/vmlinuz-<release>` |
/// | NixOS | `/boot/kernels/<hash>-linux-<ver>/bzImage` (no vmlinuz) |
///
/// Selection priority (highest first):
/// 1. `/boot/vmlinuz-<uname -r>` — exact match for the running release.
/// 2. Any file under `/boot` whose **filename** starts with `vmlinuz` and
///    whose filename contains the running release string.
/// 3. Any file under `/boot` whose **filename** starts with `vmlinuz`.
fn find_kernel() -> miette::Result<PathBuf> {
    // Obtain the running kernel release string from `uname -r`.
    let release = Command::new("uname").arg("-r").output().into_diagnostic()?;
    let release = String::from_utf8_lossy(&release.stdout).trim().to_string();

    // ── Fast path: standard /boot/vmlinuz-<release> ──────────────────────────
    let standard = PathBuf::from(format!("/boot/vmlinuz-{release}"));
    if standard.is_file() {
        return Ok(standard);
    }

    // ── Slow path: recursive walk, filename-only matching ────────────────────
    //
    // We match on the *filename* component only (not the full path) so that
    // directory names like `/boot/ostree/default-<hash>/` do not interfere.
    // Entries that cannot be read (permission errors, broken symlinks) are
    // silently skipped.
    let mut best_with_release: Option<PathBuf> = None;
    let mut best_any: Option<PathBuf> = None;

    for entry in WalkDir::new("/boot")
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
    {
        let path = entry.into_path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        if !name.starts_with("vmlinuz") {
            continue;
        }
        if name.contains(release.as_str()) {
            // Prefer the first match that contains the running release string.
            if best_with_release.is_none() {
                best_with_release = Some(path);
            }
        } else if best_any.is_none() {
            best_any = Some(path);
        }
    }

    best_with_release.or(best_any).ok_or_else(|| {
        miette::miette!(
            "no kernel image found under /boot — \
             pass --kernel explicitly or set the KERNEL env-var"
        )
    })
}

/// Parses CLI arguments, optionally builds the initramfs via `build_initramfs`, then assembles the ISO via `build_iso`.
fn main() -> miette::Result<()> {
    fmt().init();
    let args = Args::parse();

    let kernel = match args.kernel {
        Some(k) => k,
        None => find_kernel()?,
    };

    // Resolve the ISO output path to absolute before we potentially change
    // into a staging directory.
    let output = {
        let cwd = std::env::current_dir().into_diagnostic()?;
        resolve_output(&cwd, &args.output)
    };

    // When --build is requested we produce the initramfs ourselves into a
    // temp file, then use that path for the ISO assembly step.
    let stage = std::env::temp_dir().join(format!("isoman-{}", std::process::id()));

    let initramfs: PathBuf = if args.build {
        let build_context = args
            .build_context
            .clone()
            .unwrap_or_else(|| current_dir().into_diagnostic().unwrap().clone());

        std::fs::create_dir_all(&stage).into_diagnostic()?;
        let initramfs_out = stage.join("os.initramfs.tar.gz");

        info!(
            mode     = %args.mode.to_string(),
            context  = %build_context.display(),
            out      = %initramfs_out.display(),
            "Building initramfs via podman"
        );

        container::build_initramfs(&build_context, &args.mode, &initramfs_out, args.with_cache)?;

        // Optionally persist the initramfs outside the staging dir so callers
        // (e.g. CI pipelines) can keep it as a standalone artifact.
        if let Some(ref persist) = args.initramfs_out {
            std::fs::copy(&initramfs_out, persist).into_diagnostic()?;
            info!(path = %persist.display(), "Initramfs persisted to --initramfs-out");
        }

        initramfs_out
    } else {
        args.initramfs.clone()
    };

    if args.gsi {
        let cwd = std::env::current_dir().into_diagnostic()?;
        let fastboot_out = resolve_output(&cwd, &args.fastboot_out);
        let odin_out = resolve_output(&cwd, &args.odin_out);

        match args.gsi_format {
            GsiFormat::Fastboot => {
                info!(
                    kernel    = %kernel.display(),
                    initramfs = %initramfs.display(),
                    output    = %fastboot_out.display(),
                    "Building Fastboot GSI"
                );
                gsi::build_gsi_fastboot(&kernel, &initramfs, &fastboot_out, &stage)?;
            }
            GsiFormat::Odin => {
                info!(
                    kernel    = %kernel.display(),
                    initramfs = %initramfs.display(),
                    output    = %odin_out.display(),
                    "Building Odin GSI"
                );
                gsi::build_gsi_odin(&kernel, &initramfs, &odin_out, &stage)?;
            }
            GsiFormat::All => {
                info!(
                    kernel    = %kernel.display(),
                    initramfs = %initramfs.display(),
                    fastboot  = %fastboot_out.display(),
                    odin      = %odin_out.display(),
                    "Building GSI for all formats"
                );
                gsi::build_gsi_fastboot(&kernel, &initramfs, &fastboot_out, &stage)?;
                gsi::build_gsi_odin(&kernel, &initramfs, &odin_out, &stage)?;
            }
        }
    } else {
        info!(
            kernel    = %kernel.display(),
            initramfs = %initramfs.display(),
            output    = %output.display(),
            mode      = %args.mode.to_string(),
            "Assembling ISO (Limine)"
        );
        build::build_iso(&kernel, &initramfs, &output, &stage, &args.mode)?;
    }

    Ok(())
}
