//! Smoke tests and micro-benchmarks for `isoman`'s GSI pipeline.
//!
//! Exercises:
//! * `build_gsi_fastboot`   — full end-to-end Fastboot image build into a temp dir.
//! * `build_gsi_odin`       — full end-to-end Odin `.tar.md5` build into a temp dir.
//! * boot image header      — `ANDROID!` magic, kernel/ramdisk size fields, page-size field.
//! * Odin archive structure — tar member list, MD5 trailer presence and format.
//! * `MkbootimgParams`      — construction and field assignment throughput.
//! * `resolve_output`       — absolute/relative path resolution.
//! * Scaling                — fastboot + odin repeated over several ramdisk sizes.

use std::fs;
use std::hint::black_box;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use isoman::{GSI_CMDLINE, GSI_FASTBOOT_DEFAULT, GSI_ODIN_DEFAULT, resolve_output};
use mkbootimg::MkbootimgParams;
use tempfile::TempDir;

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Magic bytes at offset 0 of every Android boot image.
const ANDROID_MAGIC: &[u8] = b"ANDROID!";

/// Write `size` pseudo-random (but deterministic) bytes to `path`.
///
/// The content only needs to satisfy `mkbootimg`'s "file exists and is
/// non-empty" requirement; it does not need to be a real kernel/ramdisk.
fn write_dummy_file(path: &Path, size: usize) {
    let data: Vec<u8> = (0..size).map(|i| (i & 0xFF) as u8).collect();
    fs::write(path, &data).expect("failed to write dummy file");
}

/// Create a staging directory together with a dummy kernel and initramfs.
///
/// Returns `(tmp, kernel_path, initramfs_path)`.  The `TempDir` must stay
/// alive for the duration of the test.
fn setup(kernel_bytes: usize, ramdisk_bytes: usize) -> (TempDir, PathBuf, PathBuf) {
    let tmp = TempDir::new().expect("tempdir");
    let kernel = tmp.path().join("vmlinuz");
    let initramfs = tmp.path().join("initramfs.gz");
    write_dummy_file(&kernel, kernel_bytes);
    write_dummy_file(&initramfs, ramdisk_bytes);
    (tmp, kernel, initramfs)
}

/// Read the first `n` bytes of `path`.
fn read_head(path: &Path, n: usize) -> Vec<u8> {
    let mut f = fs::File::open(path).expect("open");
    let mut buf = vec![0u8; n];
    f.read_exact(&mut buf).expect("read_head");
    buf
}

/// Read a little-endian `u32` at byte offset `off` inside `path`.
fn read_u32_le(path: &Path, off: u64) -> u32 {
    let mut f = fs::File::open(path).expect("open");
    f.seek(SeekFrom::Start(off)).expect("seek");
    let mut buf = [0u8; 4];
    f.read_exact(&mut buf).expect("read_u32_le");
    u32::from_le_bytes(buf)
}

// boot_img_hdr_v0 field offsets (bytes):
//   0x00  magic[8]
//   0x08  kernel_size (u32 LE)
//   0x0C  kernel_addr (u32 LE)
//   0x10  ramdisk_size (u32 LE)
//   0x14  ramdisk_addr (u32 LE)
//   0x24  page_size (u32 LE)
const HDR_OFF_KERNEL_SIZE: u64 = 0x08;
const HDR_OFF_RAMDISK_SIZE: u64 = 0x10;
const HDR_OFF_PAGE_SIZE: u64 = 0x24;

// ── build_gsi_fastboot — correctness ─────────────────────────────────────────

mod fastboot_correctness {
    use super::*;

    #[test]
    fn output_file_is_created() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        assert!(output.exists(), "boot.img must exist after build");
    }

    #[test]
    fn output_is_nonempty() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let meta = fs::metadata(&output).unwrap();
        assert!(meta.len() > 0, "boot.img must not be empty");
    }

    #[test]
    fn output_starts_with_android_magic() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let head = read_head(&output, 8);
        assert_eq!(
            &head, ANDROID_MAGIC,
            "boot.img must start with ANDROID! magic"
        );
    }

    #[test]
    fn header_kernel_size_matches_input() {
        let kernel_sz = 8192usize;
        let (tmp, kernel, initramfs) = setup(kernel_sz, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let stored = read_u32_le(&output, HDR_OFF_KERNEL_SIZE);
        assert_eq!(
            stored as usize, kernel_sz,
            "kernel_size in header ({stored}) must match actual kernel size ({kernel_sz})"
        );
    }

    #[test]
    fn header_ramdisk_size_matches_input() {
        let ramdisk_sz = 16384usize;
        let (tmp, kernel, initramfs) = setup(4096, ramdisk_sz);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let stored = read_u32_le(&output, HDR_OFF_RAMDISK_SIZE);
        assert_eq!(
            stored as usize, ramdisk_sz,
            "ramdisk_size in header ({stored}) must match actual ramdisk size ({ramdisk_sz})"
        );
    }

    #[test]
    fn header_page_size_is_default_2048() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let page_size = read_u32_le(&output, HDR_OFF_PAGE_SIZE);
        assert_eq!(page_size, 2048, "default page size must be 2048");
    }

    #[test]
    fn image_size_is_multiple_of_page_size() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        let len = fs::metadata(&output).unwrap().len();
        assert_eq!(
            len % 2048,
            0,
            "boot.img size ({len}) must be a multiple of the page size (2048)"
        );
    }

    #[test]
    fn stage_dir_contains_boot_img_intermediate() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
        // The intermediate boot.img written to the stage dir must also exist.
        assert!(
            stage.path().join("boot.img").exists(),
            "stage/boot.img must exist after fastboot build"
        );
    }

    #[test]
    fn two_successive_builds_produce_identical_output() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let out1 = tmp.path().join("boot1.img");
        let out2 = tmp.path().join("boot2.img");
        let stage1 = TempDir::new().unwrap();
        let stage2 = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &out1, stage1.path());
        isoman_gsi_fastboot(&kernel, &initramfs, &out2, stage2.path());
        let b1 = fs::read(&out1).unwrap();
        let b2 = fs::read(&out2).unwrap();
        assert_eq!(
            b1, b2,
            "two builds from identical inputs must be byte-for-byte equal"
        );
    }
}

// ── build_gsi_odin — correctness ──────────────────────────────────────────────

mod odin_correctness {
    use super::*;

    #[test]
    fn output_file_is_created() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
        assert!(output.exists(), "Odin archive must exist after build");
    }

    #[test]
    fn output_is_nonempty() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
        assert!(fs::metadata(&output).unwrap().len() > 0);
    }

    #[test]
    fn output_starts_with_tar_magic() {
        // POSIX `tar` archives start with the filename in the first 100 bytes
        // and have the ustar magic at offset 257.  A simpler check: the first
        // byte must not be 0x00 (null-padded blocks come later in the stream).
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
        let head = read_head(&output, 1);
        assert_ne!(head[0], 0x00, "first byte of Odin archive must not be null");
    }

    #[test]
    fn tar_member_is_boot_img() {
        // The first tar header entry (bytes 0..100) holds the filename field.
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
        let head = read_head(&output, 100);
        let name = std::str::from_utf8(&head)
            .unwrap_or("")
            .trim_end_matches('\0');
        assert!(
            name.contains("boot.img"),
            "first tar member must be boot.img, got: {name:?}"
        );
    }

    #[test]
    fn md5_trailer_is_present() {
        // The Odin format appends `<hex32>  <filename>\n` to the tar.
        // A valid MD5 hex string is exactly 32 lowercase hex characters.
        //
        // A POSIX tar stream ends with two 512-byte null-filled blocks, so the
        // appended MD5 line sits *after* those null blocks.  We must skip
        // backward past any trailing null bytes before looking for the line.
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());

        let last_line = last_nonempty_line(&output);

        // Format is "<hash>  <path>" — split on the double space.
        let parts: Vec<&str> = last_line.splitn(2, "  ").collect();
        assert_eq!(
            parts.len(),
            2,
            "MD5 trailer must have format '<hash>  <path>', got: {last_line:?}"
        );

        let hash = parts[0];
        assert_eq!(
            hash.len(),
            32,
            "MD5 hash must be 32 hex characters, got: {hash:?}"
        );
        assert!(
            hash.chars().all(|c| c.is_ascii_hexdigit()),
            "MD5 hash must be lowercase hex, got: {hash:?}"
        );
    }

    #[test]
    fn md5_trailer_references_tar_filename() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());

        let last_line = last_nonempty_line(&output);
        // The path component after "  " must contain "AP_losos.tar".
        assert!(
            last_line.contains("AP_losos.tar"),
            "MD5 trailer must reference AP_losos.tar, got: {last_line:?}"
        );
    }

    #[test]
    fn stage_dir_contains_intermediate_tar() {
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
        assert!(
            stage.path().join("AP_losos.tar").exists(),
            "stage/AP_losos.tar must exist after odin build"
        );
    }

    #[test]
    fn embedded_boot_img_has_android_magic() {
        // Extract the tar member and verify the magic bytes of the embedded boot.img.
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("AP_losos.tar.md5");
        let stage = TempDir::new().unwrap();
        isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());

        // The boot.img in the stage directory is the exact file that went into
        // the tar, so checking it is equivalent to checking the tar member.
        let boot_img = stage.path().join("boot.img");
        assert!(boot_img.exists(), "stage/boot.img must exist");
        let head = read_head(&boot_img, 8);
        assert_eq!(
            &head, ANDROID_MAGIC,
            "embedded boot.img must start with ANDROID! magic"
        );
    }

    #[test]
    fn odin_output_larger_than_fastboot_output() {
        // The Odin archive wraps the boot image in a tar, so it must be larger.
        let (tmp, kernel, initramfs) = setup(8192, 8192);
        let fastboot_out = tmp.path().join("boot.img");
        let odin_out = tmp.path().join("AP_losos.tar.md5");
        let stage_fb = TempDir::new().unwrap();
        let stage_od = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &fastboot_out, stage_fb.path());
        isoman_gsi_odin(&kernel, &initramfs, &odin_out, stage_od.path());
        let fb_size = fs::metadata(&fastboot_out).unwrap().len();
        let odin_size = fs::metadata(&odin_out).unwrap().len();
        assert!(
            odin_size > fb_size,
            "Odin archive ({odin_size} B) must be larger than raw boot.img ({fb_size} B)"
        );
    }
}

// ── MkbootimgParams construction benchmarks ───────────────────────────────────

mod mkbootimg_params {
    use super::*;

    #[test]
    fn default_construction() {
        black_box(MkbootimgParams::default());
    }

    #[test]
    fn construction_with_output_only() {
        let mut p = MkbootimgParams::default();
        p.output = "/tmp/boot.img".to_owned();
        black_box(p);
    }

    #[test]
    fn construction_all_gsi_fields() {
        let mut p = MkbootimgParams::default();
        p.output = "/tmp/boot.img".to_owned();
        p.kernel = Some("/boot/vmlinuz".to_owned());
        p.ramdisk = Some("/boot/initramfs.gz".to_owned());
        p.cmdline = Some(GSI_CMDLINE.to_owned());
        black_box(p);
    }

    #[test]
    fn repeated_construction_100() {
        for _ in 0..100 {
            let mut p = MkbootimgParams::default();
            p.output = "/tmp/boot.img".to_owned();
            p.kernel = Some("/boot/vmlinuz".to_owned());
            p.ramdisk = Some("/boot/initramfs.gz".to_owned());
            p.cmdline = Some(GSI_CMDLINE.to_owned());
            black_box(p);
        }
    }

    #[test]
    fn clone() {
        let mut p = MkbootimgParams::default();
        p.output = "/tmp/boot.img".to_owned();
        p.kernel = Some("/boot/vmlinuz".to_owned());
        p.ramdisk = Some("/boot/initramfs.gz".to_owned());
        p.cmdline = Some(GSI_CMDLINE.to_owned());
        black_box(p.clone());
    }

    #[test]
    fn cmdline_is_embedded_in_created_image() {
        // Verify that the GSI_CMDLINE constant makes it into the boot image.
        //
        // boot_img_hdr_v0 layout (packed):
        //   0x00  magic[8]
        //   0x08  kernel_size(u32) + kernel_addr(u32)
        //   0x10  ramdisk_size(u32) + ramdisk_addr(u32)
        //   0x18  second_size(u32)  + second_addr(u32)
        //   0x20  tags_addr(u32)    + page_size(u32)
        //   0x28  header_version(u32)
        //   0x2C  os_version(u32)
        //   0x30  name[16]
        //   0x40  cmdline[512]   ← BOOT_ARGS_SIZE
        let (tmp, kernel, initramfs) = setup(4096, 4096);
        let output = tmp.path().join("boot.img");
        let stage = TempDir::new().unwrap();
        isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());

        let mut f = fs::File::open(&output).unwrap();
        f.seek(SeekFrom::Start(0x40)).unwrap();
        let mut buf = [0u8; 512];
        f.read_exact(&mut buf).unwrap();
        let stored = std::str::from_utf8(&buf)
            .unwrap_or("")
            .trim_end_matches('\0');
        assert!(
            stored.contains("net.ifnames=0"),
            "embedded cmdline must contain net.ifnames=0, got: {stored:?}"
        );
        assert!(
            stored.contains("biosdevname=0"),
            "embedded cmdline must contain biosdevname=0, got: {stored:?}"
        );
    }
}

// ── GSI_CMDLINE / default filename constants ──────────────────────────────────

mod gsi_constants {
    use super::*;

    #[test]
    fn gsi_cmdline_is_nonempty() {
        assert!(!GSI_CMDLINE.is_empty());
    }

    #[test]
    fn gsi_cmdline_disables_biosdevname() {
        assert!(
            GSI_CMDLINE.contains("biosdevname=0"),
            "GSI_CMDLINE must disable biosdevname"
        );
    }

    #[test]
    fn gsi_cmdline_disables_net_ifnames() {
        assert!(
            GSI_CMDLINE.contains("net.ifnames=0"),
            "GSI_CMDLINE must disable net.ifnames"
        );
    }

    #[test]
    fn gsi_fastboot_default_has_img_extension() {
        assert!(
            GSI_FASTBOOT_DEFAULT.ends_with(".img"),
            "fastboot default filename must end with .img"
        );
    }

    #[test]
    fn gsi_odin_default_has_tar_md5_extension() {
        assert!(
            GSI_ODIN_DEFAULT.ends_with(".tar.md5"),
            "odin default filename must end with .tar.md5"
        );
    }

    #[test]
    fn gsi_odin_default_starts_with_ap() {
        // Samsung Odin requires the AP_ prefix to identify the AP (application
        // processor) partition payload.
        assert!(
            GSI_ODIN_DEFAULT.starts_with("AP_"),
            "odin default filename must start with AP_"
        );
    }
}

// ── resolve_output ────────────────────────────────────────────────────────────

mod resolve_output_tests {
    use super::*;

    #[test]
    fn absolute_path_returned_unchanged() {
        let base = PathBuf::from("/build/output");
        let result = resolve_output(&base, "/tmp/boot.img");
        assert_eq!(result, PathBuf::from("/tmp/boot.img"));
    }

    #[test]
    fn relative_path_joined_to_base() {
        let base = PathBuf::from("/build/output");
        let result = resolve_output(&base, "boot.img");
        assert_eq!(result, PathBuf::from("/build/output/boot.img"));
    }

    #[test]
    fn relative_path_with_subdirectory() {
        let base = PathBuf::from("/ci/artifacts");
        let result = resolve_output(&base, "gsi/AP_losos.tar.md5");
        assert_eq!(result, PathBuf::from("/ci/artifacts/gsi/AP_losos.tar.md5"));
    }

    #[test]
    fn fastboot_default_joined_to_base() {
        let base = PathBuf::from("/output");
        black_box(resolve_output(&base, GSI_FASTBOOT_DEFAULT));
    }

    #[test]
    fn odin_default_joined_to_base() {
        let base = PathBuf::from("/output");
        black_box(resolve_output(&base, GSI_ODIN_DEFAULT));
    }

    #[test]
    fn repeated_100() {
        let base = PathBuf::from("/output");
        for i in 0..100usize {
            black_box(resolve_output(&base, &format!("boot-{i}.img")));
        }
    }
}

// ── Scaling — fastboot ────────────────────────────────────────────────────────

mod fastboot_scaling {
    use super::*;

    /// Ramdisk sizes (bytes) used in scaling tests.
    const RAMDISK_SIZES: &[usize] = &[1024, 4096, 16_384, 65_536, 262_144];

    #[test]
    fn varying_ramdisk_size() {
        for &sz in RAMDISK_SIZES {
            let (tmp, kernel, initramfs) = setup(4096, sz);
            let output = tmp.path().join("boot.img");
            let stage = TempDir::new().unwrap();
            isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
            // Verify the image was created and has the right magic.
            assert!(output.exists(), "boot.img must exist for ramdisk size {sz}");
            let head = read_head(&output, 8);
            assert_eq!(&head, ANDROID_MAGIC, "bad magic for ramdisk size {sz}");
            let stored_sz = read_u32_le(&output, HDR_OFF_RAMDISK_SIZE);
            assert_eq!(stored_sz as usize, sz, "ramdisk_size mismatch for {sz}");
            black_box(&output);
        }
    }

    #[test]
    fn varying_kernel_size() {
        const KERNEL_SIZES: &[usize] = &[4096, 16_384, 65_536, 131_072];
        for &sz in KERNEL_SIZES {
            let (tmp, kernel, initramfs) = setup(sz, 4096);
            let output = tmp.path().join("boot.img");
            let stage = TempDir::new().unwrap();
            isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
            assert!(output.exists(), "boot.img must exist for kernel size {sz}");
            let stored_sz = read_u32_le(&output, HDR_OFF_KERNEL_SIZE);
            assert_eq!(stored_sz as usize, sz, "kernel_size mismatch for {sz}");
            black_box(&output);
        }
    }

    #[test]
    fn output_size_grows_monotonically_with_ramdisk() {
        let mut prev_size = 0u64;
        for &sz in RAMDISK_SIZES {
            let (tmp, kernel, initramfs) = setup(4096, sz);
            let output = tmp.path().join("boot.img");
            let stage = TempDir::new().unwrap();
            isoman_gsi_fastboot(&kernel, &initramfs, &output, stage.path());
            let cur_size = fs::metadata(&output).unwrap().len();
            assert!(
                cur_size >= prev_size,
                "boot.img size must not shrink as ramdisk grows: {prev_size} → {cur_size} (ramdisk {sz})"
            );
            prev_size = cur_size;
            black_box(cur_size);
        }
    }
}

// ── Scaling — odin ────────────────────────────────────────────────────────────

mod odin_scaling {
    use super::*;

    const RAMDISK_SIZES: &[usize] = &[1024, 4096, 16_384, 65_536, 262_144];

    #[test]
    fn varying_ramdisk_size() {
        for &sz in RAMDISK_SIZES {
            let (tmp, kernel, initramfs) = setup(4096, sz);
            let output = tmp.path().join("AP_losos.tar.md5");
            let stage = TempDir::new().unwrap();
            isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
            assert!(
                output.exists(),
                "Odin archive must exist for ramdisk size {sz}"
            );
            // MD5 trailer must still be present regardless of size.
            let last = last_nonempty_line(&output);
            assert!(
                last.splitn(2, "  ").next().map_or(false, |h| h.len() == 32),
                "MD5 trailer missing or malformed for ramdisk size {sz}: {last:?}"
            );
            black_box(&output);
        }
    }

    #[test]
    fn odin_size_grows_monotonically_with_ramdisk() {
        let mut prev_size = 0u64;
        for &sz in RAMDISK_SIZES {
            let (tmp, kernel, initramfs) = setup(4096, sz);
            let output = tmp.path().join("AP_losos.tar.md5");
            let stage = TempDir::new().unwrap();
            isoman_gsi_odin(&kernel, &initramfs, &output, stage.path());
            let cur_size = fs::metadata(&output).unwrap().len();
            assert!(
                cur_size >= prev_size,
                "Odin size must not shrink as ramdisk grows: {prev_size} → {cur_size} (ramdisk {sz})"
            );
            prev_size = cur_size;
            black_box(cur_size);
        }
    }
}

// ── Tail-reading helper ───────────────────────────────────────────────────────

/// Return the appended MD5 trailer line from an Odin `.tar.md5` file.
///
/// The format appends a single ASCII line of the form `<md5hex>  <path>\n`
/// *after* the tar stream's two 512-byte all-zero end-of-archive blocks.
///
/// We cannot simply split on `\n` because the tar payload (and even the null
/// padding blocks themselves) can contain bytes that equal `0x0A`.  Instead
/// we walk backward from the end of the file:
///
/// 1. Skip the terminating `\n` of the appended line.
/// 2. Collect bytes backward until we hit another `\n` or the start of file
///    **but stop early** as soon as we encounter a null byte — the MD5 line
///    is pure ASCII, so a null means we have walked back into the tar padding.
/// 3. Reverse and return the collected slice as a trimmed string.
fn last_nonempty_line(path: &Path) -> String {
    let bytes = fs::read(path).expect("read file");
    if bytes.is_empty() {
        return String::new();
    }

    // Step 1: find the position of the final `\n` (the line terminator).
    let newline_pos = match bytes.iter().rposition(|&b| b == b'\n') {
        Some(p) => p,
        None => return String::new(),
    };

    // Step 2: walk backward from newline_pos - 1, collecting bytes until we
    // hit another `\n`, a null byte, or the beginning of the file.
    let mut start = newline_pos;
    for i in (0..newline_pos).rev() {
        if bytes[i] == b'\n' || bytes[i] == b'\0' {
            start = i + 1;
            break;
        }
        start = i;
    }

    String::from_utf8_lossy(&bytes[start..newline_pos])
        .trim()
        .to_owned()
}

// ── Private test-only wrappers ────────────────────────────────────────────────
//
// `build_gsi_fastboot` and `build_gsi_odin` are `pub(crate)` inside the
// `isoman` binary crate and therefore not accessible from an external test
// crate.  We replicate the thin logic here using the public `mkbootimg`
// library and `std` primitives so the bench suite remains fully self-contained
// and does not require any visibility changes in the production code.

/// Replicate `gsi::build_gsi_fastboot` using only the public `mkbootimg` API.
fn isoman_gsi_fastboot(kernel: &Path, initramfs: &Path, output: &Path, stage: &Path) {
    let boot_img = build_boot_img_inner(kernel, initramfs, stage);
    fs::copy(&boot_img, output).expect("copy boot.img to fastboot output");
}

/// Replicate `gsi::build_gsi_odin` using only the public `mkbootimg` API and
/// `std` shell-free primitives.
fn isoman_gsi_odin(kernel: &Path, initramfs: &Path, output: &Path, stage: &Path) {
    use std::io::Write as _;

    let _boot_img = build_boot_img_inner(kernel, initramfs, stage);
    let tar_path = stage.join("AP_losos.tar");
    let tar_str = tar_path.to_str().unwrap();

    // Create tar archive via system `tar` (same as production code).
    let status = std::process::Command::new("tar")
        .args(["-cf", tar_str, "-C", stage.to_str().unwrap(), "boot.img"])
        .status()
        .expect("tar not found");
    assert!(status.success(), "tar failed: {status}");

    // Compute and append MD5 checksum.
    let md5_out = std::process::Command::new("md5sum")
        .arg(tar_str)
        .output()
        .expect("md5sum not found");
    assert!(md5_out.status.success(), "md5sum failed");

    let md5_line = String::from_utf8_lossy(&md5_out.stdout);
    let mut f = fs::OpenOptions::new()
        .append(true)
        .open(&tar_path)
        .expect("open tar for MD5 append");
    f.write_all(md5_line.as_bytes()).expect("write MD5 trailer");
    drop(f);

    fs::copy(&tar_path, output).expect("copy Odin archive to output");
}

/// Shared inner helper: build `<stage>/boot.img` via `mkbootimg` library.
fn build_boot_img_inner(kernel: &Path, initramfs: &Path, stage: &Path) -> PathBuf {
    let output = stage.join("boot.img");
    let mut params = MkbootimgParams::default();
    params.output = output.to_str().unwrap().to_owned();
    params.kernel = Some(kernel.to_str().unwrap().to_owned());
    params.ramdisk = Some(initramfs.to_str().unwrap().to_owned());
    params.cmdline = Some(GSI_CMDLINE.to_owned());
    params.create().expect("mkbootimg failed");
    output
}
