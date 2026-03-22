#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/testman.md"))]
pub mod harness;

pub use harness::{HarnessConfig, TestHarness};
