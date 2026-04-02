//! Update configuration schema and orchestration.
//!
//! Defines [`UpdMan`], which is deserialised from `/etc/update.json` and
//! drives the full update sequence via [`UpdMan::update`].

use std::{
    env::{set_current_dir, temp_dir},
    fs::{self, create_dir_all, rename},
    io,
    process::{Command, Stdio},
};

use actman::{cmdline::CmdLineOptions, persistence::Persistence};
use miette::{IntoDiagnostic, bail};
use tracing::{info, warn};
/// Central update-manager type for the MTOS update sequence.
///
/// Holds the container registry coordinates that are read from the kernel
/// command line at startup and uses them to drive the full update pipeline.
///
/// # Entry point
///
/// The main entry point is [`UpdMan::update`], which executes every step of
/// the update sequence in order: pull → save → extract → install → unmount.
///
/// # Construction
///
/// | Method | When to use |
/// |--------|-------------|
/// | [`UpdMan::new`]`(base_url, image_tag, hash)` | Tests and programmatic use where the values are already known. |
/// | [`Default::default`]`()` | Production path — reads `base_url`, `tag`, and `hash` directly from `/proc/cmdline` via [`CmdLineOptions`]. |
pub struct UpdMan {
    /// Container registry prefix, e.g. `"registry.example.com/mtos-v2"`.
    /// Combined with [`image_tag`](UpdMan::image_tag) as `<base_url>/<image_tag>`
    /// when calling `nerdctl save`.
    base_url: String,

    /// Image name and tag, e.g. `"util-mdl:latest"`.
    image_tag: String,

    /// Reserved for future integrity verification. Not currently validated.
    #[allow(dead_code)]
    hash: String,
}

impl Default for UpdMan {
    fn default() -> Self {
        let cmdline: CmdLineOptions = CmdLineOptions::new().unwrap();
        let opts = cmdline.opts();
        Self {
            base_url: opts.get("base_url").unwrap().to_owned(),
            image_tag: opts.get("tag").unwrap().to_owned(),
            hash: opts.get("hash").unwrap().to_owned(),
        }
    }
}

impl UpdMan {
    /// Constructs an [`UpdMan`] directly from the given field values.
    /// Useful in tests and when the values are known without reading the kernel
    /// command line.
    pub fn new(
        base_url: impl Into<String>,
        image_tag: impl Into<String>,
        hash: impl Into<String>,
    ) -> Self {
        Self {
            base_url: base_url.into(),
            image_tag: image_tag.into(),
            hash: hash.into(),
        }
    }

    /// Returns the fully-qualified image reference used when calling
    /// `nerdctl pull` / `nerdctl save`.
    ///
    /// # Example
    ///
    /// ```text
    /// // base_url = "registry.example.com/mtos-v2"
    /// // image_tag = "util-mdl:latest"
    /// // → "registry.example.com/mtos-v2/util-mdl:latest"
    /// ```
    pub fn image_ref(&self) -> String {
        format!("{}/{}", self.base_url, self.image_tag)
    }

    /// Pulls the new OS image and installs it onto the `BOOT` partition.
    ///
    /// # Steps
    ///
    /// 1. `nerdctl save <base_url>/<image_tag>` — exports the image tarball to `dl.tar`.
    /// 2. `tar -xvf dl.tar -C $TMPDIR/out/` — extracts the OCI layer tarballs.
    /// 3. Locates the first `.tar` layer file inside `$TMPDIR/out/`.
    /// 4. `tar -xvf <layer>.tar` — extracts `os.initramfs.tar.gz` from the layer.
    /// 5. Mounts `/dev/disk/by-label/BOOT` at `$TMPDIR/mnt/`.
    /// 6. Moves `os.initramfs.tar.gz` onto the boot partition.
    /// 7. Unmounts the boot partition.
    ///
    /// # Errors
    ///
    /// Returns a [`miette::Report`] if any subprocess fails or a filesystem
    /// operation cannot be completed.
    pub fn update(&self) -> miette::Result<()> {
        info!("Pulling new MDL image from registry...");
        Command::new("nerdctl")
            .arg("pull")
            .arg("--insecure-registry")
            .arg(self.image_ref())
            .output()
            .into_diagnostic()?;
        info!("Downloading new MDL tarball...");
        // Stream nerdctl-save stdout directly into dl.tar so the entire
        // tarball (potentially several GiB) is never buffered in RAM.
        let mut nerdctl = Command::new("nerdctl")
            .arg("save")
            .arg(self.image_ref())
            .stdout(Stdio::piped())
            .spawn()
            .into_diagnostic()?;
        let mut dl_file = fs::File::create("dl.tar").into_diagnostic()?;
        io::copy(
            nerdctl.stdout.as_mut().expect("stdout is piped"),
            &mut dl_file,
        )
        .into_diagnostic()?;
        let status = nerdctl.wait().into_diagnostic()?;
        if !status.success() {
            bail!("nerdctl save failed (exit {})", status);
        }
        drop(dl_file);
        create_dir_all(temp_dir().join("out")).into_diagnostic()?;
        create_dir_all(temp_dir().join("mnt")).into_diagnostic()?;

        // Mount the BOOT partition to a temporary directory so we can inspect
        // its current contents (the lower layer for the overlay).
        Command::new("mount")
            .arg("/dev/disk/by-label/BOOT")
            .arg(temp_dir().join("mnt"))
            .output()
            .into_diagnostic()?;

        // Wrap the BOOT mountpoint in a persistent overlay.  The new initramfs
        // is staged in the upper layer; only a successful CommitAtomic lands it
        // on the actual partition, so a power-loss mid-download leaves the BOOT
        // partition intact.
        let mut boot_persist = Persistence::new(temp_dir().join("mnt"));
        boot_persist.mount()?;
        let boot_overlay = boot_persist.mountpoint();

        let update_result: miette::Result<()> = (|| {
            Command::new("tar")
                .arg("-xvf")
                .arg("dl.tar")
                .arg("-C")
                .arg(temp_dir().join("out"))
                .output()
                .into_diagnostic()?;
            set_current_dir(temp_dir().join("out")).into_diagnostic()?;
            info!("Extracting the initramfs image...");
            // Find the first *.tar layer file with a plain read_dir — no
            // recursive walk needed since OCI layers sit directly in the out/ dir.
            let layer_tar = fs::read_dir(temp_dir().join("out"))
                .into_diagnostic()?
                .filter_map(|entry| entry.ok())
                .find(|entry| entry.file_name().to_string_lossy().ends_with(".tar"))
                .map(|entry| entry.path())
                .ok_or_else(|| miette::miette!("no .tar layer found in extraction directory"))?;
            Command::new("tar")
                .arg("-xvf")
                .arg(&layer_tar)
                .output()
                .into_diagnostic()?;
            info!("Moving the initramfs image to the boot partition overlay...");
            rename(
                temp_dir().join("out").join("os.initramfs.tar.gz"),
                boot_overlay.join("os.initramfs.tar.gz"),
            )
            .into_diagnostic()?;
            Ok(())
        })();

        match update_result {
            Ok(()) => {
                info!("Committing new initramfs to BOOT partition");
                boot_persist.commit();
            }
            Err(ref e) => {
                warn!("Update failed ({e}); discarding overlay — BOOT partition unchanged");
                boot_persist.discard();
            }
        }

        info!("Finishing up");
        Command::new("umount").arg("-R").arg(temp_dir().join("mnt"));
        update_result
    }
}
