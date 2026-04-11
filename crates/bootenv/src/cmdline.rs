//! Kernel command-line parser for `bootenv`.
//!
//! Reads `/proc/cmdline` and extracts the parameters needed by the stage-1
//! boot environment:
//!
//! | Parameter | Description |
//! |-----------|-------------|
//! | `data_drive` | Semicolon-separated list of block/NFS URIs for the persistent data volume. |

use std::{collections::HashMap, fs::read_to_string};

use miette::{Context, IntoDiagnostic};
use tracing::info;

/// Parsed boot command line.
#[derive(Debug, Clone, Default)]
pub struct BootCmdline {
    raw: HashMap<String, String>,
}

impl BootCmdline {
    /// Reads `/proc/cmdline` and parses it into key=value pairs.
    pub fn new() -> miette::Result<Self> {
        let content = read_to_string("/proc/cmdline")
            .into_diagnostic()
            .wrap_err("cannot read /proc/cmdline — is /proc mounted?")?;
        let raw = Self::parse(&content);
        info!(?raw, "Parsed boot cmdline");
        Ok(Self { raw })
    }

    /// Returns the `data_drive` parameter — semicolon-separated list of
    /// block-device or NFS URIs that describe the persistent data volume.
    ///
    /// Returns `None` when the parameter is absent (RAM-only mode).
    #[must_use]
    pub fn data_drive(&self) -> Option<&str> {
        self.raw.get("data_drive").map(String::as_str)
    }

    /// Parses a raw cmdline string into a `HashMap`, splitting on whitespace
    /// then on `=`. Bare flags (no `=`) are discarded.
    fn parse(content: &str) -> HashMap<String, String> {
        content
            .split_whitespace()
            .filter_map(|token| {
                token
                    .split_once('=')
                    .map(|(k, v)| (k.to_string(), v.to_string()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_flags_are_discarded() {
        let map = BootCmdline::parse("quiet ro console=ttyS0");
        assert_eq!(map.len(), 1);
    }

    #[test]
    fn data_drive_absent_returns_none() {
        let bc = BootCmdline {
            raw: BootCmdline::parse("console=ttyS0"),
        };
        assert!(bc.data_drive().is_none());
    }

    #[test]
    fn data_drive_present_returns_value() {
        let bc = BootCmdline {
            raw: BootCmdline::parse(
                "console=ttyS0 data_drive=luks:///dev/sdb1",
            ),
        };
        assert_eq!(bc.data_drive(), Some("luks:///dev/sdb1"));
    }

    #[test]
    fn data_drive_nfs_uri_preserved() {
        let bc = BootCmdline {
            raw: BootCmdline::parse(
                "data_drive=nfs://fileserver/data?opts=nolock,vers=4",
            ),
        };
        assert_eq!(
            bc.data_drive(),
            Some("nfs://fileserver/data?opts=nolock,vers=4")
        );
    }
}
