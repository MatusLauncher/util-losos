mod build;

use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use isoman::{resolve_output, scopeguard};
use miette::IntoDiagnostic;
use tracing::info;

/// Build a bootable hybrid ISO image using the Limine bootloader.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Args {
    /// Path to the kernel image.
    ///
    /// Defaults to the running kernel (/boot/vmlinuz-<uname -r>) when omitted.
    #[arg(short, long, env = "KERNEL")]
    kernel: Option<PathBuf>,

    /// Path to the initramfs archive.
    #[arg(short, long, env = "INITRAMFS", default_value = "os.initramfs.tar.gz")]
    initramfs: PathBuf,

    /// Destination path for the produced ISO file.
    #[arg(short, long, env = "OUTPUT", default_value = "os.iso")]
    output: String,
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

    // Resolve output to an absolute path before we enter the staging dir.
    let output = {
        let cwd = std::env::current_dir().into_diagnostic()?;
        resolve_output(&cwd, &args.output)
    };

    info!(
        kernel    = %kernel.display(),
        initramfs = %args.initramfs.display(),
        output    = %output.display(),
        "Starting isoman (Limine)"
    );

    let stage = std::env::temp_dir().join(format!("isoman-{}", std::process::id()));
    let _cleanup = scopeguard(&stage);

    build::build_iso(&kernel, &args.initramfs, &output, &stage)
}
