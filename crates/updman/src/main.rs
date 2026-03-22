#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/updman.md"))]

use std::sync::LazyLock;

use tracing_subscriber::fmt;

use crate::schema::UpdMan;
static mut UPDMAN: LazyLock<UpdMan> = LazyLock::new(|| UpdMan::default());
mod schema;
fn main() -> miette::Result<()> {
    fmt().init();
    #[allow(static_mut_refs)]
    (unsafe { UPDMAN.update() })?;
    Ok(())
}
