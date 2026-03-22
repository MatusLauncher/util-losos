//! Reboot command dispatch.
//!
//! Maps the binary's `argv[0]` basename to a [`RebootCMD`] variant, and
//! converts that variant into the corresponding [`rustix::system::RebootCommand`]
//! for the `reboot(2)` syscall.

use rustix::system::RebootCommand;
use strum::EnumIter;

/// The operating mode selected by the binary's `argv[0]` basename.
///
/// actman is installed as a single binary with symlinks:
///
/// ```text
/// /bin/init     → actman   (Init)
/// /bin/poweroff → actman   (PowerOff)
/// /bin/reboot   → actman   (Reboot)
/// ```
///
/// Any other basename resolves to [`CadOff`](RebootCMD::CadOff), which is
/// also the fallback for unrecognised [`RebootCommand`] variants.
#[derive(Debug, EnumIter, PartialEq, Eq, PartialOrd, Ord)]
pub enum RebootCMD {
    /// Normal init mode: mount filesystems and spawn startup scripts.
    Init,
    /// Halt and cut power via `LINUX_REBOOT_CMD_POWER_OFF`.
    PowerOff,
    /// Reboot the machine via `LINUX_REBOOT_CMD_RESTART`.
    Reboot,
    /// Disable Ctrl-Alt-Delete (`LINUX_REBOOT_CMD_CAD_OFF`).
    /// Used as the catch-all fallback variant.
    CadOff,
}

impl<'a> From<&'a String> for RebootCMD {
    /// Derives the mode from the basename of `argv[0]`.
    ///
    /// | Basename    | Variant              |
    /// |-------------|----------------------|
    /// | `"init"`    | [`RebootCMD::Init`]     |
    /// | `"poweroff"`| [`RebootCMD::PowerOff`] |
    /// | `"reboot"`  | [`RebootCMD::Reboot`]   |
    /// | _anything else_ | [`RebootCMD::CadOff`] |
    fn from(value: &'a String) -> Self {
        let basename = std::path::Path::new(value)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(value.as_str());
        match basename {
            "init" => Self::Init,
            "poweroff" => Self::PowerOff,
            "reboot" => Self::Reboot,
            _ => Self::CadOff,
        }
    }
}

impl From<RebootCommand> for RebootCMD {
    /// Converts a [`RebootCommand`] syscall constant back to a [`RebootCMD`].
    fn from(value: RebootCommand) -> Self {
        match value {
            RebootCommand::Restart => Self::Reboot,
            RebootCommand::PowerOff => Self::PowerOff,
            _ => Self::CadOff,
        }
    }
}

impl From<RebootCMD> for RebootCommand {
    /// Converts a [`RebootCMD`] to the [`RebootCommand`] passed to `reboot(2)`.
    ///
    /// Both [`Init`](RebootCMD::Init) and [`CadOff`](RebootCMD::CadOff) map to
    /// [`RebootCommand::CadOff`] as a safe no-op default.
    fn from(value: RebootCMD) -> Self {
        match value {
            RebootCMD::Reboot => Self::Restart,
            RebootCMD::PowerOff => Self::PowerOff,
            _ => Self::CadOff,
        }
    }
}

#[cfg(test)]
mod tests {
    use rustix::system::RebootCommand;

    use super::RebootCMD;

    // ── From<&String> ────────────────────────────────────────────────────────

    #[test]
    fn basename_init_maps_to_init() {
        assert_eq!(RebootCMD::from(&"init".to_string()), RebootCMD::Init);
    }

    #[test]
    fn basename_poweroff_maps_to_poweroff() {
        assert_eq!(
            RebootCMD::from(&"poweroff".to_string()),
            RebootCMD::PowerOff
        );
    }

    #[test]
    fn basename_reboot_maps_to_reboot() {
        assert_eq!(RebootCMD::from(&"reboot".to_string()), RebootCMD::Reboot);
    }

    #[test]
    fn unrecognised_basename_maps_to_cadoff() {
        assert_eq!(RebootCMD::from(&"foobar".to_string()), RebootCMD::CadOff);
        assert_eq!(RebootCMD::from(&"shutdown".to_string()), RebootCMD::CadOff);
    }

    #[test]
    fn path_prefixed_init_resolves_correctly() {
        assert_eq!(RebootCMD::from(&"/bin/init".to_string()), RebootCMD::Init);
    }

    #[test]
    fn path_prefixed_poweroff_resolves_correctly() {
        assert_eq!(
            RebootCMD::from(&"/bin/poweroff".to_string()),
            RebootCMD::PowerOff,
        );
    }

    #[test]
    fn path_prefixed_reboot_resolves_correctly() {
        assert_eq!(
            RebootCMD::from(&"/bin/reboot".to_string()),
            RebootCMD::Reboot,
        );
    }

    #[test]
    fn path_prefixed_unknown_maps_to_cadoff() {
        assert_eq!(
            RebootCMD::from(&"/usr/sbin/unknown".to_string()),
            RebootCMD::CadOff,
        );
    }

    // ── RebootCMD → RebootCommand ─────────────────────────────────────────────

    #[test]
    fn reboot_cmd_reboot_maps_to_restart() {
        let rc: RebootCommand = RebootCMD::Reboot.into();
        assert_eq!(rc, RebootCommand::Restart);
    }

    #[test]
    fn reboot_cmd_poweroff_maps_to_poweroff() {
        let rc: RebootCommand = RebootCMD::PowerOff.into();
        assert_eq!(rc, RebootCommand::PowerOff);
    }

    #[test]
    fn reboot_cmd_init_maps_to_cadoff() {
        let rc: RebootCommand = RebootCMD::Init.into();
        assert_eq!(rc, RebootCommand::CadOff);
    }

    #[test]
    fn reboot_cmd_cadoff_maps_to_cadoff() {
        let rc: RebootCommand = RebootCMD::CadOff.into();
        assert_eq!(rc, RebootCommand::CadOff);
    }

    // ── RebootCommand → RebootCMD ─────────────────────────────────────────────

    #[test]
    fn reboot_command_restart_maps_to_reboot() {
        assert_eq!(RebootCMD::from(RebootCommand::Restart), RebootCMD::Reboot);
    }

    #[test]
    fn reboot_command_poweroff_maps_to_poweroff() {
        assert_eq!(
            RebootCMD::from(RebootCommand::PowerOff),
            RebootCMD::PowerOff,
        );
    }

    #[test]
    fn reboot_command_cadoff_maps_to_cadoff() {
        assert_eq!(RebootCMD::from(RebootCommand::CadOff), RebootCMD::CadOff);
    }
}
