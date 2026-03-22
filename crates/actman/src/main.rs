#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/actman.md"))]

use actman::{preboot::Preboot, reboot::RebootCMD};
use miette::IntoDiagnostic;
use rustix::process::{WaitOptions, waitpid};
use rustix::system::reboot;
use std::process::Command;
use strum::IntoEnumIterator;
use tracing::info;
use tracing_subscriber::fmt;
use walkdir::WalkDir;
#[allow(trivial_bounds)]
fn main() -> miette::Result<()> {
    fmt().init();
    let args: Vec<_> = std::env::args().collect();
    match RebootCMD::from(&args[0]) {
        RebootCMD::Init => {
            Preboot::new().mount()?;
            for scripts in WalkDir::new("/etc/init/start").min_depth(1) {
                let dir_entry = scripts.into_diagnostic()?;
                let script = dir_entry.path();
                info!("Spawning {}.", script.display());
                Command::new(script).spawn().into_diagnostic()?;
            }
            // PID 1 must never exit — reap zombie children forever.
            loop {
                let _ = waitpid(None, WaitOptions::empty());
            }
        }
        RebootCMD::PowerOff | RebootCMD::Reboot => {
            info!("Powering off");
            for scripts in WalkDir::new("/etc/init/stop").min_depth(1) {
                let dir_entry = scripts.into_diagnostic()?;
                let script = dir_entry.path();
                info!("Shutting down {}.", script.display());
                Command::new(script).spawn().into_diagnostic()?;
            }
            reboot(RebootCMD::from(&args[0]).into()).into_diagnostic()?;
        }
        _ => info!(
            "You've called the wrong binary. Make a symbolic link from this binary to one of these: {:?} to use it properly.",
            RebootCMD::iter()
                .filter(|cadoff| *cadoff != RebootCMD::CadOff)
                .map(|ops| format!("{ops:?}").to_lowercase())
                .collect::<Vec<_>>()
        ),
    }
    Ok(())
}
