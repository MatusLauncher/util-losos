use std::{
    env::temp_dir,
    fs::{create_dir_all, read_to_string, write},
    path::Path,
    process::Command,
    thread::scope,
};

use actman::cmdline::CmdLineOptions;
use miette::{IntoDiagnostic, miette};
use rustix::mount::{MountFlags, mount};
use tracing::{info, warn};

#[derive(Default)]
pub struct PackageInstallation {
    install_tasks: Vec<String>,
    lineopts: CmdLineOptions,
}

impl PackageInstallation {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn add_to_queue(&mut self, pkg: &str) {
        self.install_tasks.push(pkg.into());
    }
    pub fn start(&self) -> miette::Result<()> {
        info!("Checking whether data drive is mounted for persistency");
        let ddrive = self.lineopts.opts().get("data_drive");
        if ddrive.is_none() {
            return Err(miette!("Cannot continue, no data_drive is set."));
        }
        let mounts = read_to_string("/proc/mounts").into_diagnostic()?;
        let actual_mount = mounts
            .lines()
            .filter(|d| d.contains(self.lineopts.opts().get("data_drive").unwrap()))
            .map(|drive| drive.to_string())
            .collect::<String>();
        let mount_dir = actual_mount.split_whitespace().collect::<Vec<_>>()[1];
        if !mount_dir.starts_with("/") {
            info!("Mounting {} to /data", ddrive.unwrap());
            mount(ddrive.unwrap(), "/data", "", MountFlags::all(), None).into_diagnostic()?;
        } else {
            warn!("{} is already mounted, continuing", ddrive.unwrap());
        }
        match create_dir_all("/data/progs").into_diagnostic() {
            Ok(_) => (),
            Err(_) => warn!("The program directory probably exists, continuing"),
        };
        scope(|thread| {
            self.install_tasks.iter().for_each(|task| {
                thread.spawn(move || -> miette::Result<()> {
                    let path = temp_dir().join(task);
                    let join = Path::new("/data").join("progs");
                    let perm_path = join.join(format!("{task}.tar"));
                    write(
                        &path,
                        format!(
                            "FROM nixos/nix as base\nENTRYPOINT nix-shell -p {task} --run {task}"
                        ),
                    )
                    .into_diagnostic()?;
                    info!("Building the image with a system container runtime...");
                    let mut command = Command::new("/bin/nerdctl");
                    command
                        .arg("build")
                        .arg(&path)
                        .arg("-t")
                        .arg(format!("local/{task}"))
                        .status()
                        .into_diagnostic()?;
                    command
                        .arg("save")
                        .arg(format!("local/{task}"))
                        .arg("-o")
                        .arg(perm_path)
                        .spawn()
                        .into_diagnostic()?;
                    info!("DONE! Restart this machine to clean up build cache");
                    Ok(())
                });
            });
        });
        Ok(())
    }
}
