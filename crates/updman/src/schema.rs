use std::process::Command;

use actman::cmdline::CmdLineOptions;
use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize)]
pub struct UpdMan {
    base_url: String,
    image_tag: String,
    hash: String
}

impl UpdMan {
    pub fn update(&self) -> miette::Result<()> {
        let params = CmdLineOptions::new()?.opts();
        info!("Downloading new MDL tarball...");
        let out = String::from_utf8(
            Command::new("nerdctl")
                .arg("pull")
                .arg(format!("{}/{}", self.base_url, self.image_tag))
                .output()
                .into_diagnostic()?
                .stdout,
        )
        .into_diagnostic()?;
        let hash = out.lines().last().unwrap();
        // if params.get("tb_hash").unwrap().contains(hash)
        Ok(())
    }
}
