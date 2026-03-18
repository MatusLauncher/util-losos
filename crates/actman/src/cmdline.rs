use std::{collections::HashMap, fs::read_to_string};

use miette::IntoDiagnostic;
#[derive(Debug, Default)]
pub struct CmdLineOptions {
    opts: HashMap<String, String>,
}
impl CmdLineOptions {
    /// initalizes Self with parameters from /proc/cmdline.
    pub fn new() -> miette::Result<Self> {
        let f = read_to_string("/proc/cmdline").into_diagnostic()?;
        let base = Self::param_search(f);
        Ok(Self { opts: base })
    }
    /// Getter to the options.
    pub fn opts(&self) -> &HashMap<String, String> {
        &self.opts
    }
    /// Searches for kernel paramters in /proc/cmdline.
    fn param_search(f: String) -> HashMap<String, String> {
        let base: HashMap<String, String> = f
            .split_whitespace()
            .filter_map(|kv| {
                kv.split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect();
        base
    }
}

