use std::{fs::remove_file, path::Path};

use clap::Parser;
use tracing::{info, warn};
use tracing_subscriber::fmt;

use crate::{install::PackageInstallation, run::ProgRunner};

mod install;
mod run;

#[derive(Parser)]
#[clap(name = "pakman", version, about, long_about = None)]
pub struct CLIface {
    #[arg(long = "install")]
    install: Option<Vec<String>>,
    #[arg(long = "remove")]
    remove: Option<Vec<String>>,
    #[arg(long = "run")]
    run: Option<String>,
}
fn main() -> miette::Result<()> {
    fmt().init();
    let args = CLIface::parse();
    let mut installation = PackageInstallation::new();
    if let Some(i) = args.install {
        i.iter().for_each(|prog| installation.add_to_queue(prog));
        installation.start()?;
    } else if let Some(rm) = args.remove {
        rm.iter().for_each(|to_rm| {
            info!("Removing {to_rm}");
            remove_file(Path::new("/data/progs").join(format!("{to_rm}.tar"))).unwrap();
        });
    } else if let Some(run) = args.run {
        let pr = ProgRunner::new();
        info!("Running {run}");
        pr.run(&run)?;
    } else {
        warn!("Run pakman --help for help.");
    }
    Ok(())
}
