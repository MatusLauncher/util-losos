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

#[cfg(test)]
mod tests {
    use super::CmdLineOptions;

    #[test]
    fn parses_key_value_pairs() {
        let map = CmdLineOptions::param_search("console=ttyS0 earlyprintk=ttyS0".to_string());
        assert_eq!(map.get("console").map(String::as_str), Some("ttyS0"));
        assert_eq!(map.get("earlyprintk").map(String::as_str), Some("ttyS0"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn bare_flags_are_dropped() {
        let map = CmdLineOptions::param_search("quiet ro splash".to_string());
        assert!(map.is_empty(), "bare flags must be silently dropped");
    }

    #[test]
    fn mixed_flags_and_pairs() {
        let map = CmdLineOptions::param_search("quiet console=ttyS0 ro".to_string());
        assert_eq!(map.len(), 1);
        assert_eq!(map.get("console").map(String::as_str), Some("ttyS0"));
    }

    #[test]
    fn multiple_spaces_between_tokens() {
        let map = CmdLineOptions::param_search("  a=1   b=2  ".to_string());
        assert_eq!(map.get("a").map(String::as_str), Some("1"));
        assert_eq!(map.get("b").map(String::as_str), Some("2"));
        assert_eq!(map.len(), 2);
    }

    #[test]
    fn empty_input_gives_empty_map() {
        let map = CmdLineOptions::param_search(String::new());
        assert!(map.is_empty());
    }

    #[test]
    fn value_containing_equals_splits_on_first_only() {
        // Only the first '=' is the separator; the rest belongs to the value.
        let map = CmdLineOptions::param_search("url=http://host/path?a=1&b=2".to_string());
        assert_eq!(
            map.get("url").map(String::as_str),
            Some("http://host/path?a=1&b=2")
        );
        assert_eq!(map.len(), 1);
    }
}
