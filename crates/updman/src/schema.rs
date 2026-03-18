use std::process::Command;

use miette::IntoDiagnostic;
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Serialize, Deserialize)]
pub struct UpdMan {
    base_url: String,
    image_tag: String,
}

impl UpdMan {
    pub fn update(&self) -> miette::Result<()> {
        info!("Downloading new image");
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
        
        Ok(())
    }
}
