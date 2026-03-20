use std::process::Command;

use miette::IntoDiagnostic;
use tracing::info;
use walkdir::WalkDir;

use crate::cmdline::CmdLineOptions;
#[derive(Debug, Clone)]
pub struct Preboot {
    mounts: Vec<String>,
}
#[allow(trivial_bounds)]
impl Default for Preboot {
    fn default() -> Self {
        Self {
            mounts: WalkDir::new("/")
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    name != "home" && name != "etc" && name != "bin" && name != "sbin"
                })
                .filter_map(|e| e.ok())
                .map(|e| e.path().display().to_string())
                .collect(),
        }
    }
}

#[allow(trivial_bounds)]
impl Preboot {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn mount(&self) -> miette::Result<()> {
        let binding = CmdLineOptions::new()?;
        let params = binding.opts();
        let data_part = params.get("data").unwrap();
        Command::new("mount")
            .arg(data_part)
            .arg("/data")
            .output()
            .into_diagnostic()?;
        Ok(self
            .mounts
            .iter()
            .try_for_each(|mount| -> miette::Result<()> {
                Ok({
                    info!("Mounting {mount} to /{mount}");
                    Command::new("mount")
                        .arg("-t")
                        .arg(mount)
                        .arg(format!("/{mount}"))
                        .spawn()
                        .into_diagnostic()?;
                })
            })?)
    }
}
