#![feature(trivial_bounds, const_trait_impl, const_default)]
use crate::{preboot::Preboot, reboot::RebootCMD};
use std::{path::Path, process::Command, str::FromStr, sync::LazyLock};

use miette::IntoDiagnostic;
use rustix::system::{RebootCommand, reboot};
use strum::IntoEnumIterator;
use tracing::info;
use tracing_subscriber::fmt;
use walkdir::{Error, WalkDir};
mod preboot;
mod reboot;

static PREBOOT: LazyLock<Preboot> = LazyLock::new(|| Preboot::new());

#[allow(trivial_bounds)]
fn main() -> miette::Result<()> {
    fmt().init();
    let args: Vec<_> = std::env::args().collect();
    match RebootCMD::from_str(
        &*Path::new(&args[0])
            .file_name()
            .unwrap()
            .display()
            .to_string(),
    )
    .into_diagnostic()?
    {
        RebootCMD::Init => {
            Preboot::new().mount()?;
            for scripts in WalkDir::new("/etc/init/start") {
                let dir_entry = scripts.into_diagnostic()?;
                let script = dir_entry.path();
                info!("Spawning {}.", script.display());
                Command::new(script).spawn().into_diagnostic()?;
            }
        }
        RebootCMD::PowerOff => {
            info!("Powering off");
            for scripts in WalkDir::new("/etc/init/stop") {
                let dir_entry = scripts.into_diagnostic()?;
                let script = dir_entry.path();
                info!("Shutting down {}.", script.display());
                Command::new(script).spawn().into_diagnostic()?;
            }
            reboot(RebootCommand::PowerOff).into_diagnostic()?;
        }
        RebootCMD::Reboot => {
            info!("Rebooting...");
            for scripts in WalkDir::new("/etc/init/stop") {
                let dir_entry = scripts.into_diagnostic()?;
                let script = dir_entry.path();
                info!("Shutting down {}.", script.display());
                Command::new(script).spawn().into_diagnostic()?;
            }
            reboot(RebootCommand::Restart).into_diagnostic()?;
        }
        _ => info!(
            "You've probably called the wrong binary. Make a symbolic link from this binary to one of these: {syms:?} to use it properly.",
            syms = RebootCMD::iter()
                .filter(|cadoff| *cadoff != RebootCMD::CadOff)
                .collect::<Vec<_>>()
        ),
    }
    Ok(())
}
