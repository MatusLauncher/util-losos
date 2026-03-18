use std::process::Command;

use miette::IntoDiagnostic;
use walkdir::{Error, WalkDir};
#[derive(Debug, Clone, Copy)]
pub struct Preboot {
    mounts: Vec<String>,
}
#[allow(trivial_bounds)]
impl Default for Preboot
{
    fn default() -> Self {
        const SKIP: &[&str] = &["etc", "home", "bin", "sbin"];
    
        Self {
            mounts: WalkDir::new("/")
                .into_iter()
                .filter_entry(|e| {
                    let name = e.file_name().to_string_lossy();
                    !SKIP.contains(&name.as_ref())
                })
                .filter_map(|e| e.ok())
                .map(|e| e.path().display().to_string())
                .collect(),
        }
    }
}

#[allow(trivial_bounds)]
impl Preboot {
    pub const fn new() -> Self
    {
        Self::default()
    }
    pub fn mount(&self) -> miette::Result<()> {
        Ok(self
            .mounts
            .iter()
            .try_for_each(|mount| -> miette::Result<()> {
                Ok({
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
