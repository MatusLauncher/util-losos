//! Kernel command-line parser for `bootenv`.
//!
//! Reads `/proc/cmdline` and extracts the parameters needed by the stage-1
//! boot environment:
//!
//! | Parameter | Description |
//! |-----------|-------------|
//! | `stage2`  | Path to the stage-2 archive (`.tar.gz` / `.tar.zst`) or block device containing the real root filesystem. |
//! | `boot_luks` | Optional: path to a LUKS-encrypted boot partition to unlock before loading stage 2. |
//! | `boot_luks_keyfile` | Optional: path to a keyfile for the LUKS boot partition. |

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

    /// Returns the `stage2` parameter — the path to the real root filesystem.
    ///
    /// # Errors
    ///
    /// Returns an error if `stage2` is not present in the command line.
    pub fn stage2(&self) -> miette::Result<String> {
        self.raw.get("stage2").cloned().ok_or_else(|| {
            miette::miette!("stage2 parameter not found in /proc/cmdline")
        })
    }

    /// Returns the `boot_luks` parameter — path to an encrypted boot partition.
    ///
    /// Returns `None` if the parameter is absent (boot partition is not encrypted).
    #[must_use]
    pub fn boot_luks(&self) -> Option<String> {
        self.raw.get("boot_luks").cloned()
    }

    /// Returns the `boot_luks_keyfile` parameter — path to a LUKS keyfile.
    ///
    /// Returns `None` if the parameter is absent (console prompt fallback).
    #[must_use]
    pub fn boot_luks_keyfile(&self) -> Option<String> {
        self.raw.get("boot_luks_keyfile").cloned()
    }

    /// Returns `true` when a LUKS boot partition is configured.
    #[must_use]
    pub fn has_boot_luks(&self) -> bool {
        self.boot_luks().is_some()
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
    fn parses_stage2() {
        let map = BootCmdline::parse(
            "console=ttyS0 stage2=/dev/sda2 boot_luks=/dev/sda1",
        );
        assert_eq!(map.get("stage2"), Some(&"/dev/sda2".to_string()));
        assert_eq!(map.get("boot_luks"), Some(&"/dev/sda1".to_string()));
    }

    #[test]
    fn stage2_missing_is_error() {
        let bc = BootCmdline {
            raw: BootCmdline::parse("console=ttyS0 quiet"),
        };
        assert!(bc.stage2().is_err());
    }

    #[test]
    fn boot_luks_absent_returns_none() {
        let bc = BootCmdline {
            raw: BootCmdline::parse("stage2=/dev/sda2"),
        };
        assert!(!bc.has_boot_luks());
        assert!(bc.boot_luks().is_none());
    }

    #[test]
    fn boot_luks_present() {
        let bc = BootCmdline {
            raw: BootCmdline::parse(
                "stage2=/real_root stage2=/dev/sda2 boot_luks=/dev/sda1 boot_luks_keyfile=/key.bin",
            ),
        };
        assert!(bc.has_boot_luks());
        assert_eq!(bc.boot_luks().as_deref(), Some("/dev/sda1"));
        assert_eq!(bc.boot_luks_keyfile().as_deref(), Some("/key.bin"));
    }

    #[test]
    fn bare_flags_are_discarded() {
        let map = BootCmdline::parse("quiet ro stage2=/dev/sda2");
        assert_eq!(map.len(), 1);
    }
}
