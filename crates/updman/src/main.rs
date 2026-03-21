#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/updman.md"))]

use std::fs::read_to_string;

use miette::IntoDiagnostic;
use tracing_subscriber::fmt;

use crate::schema::UpdMan;

mod schema;
fn main() -> miette::Result<()> {
    fmt().init();
    let cfg: UpdMan = serde_json::from_str(&read_to_string("/etc/update.json").into_diagnostic()?)
        .into_diagnostic()?;
    cfg.update()?;
    Ok(())
}
