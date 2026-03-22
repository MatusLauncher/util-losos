//! Update configuration schema and orchestration.
//!
//! Defines [`UpdMan`], which is deserialised from `/etc/update.json` and
//! drives the full update sequence via [`UpdMan::update`].

use std::{
    env::{set_current_dir, temp_dir},
    fs::{create_dir_all, rename, write},
    process::Command,
};

use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize};
use tracing::info;
use walkdir::WalkDir;

/// Update configuration, deserialised from `/etc/update.json`.
///
/// # Example `/etc/update.json`
///
/// ```json
/// {
///   "base_url":  "registry.example.com/mtos-v2",
///   "image_tag": "util-mdl:latest",
///   "hash":      "sha256:abc123"
/// }
/// ```
#[derive(Serialize, Deserialize)]
pub struct UpdMan {
    /// Container registry prefix, e.g. `"registry.example.com/mtos-v2"`.
    /// Combined with [`image_tag`](UpdMan::image_tag) as `<base_url>/<image_tag>`
    /// when calling `nerdctl save`.
    base_url: String,

    /// Image name and tag, e.g. `"util-mdl:latest"`.
    image_tag: String,

    /// Reserved for future integrity verification. Not currently validated.
    hash: String,
}

impl UpdMan {
    /// Returns the fully-qualified image reference used when calling
    /// `nerdctl pull` / `nerdctl save`.
    ///
    /// # Example
    ///
    /// ```
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
            .arg(format!("{}/{}", self.base_url, self.image_tag))
            .output()
            .into_diagnostic()?;
        info!("Downloading new MDL tarball...");
        let out = String::from_utf8(
            Command::new("nerdctl")
                .arg("save")
                .arg(format!("{}/{}", self.base_url, self.image_tag))
                .output()
                .into_diagnostic()?
                .stdout,
        )
        .into_diagnostic()?;
        write("dl.tar", out).into_diagnostic()?;
        create_dir_all(temp_dir().join("out")).into_diagnostic()?;
        create_dir_all(temp_dir().join("mnt")).into_diagnostic()?;
        Command::new("mount")
            .arg("/dev/disk/by-label/BOOT")
            .arg(temp_dir().join("mnt"))
            .output()
            .into_diagnostic()?;
        Command::new("tar")
            .arg("-xvf")
            .arg("dl.tar")
            .arg("-C")
            .arg(temp_dir().join("out"))
            .output()
            .into_diagnostic()?;
        set_current_dir(temp_dir().join("out")).into_diagnostic()?;
        info!("Extracting the initramfs image...");
        Command::new("tar")
            .arg("-xvf")
            .arg(
                WalkDir::new(temp_dir().join("out"))
                    .into_iter()
                    .filter(|fname| {
                        fname
                            .as_ref()
                            .unwrap()
                            .file_name()
                            .display()
                            .to_string()
                            .ends_with(".tar")
                    })
                    .map(|v| v.unwrap().file_name().display().to_string())
                    .collect::<Vec<_>>()[0]
                    .clone(),
            )
            .output()
            .into_diagnostic()?;
        info!("Moving the initramfs image to the boot partition...");
        rename(
            temp_dir().join("out").join("os.initramfs.tar.gz"),
            temp_dir().join("mnt").join("os.initramfs.tar.gz"),
        )
        .into_diagnostic()?;
        info!("Finishing up");
        Command::new("umount").arg("-R").arg("mnt");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::UpdMan;

    fn make_updman() -> UpdMan {
        serde_json::from_str(
            r#"{"base_url":"registry.example.com/mtos-v2","image_tag":"util-mdl:latest","hash":"sha256:abc123"}"#,
        )
        .unwrap()
    }

    // ── serde round-trip ──────────────────────────────────────────────────────

    #[test]
    fn round_trip_preserves_all_fields() {
        let original = make_updman();
        let json = serde_json::to_string(&original).unwrap();
        let restored: UpdMan = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.base_url, original.base_url);
        assert_eq!(restored.image_tag, original.image_tag);
        assert_eq!(restored.hash, original.hash);
    }

    #[test]
    fn deserialise_preserves_base_url() {
        let u = make_updman();
        assert_eq!(u.base_url, "registry.example.com/mtos-v2");
    }

    #[test]
    fn deserialise_preserves_image_tag() {
        let u = make_updman();
        assert_eq!(u.image_tag, "util-mdl:latest");
    }

    #[test]
    fn deserialise_preserves_hash() {
        let u = make_updman();
        assert_eq!(u.hash, "sha256:abc123");
    }

    #[test]
    fn missing_field_is_an_error() {
        // "hash" is omitted — serde must return an error.
        let result: Result<UpdMan, _> = serde_json::from_str(
            r#"{"base_url":"registry.example.com","image_tag":"util-mdl:latest"}"#,
        );
        assert!(result.is_err(), "expected error for missing 'hash' field");
    }

    #[test]
    fn extra_field_is_silently_ignored() {
        // serde's default behaviour is to ignore unknown fields.
        let result: Result<UpdMan, _> = serde_json::from_str(
            r#"{"base_url":"reg.io","image_tag":"img:v1","hash":"sha256:ff","extra":"ignored"}"#,
        );
        assert!(
            result.is_ok(),
            "unexpected error when extra field present: {:?}",
            result.err()
        );
    }

    // ── image_ref ─────────────────────────────────────────────────────────────

    #[test]
    fn image_ref_combines_base_url_and_image_tag() {
        let u = make_updman();
        assert_eq!(
            u.image_ref(),
            "registry.example.com/mtos-v2/util-mdl:latest"
        );
    }

    #[test]
    fn image_ref_format_is_base_url_slash_image_tag() {
        let u: UpdMan = serde_json::from_str(
            r#"{"base_url":"myregistry.io","image_tag":"myimage:v2","hash":"sha256:00"}"#,
        )
        .unwrap();
        let expected = format!("{}/{}", u.base_url, u.image_tag);
        assert_eq!(u.image_ref(), expected);
    }
}
