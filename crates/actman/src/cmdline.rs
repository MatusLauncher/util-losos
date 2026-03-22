//! Kernel command-line parser.
//!
//! Reads `/proc/cmdline` and exposes the parsed `key=value` pairs as a
//! [`HashMap`](std::collections::HashMap). Parameters without a `=` separator
//! (bare flags) are silently ignored.

use std::{collections::HashMap, fs::read_to_string};

use miette::IntoDiagnostic;

/// Parsed representation of the kernel command line.
///
/// Populated from `/proc/cmdline` by splitting on whitespace and then on `=`.
/// Only `key=value` pairs are retained; bare flags are dropped.
///
/// # Example
///
/// Given `/proc/cmdline`:
/// ```text
/// console=ttyS0 earlyprintk=ttyS0 quiet
/// ```
/// `CmdLineOptions::new()` produces a map of:
/// ```text
/// { "console" => "ttyS0", "earlyprintk" => "ttyS0" }
/// ```
#[derive(Debug, Default)]
pub struct CmdLineOptions {
    opts: HashMap<String, String>,
}

impl CmdLineOptions {
    /// Reads `/proc/cmdline` and parses it into a [`CmdLineOptions`].
    pub fn new() -> miette::Result<Self> {
        let f = read_to_string("/proc/cmdline").into_diagnostic()?;
        let base = Self::param_search(f);
        Ok(Self { opts: base })
    }

    /// Returns a reference to the parsed key=value map.
    pub fn opts(&self) -> &HashMap<String, String> {
        &self.opts
    }

    /// Splits `f` on whitespace, then on `=`, collecting into a map.
    /// Entries without a `=` separator are discarded.
    pub(crate) fn param_search(f: String) -> HashMap<String, String> {
        f.split_whitespace()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }
}
