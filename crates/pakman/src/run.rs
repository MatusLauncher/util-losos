use std::process::Command;

use miette::IntoDiagnostic;
use tracing::info;
use walkdir::WalkDir;

#[derive(Default)]
pub struct ProgRunner;

impl ProgRunner {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn run(&self, prog: &str) -> miette::Result<()> {
        info!("Loading the program from the data_drive");
        let p = WalkDir::new("/data/progs")
            .into_iter()
            .filter(|p| {
                let dir_entry = p.as_ref().unwrap();
                dir_entry.path().display().to_string().contains(prog)
            })
            .map(|fname| {
                let dir_entry = fname.as_ref();
                let fname = dir_entry.unwrap().path();
                fname.display().to_string()
            })
            .collect::<Vec<_>>()[0]
            .clone();
        Command::new("/bin/nerdctl")
            .arg("load")
            .arg("-i")
            .arg(p)
            .status()
            .into_diagnostic()?;
        info!("Starting up {prog}");
        Command::new("/bin/nerdctl")
            .arg("run")
            .arg("-it")
            .arg(format!("localhost/local/{prog}"));
        Ok(())
    }
}
