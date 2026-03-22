mod build;
mod container;

use std::path::PathBuf;
use std::process::Command;
use std::str::FromStr;

use clap::Parser;
use cluman::schemas::Mode;
use isoman::{resolve_output, scopeguard};
use miette::IntoDiagnostic;
use tracing::info;

/// Build a bootable hybrid ISO image using the Limine bootloader.
///
/// When `--build` is supplied the initramfs is first produced by running
/// `podman build` against the project Containerfile with the chosen `--mode`
/// baked in as a build-arg.  The resulting `os.initramfs.tar.gz` is then used
/// as the initramfs for the ISO assembly step.
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
    /// the ISO.  Requires podman to be installed and the Containerfile to be
    /// present (see `--containerfile`).
    #[arg(long, default_value_t = false)]
    build: bool,

    /// cluman operating mode to embed in the initramfs image.
    ///
    /// Accepted values: `client`, `server`, `controller`.
    /// Only meaningful when `--build` is set.
    #[arg(
        short,
        long,
        default_value = "client",
        value_parser = parse_mode
    )]
    mode: Mode,

    /// Path to the Containerfile used to build the initramfs image.
    ///
    /// Only meaningful when `--build` is set.
    #[arg(long, env = "CONTAINERFILE", default_value = "Containerfile")]
    containerfile: PathBuf,

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
}

fn parse_mode(s: &str) -> Result<Mode, String> {
    Mode::from_str(s).map_err(|e| e.to_string())
}

fn default_kernel() -> PathBuf {
    let release = Command::new("uname")
        .arg("-r")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default();
    PathBuf::from(format!("/boot/vmlinuz-{release}"))
}

fn main() -> miette::Result<()> {
    tracing_subscriber::fmt::init();

    let args = Args::parse();

    let kernel = args.kernel.unwrap_or_else(default_kernel);

    // Resolve the ISO output path to absolute before we potentially change
    // into a staging directory.
    let output = {
        let cwd = std::env::current_dir().into_diagnostic()?;
        resolve_output(&cwd, &args.output)
    };

    // When --build is requested we produce the initramfs ourselves into a
    // temp file, then use that path for the ISO assembly step.
    let stage = std::env::temp_dir().join(format!("isoman-{}", std::process::id()));
    let _cleanup = scopeguard(&stage);

    let initramfs: PathBuf = if args.build {
        let cwd = std::env::current_dir().into_diagnostic()?;
        let containerfile = if args.containerfile.is_absolute() {
            args.containerfile.clone()
        } else {
            cwd.join(&args.containerfile)
        };

        let build_context = args.build_context.clone().unwrap_or_else(|| cwd.clone());

        std::fs::create_dir_all(&stage).into_diagnostic()?;
        let initramfs_out = stage.join("os.initramfs.tar.gz");

        info!(
            mode     = %args.mode.to_string(),
            context  = %build_context.display(),
            out      = %initramfs_out.display(),
            "Building initramfs via podman"
        );

        container::build_initramfs(&containerfile, &build_context, &args.mode, &initramfs_out)?;

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

    info!(
        kernel    = %kernel.display(),
        initramfs = %initramfs.display(),
        output    = %output.display(),
        mode      = %args.mode.to_string(),
        "Assembling ISO (Limine)"
    );

    build::build_iso(&kernel, &initramfs, &output, &stage, &args.mode)
}
