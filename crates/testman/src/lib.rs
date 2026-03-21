#![doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/testman.md"))]
pub mod harness;
pub mod suite;

pub use harness::{HarnessConfig, TestHarness};
pub use suite::{TestCase, TestReport, TestResult, TestSuite};
