use std::fs;
use std::path::PathBuf;
use std::process::Command;

use clap::Parser;
use isoman::{LIMINE_BRANCH, LIMINE_CONF, LIMINE_REPO, resolve_output, scopeguard};
use miette::{Context, IntoDiagnostic, bail};
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
    fs::copy(&args.initramfs, iso_root.join("boot").join("initramfs.gz")).into_diagnostic()?;

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
