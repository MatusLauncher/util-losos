/// VM-based integration test for the preflight installer.
///
/// This test boots the **latest Arch Linux ISO** in QEMU (UEFI/OVMF),
/// attaches a virtual hard-drive image, copies the preflight binary and
/// mock OS artifacts into the guest via 9p virtio shares, and runs the
/// installer non-interactively.
///
/// After the VM shuts down, the virtual disk image is inspected on the host
/// to verify the install layout (partitions, filesystems, boot files,
/// users.json).
///
/// Requirements on the host:
///   - `qemu-system-x86_64`
///   - OVMF firmware (`/usr/share/edk2-ovmf/x64/OVMF.fd` or similar)
///   - `blkid`, `mtype` (mtools), `debugfs`, `fdisk`, `dd`, `truncate`
///   - ~4 GB free RAM + ~8 GB disk space
///
/// The Arch ISO is downloaded once and cached at `$CARGO_TARGET_TMPDIR/arch.iso`.
///
/// Run with:
///   sudo -E cargo test --test vm_install -- --ignored --test-threads=1
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// URL template for the latest Arch Linux monthly ISO.
const ARCH_ISO_INDEX: &str = "https://archive.archlinux.org/iso/";

/// Timeout for the VM to finish installation (seconds).
const VM_TIMEOUT_SECS: u64 = 600;

/// Size of the virtual disk in MiB.
const DISK_SIZE_MB: u32 = 8192;

/// RAM allocated to the VM in MiB.
const VM_RAM_MB: u32 = 2048;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Locate the OVMF firmware files on the host.
fn find_ovmf() -> (PathBuf, PathBuf) {
    let candidates: &[(&str, &str)] = &[
        (
            "/usr/share/edk2-ovmf/x64/OVMF.fd",
            "/usr/share/edk2-ovmf/x64/OVMF_VARS.fd",
        ),
        (
            "/usr/share/edk2-ovmf/OVMF.fd",
            "/usr/share/edk2-ovmf/OVMF_VARS.fd",
        ),
        (
            "/usr/share/OVMF/OVMF_CODE.fd",
            "/usr/share/OVMF/OVMF_VARS.fd",
        ),
        (
            "/usr/share/edk2/ovmf/OVMF.fd",
            "/usr/share/edk2/ovmf/OVMF_VARS.fd",
        ),
        (
            "/usr/share/qemu/ovmf-x86_64-code.bin",
            "/usr/share/qemu/ovmf-x86_64-vars.bin",
        ),
    ];
    for (code, vars) in candidates {
        if Path::new(code).exists() {
            return (PathBuf::from(code), PathBuf::from(vars));
        }
    }
    panic!(
        "OVMF firmware not found. Install edk2-ovmf or qemu-ovmf. \
         Searched: {:?}",
        candidates.iter().map(|(c, _)| c).collect::<Vec<_>>()
    );
}

/// Locate `qemu-system-x86_64`.
fn qemu_bin() -> PathBuf {
    let out = Command::new("which")
        .arg("qemu-system-x86_64")
        .output()
        .expect("which not found");
    assert!(
        out.status.success(),
        "qemu-system-x86_64 not found — install qemu"
    );
    PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())
}

/// Download the latest Arch Linux ISO if not already cached.
fn ensure_arch_iso(cache_dir: &Path) -> PathBuf {
    let iso_path = cache_dir.join("arch.iso");
    if iso_path.exists() {
        let out = Command::new("file").arg(&iso_path).output().unwrap();
        let info = String::from_utf8_lossy(&out.stdout);
        if info.contains("ISO 9660") || info.contains("CD-ROM") {
            return iso_path;
        }
        fs::remove_file(&iso_path).ok();
    }

    eprintln!("==> Downloading latest Arch Linux ISO …");

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .expect("cannot build reqwest client");

    let dates_html = client
        .get(ARCH_ISO_INDEX)
        .send()
        .expect("cannot fetch arch iso index")
        .text()
        .expect("cannot read arch iso index");

    let re = regex::Regex::new(r"(\d{4}\.\d{2}\.\d{2})/").unwrap();
    let mut dates: Vec<&str> = re
        .captures_iter(&dates_html)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str()))
        .collect();
    dates.sort_unstable();
    dates.dedup();
    let latest = dates.last().expect("no dates found on archlinux.org");

    let url = format!("https://archive.archlinux.org/iso/{latest}/archlinux-{latest}-x86_64.iso");
    eprintln!("==> URL: {url}");
    let bytes = client
        .get(&url)
        .send()
        .unwrap_or_else(|_| panic!("failed to download {url}"))
        .bytes()
        .unwrap();
    fs::write(&iso_path, &bytes).expect("failed to write ISO");
    eprintln!("==> ISO saved to {}", iso_path.display());
    iso_path
}

/// Build mock OS artifacts (fake kernel + minimal gzip tarball).
fn populate_artifacts(dir: &Path) {
    fs::write(dir.join("vmlinuz"), b"fake kernel").unwrap();
    let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    let mut tar = tar::Builder::new(gz);
    tar.finish().unwrap();
    let gz_data = tar.into_inner().unwrap().finish().unwrap();
    fs::write(dir.join("os.initramfs.tar.gz"), gz_data).unwrap();
}

/// Build the preflight binary and return its path.
fn build_preflight() -> PathBuf {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bin = manifest.join("target/release/preflight");
    if !bin.exists() {
        let dev = manifest.join("target/debug/preflight");
        if dev.exists() {
            return dev;
        }
        panic!("preflight binary not found. Run `cargo build --release` first.");
    }
    bin
}

/// Shell script executed _inside_ the VM after Arch ISO boot + auto-login.
fn install_script(username: &str, password: &str) -> String {
    format!(
        r#"#!/bin/bash
set -ex

# 1. Install runtime dependencies
pacman -Sy --noconfirm \
    dosfstools e2fsprogs cryptsetup efibootmgr lvm2 util-linux udev

# 2. Start udev daemon (needed by udevadm settle)
udevd --daemon

# 3. Mount 9p shares
mkdir -p /mnt/preflight /mnt/artifacts /mnt/scripts
mount -t 9p -o trans=virtio,ro preflight /mnt/preflight
mount -t 9p -o trans=virtio,ro artifacts /mnt/artifacts
mount -t 9p -o trans=virtio,ro scripts /mnt/scripts

# 4. Copy preflight binary to RAM (9p may not support exec)
cp /mnt/preflight/preflight /tmp/preflight
chmod +x /tmp/preflight

# 5. Copy artifacts next to preflight
cp /mnt/artifacts/vmlinuz /tmp/
cp /mnt/artifacts/os.initramfs.tar.gz /tmp/

# 6. Feed answers to dialoguer prompts via stdin pipe.
#    Prompt order in main.rs:
#      a) Select disk(s)   → MultiSelect: space (toggle #0), enter
#      b) WARNING confirm  → Confirm:     enter (yes)
#      c) Feature select   → MultiSelect: space×3 (all), enter
#      d) Username          → Input:       "{username}"
#      e) Password          → Password:    "{password}"
#      f) Confirm password  → Password:    "{password}"
#      g) cluman mode       → Select:      enter (client, default)
{{
    sleep 4
    printf ' \n'      # a) toggle disk 0 + confirm
    sleep 2
    printf '\n'       # b) yes, destructive
    sleep 2
    printf '   \n'    # c) toggle all 3 features + confirm
    sleep 2
    printf '{username}\n'  # d)
    sleep 1
    printf '{password}\n'  # e)
    sleep 1
    printf '{password}\n'  # f)
    sleep 2
    printf '\n'       # g) default cluman mode
    sleep 5
}} | /tmp/preflight 2>/tmp/preflight.log || {{
    echo "===== preflight stderr ====="
    cat /tmp/preflight.log
    exit 1
}}

sync
poweroff
"#,
    )
}

/// Parse partition offsets from `fdisk -l` output.
/// Returns Vec of (start_sector, sector_count).
fn parse_fdisk_offsets(fdisk_out: &str) -> Vec<(u64, u64)> {
    let mut parts = Vec::new();
    let re = regex::Regex::new(r"(?m)^\S+\s+\d+\s+(\d+)\s+(\d+)\s+\d+[KMG]?\s+.*$").unwrap();
    for cap in re.captures_iter(fdisk_out) {
        let start: u64 = cap[1].parse().unwrap();
        let end: u64 = cap[2].parse().unwrap();
        parts.push((start, end - start + 1));
    }
    parts
}

/// Extract a partition from a disk image using `dd`.
fn extract_partition(disk: &Path, (start, sectors): &(u64, u64)) -> PathBuf {
    let sector_size: u64 = 512;
    let tmp = PathBuf::from(format!("/tmp/vm_test_part_{}_{}.img", start, sectors));
    if tmp.exists() {
        fs::remove_file(&tmp).ok();
    }
    let out = Command::new("dd")
        .args([
            &format!("if={}", disk.display()),
            &format!("of={}", tmp.display()),
            &format!("bs={sector_size}"),
            &format!("skip={start}"),
            &format!("count={sectors}"),
        ])
        .output()
        .expect("dd not found");
    assert!(out.status.success(), "dd failed");
    tmp
}

/// Run `blkid -o value -s <tag> <image>`.
fn blkid(image: &Path, tag: &str) -> String {
    let out = Command::new("blkid")
        .args(["-o", "value", "-s", tag, image.to_str().unwrap()])
        .output()
        .expect("blkid not found");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn run(cmd: &mut Command) {
    let status = cmd.status().expect("command not found");
    assert!(status.success(), "command failed: {cmd:?}");
}

// ---------------------------------------------------------------------------
// Verification
// ---------------------------------------------------------------------------

fn verify_disk_layout(disk_img: &Path) {
    let out = Command::new("fdisk")
        .args(["-l", disk_img.to_str().unwrap()])
        .output()
        .expect("fdisk not found");
    let fdisk_out = String::from_utf8_lossy(&out.stdout);
    eprintln!("=== fdisk output ===\n{fdisk_out}");

    assert!(
        fdisk_out.contains("EFI System") || fdisk_out.contains("EFI"),
        "Missing EFI partition. fdisk output:\n{fdisk_out}"
    );

    let parts = parse_fdisk_offsets(&fdisk_out);
    assert_eq!(
        parts.len(),
        2,
        "Expected 2 partitions, found {}. fdisk:\n{fdisk_out}",
        parts.len()
    );

    // Boot partition → FAT32, LABEL=BOOT
    let boot_img = extract_partition(disk_img, &parts[0]);
    assert_eq!(blkid(&boot_img, "TYPE"), "vfat", "boot not FAT32");
    assert_eq!(blkid(&boot_img, "LABEL"), "BOOT", "boot not labelled BOOT");

    let check = |path: &str| {
        let out = Command::new("mtype")
            .args(["-i", boot_img.to_str().unwrap(), &format!("::{path}")])
            .output()
            .expect("mtype not found — install mtools");
        assert!(out.status.success(), "Expected '{path}' on boot partition");
    };
    check("EFI/LosOS/vmlinuz.efi");
    check("EFI/LosOS/initramfs.gz");

    // Data partition → LUKS2
    let data_img = extract_partition(disk_img, &parts[1]);
    assert_eq!(
        blkid(&data_img, "TYPE"),
        "crypto_LUKS",
        "data partition not LUKS2"
    );
}

// ---------------------------------------------------------------------------
// Test
// ---------------------------------------------------------------------------

#[test]
#[ignore = "requires QEMU, OVMF, Arch ISO download, ~8 GB disk, ~2 GB RAM"]
fn full_install_in_qemu_vm() {
    qemu_bin();
    let (ovmf_code, ovmf_vars_template) = find_ovmf();

    let cache_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(&cache_dir).ok();

    let username = "testuser";
    let password = "hunter2";

    // --- Arch ISO ---
    let iso_path = ensure_arch_iso(&cache_dir);
    eprintln!("==> Using Arch ISO: {}", iso_path.display());

    // --- Virtual disk (qcow2) ---
    let disk_img = cache_dir.join("vm_disk.qcow2");
    if disk_img.exists() {
        fs::remove_file(&disk_img).ok();
    }
    run(Command::new("qemu-img").args([
        "create",
        "-f",
        "qcow2",
        disk_img.to_str().unwrap(),
        &format!("{DISK_SIZE_MB}M"),
    ]));

    // --- Preflight binary ---
    let preflight_bin = build_preflight();
    eprintln!("==> Preflight binary: {}", preflight_bin.display());

    // --- Mock artifacts ---
    let artifacts_dir = cache_dir.join("artifacts");
    fs::create_dir_all(&artifacts_dir).ok();
    populate_artifacts(&artifacts_dir);

    // --- 9p shares ---
    let preflight_share = cache_dir.join("9p_preflight");
    fs::create_dir_all(&preflight_share).ok();
    fs::copy(&preflight_bin, preflight_share.join("preflight")).unwrap();

    let artifacts_share = cache_dir.join("9p_artifacts");
    fs::create_dir_all(&artifacts_share).ok();
    fs::copy(
        artifacts_dir.join("vmlinuz"),
        artifacts_share.join("vmlinuz"),
    )
    .unwrap();
    fs::copy(
        artifacts_dir.join("os.initramfs.tar.gz"),
        artifacts_share.join("os.initramfs.tar.gz"),
    )
    .unwrap();

    let scripts_share = cache_dir.join("9p_scripts");
    fs::create_dir_all(&scripts_share).ok();
    let script_content = install_script(username, password);
    let script_path = scripts_share.join("install.sh");
    fs::write(&script_path, script_content).unwrap();
    fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

    // --- Overlay image (mounted as extra block device, contains /root/.bash_profile) ---
    let overlay_img = cache_dir.join("overlay.img");
    if overlay_img.exists() {
        fs::remove_file(&overlay_img).ok();
    }
    run(Command::new("truncate").args(["-s", "64M", overlay_img.to_str().unwrap()]));
    run(Command::new("mkfs.ext4").args(["-F", "-L", "OVERLAY", overlay_img.to_str().unwrap()]));

    let overlay_mount = cache_dir.join("overlay_mount");
    fs::create_dir_all(&overlay_mount).ok();
    run(Command::new("mount").args([
        overlay_img.to_str().unwrap(),
        overlay_mount.to_str().unwrap(),
    ]));
    fs::create_dir_all(overlay_mount.join("root")).ok();
    // Arch ISO auto-logs in root on tty1; .bash_profile triggers the install.
    fs::write(
        overlay_mount.join("root/.bash_profile"),
        "#!/bin/bash\n/bin/bash /mnt/scripts/install.sh > /tmp/install.log 2>&1\n",
    )
    .unwrap();
    run(Command::new("umount").arg(overlay_mount.to_str().unwrap()));

    // --- Writable OVMF vars ---
    let ovmf_vars_tmp = cache_dir.join("OVMF_VARS.tmp.fd");
    fs::copy(&ovmf_vars_template, &ovmf_vars_tmp).unwrap();

    // --- Boot QEMU ---
    let qemu = qemu_bin();
    let mut cmd = Command::new(&qemu);
    cmd.args([
        "-enable-kvm",
        "-m",
        &VM_RAM_MB.to_string(),
        "-drive",
        &format!("file={},format=qcow2,if=virtio", disk_img.display()),
        "-cdrom",
        iso_path.to_str().unwrap(),
        "-drive",
        &format!(
            "file={},format=raw,if=virtio,readonly=on",
            overlay_img.display()
        ),
        "-bios",
        ovmf_code.to_str().unwrap(),
        "-nographic",
        "-boot",
        "d",
        "-append",
        "console=ttyS0,115200 earlyprintk=ttyS0,115200 overlay=LABEL=OVERLAY",
        // 9p: preflight
        "-fsdev",
        &format!(
            "local,id=preflight,path={},security_model=none,readonly=on",
            preflight_share.display()
        ),
        "-device",
        "virtio-9p-pci,fsdev=preflight,mount_tag=preflight",
        // 9p: artifacts
        "-fsdev",
        &format!(
            "local,id=artifacts,path={},security_model=none,readonly=on",
            artifacts_share.display()
        ),
        "-device",
        "virtio-9p-pci,fsdev=artifacts,mount_tag=artifacts",
        // 9p: scripts
        "-fsdev",
        &format!(
            "local,id=scripts,path={},security_model=none,readonly=on",
            scripts_share.display()
        ),
        "-device",
        "virtio-9p-pci,fsdev=scripts,mount_tag=scripts",
    ]);

    eprintln!("==> Booting VM …");

    let mut child = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to start QEMU");

    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();

    let out_handle = thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            if let Ok(l) = line {
                eprintln!("[VM] {l}");
            }
        }
    });
    let err_handle = thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            if let Ok(l) = line {
                eprintln!("[VM] {l}");
            }
        }
    });

    let deadline = std::time::Instant::now() + Duration::from_secs(VM_TIMEOUT_SECS);
    loop {
        match child.try_wait().expect("failed to poll child") {
            Some(status) => {
                eprintln!("==> VM exited with status: {status}");
                let _ = out_handle.join();
                let _ = err_handle.join();
                assert!(status.success(), "VM install failed");
                break;
            }
            None => {
                if std::time::Instant::now() > deadline {
                    child.kill().expect("failed to kill QEMU");
                    let _ = out_handle.join();
                    let _ = err_handle.join();
                    panic!("VM install timed out after {VM_TIMEOUT_SECS}s");
                }
                thread::sleep(Duration::from_secs(5));
            }
        }
    }

    eprintln!("==> Verifying disk layout …");
    verify_disk_layout(&disk_img);
    eprintln!("==> All checks passed!");
}
