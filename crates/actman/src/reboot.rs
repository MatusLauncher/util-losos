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

impl<'a> From<&'a str> for RebootCMD {
    /// Derives the mode from the basename of `argv[0]`.
    ///
    /// | Basename    | Variant              |
    /// |-------------|----------------------|
    /// | `"init"`    | [`RebootCMD::Init`]     |
    /// | `"poweroff"`| [`RebootCMD::PowerOff`] |
    /// | `"reboot"`  | [`RebootCMD::Reboot`]   |
    /// | _anything else_ | [`RebootCMD::CadOff`] |
    fn from(value: &'a str) -> Self {
        let basename = std::path::Path::new(value)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(value);
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
