use std::{
    env::{set_current_dir, temp_dir},
    fs::{create_dir_all, rename, write},
    process::Command,
};

use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize};
use tracing::info;
use walkdir::WalkDir;

#[derive(Serialize, Deserialize)]
pub struct UpdMan {
    base_url: String,
    image_tag: String,
    hash: String,
}

impl UpdMan {
    pub fn update(&self) -> miette::Result<()> {
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
