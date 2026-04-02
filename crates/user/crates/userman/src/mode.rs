//! Runtime operation-mode detection.
//!
//! The single `userman` binary reads its own executable name at startup and
//! dispatches to the appropriate code path via [`ModeOfOperation`].

use serde::Deserialize;

/// The role the process should fulfil, derived from the executable name.
pub enum ModeOfOperation {
    /// Interactive CLI client — parses subcommands and talks to the daemon.
    Client,
    /// Background HTTP daemon. The inner [`Location`] controls which network
    /// origins are accepted.
    Daemon(Location),
    /// Interactive login screen; authenticates a user before starting a session.
    LoginScreen,
}

/// Whether the daemon accepts only loopback connections or remote ones.
#[derive(Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
pub enum Location {
    /// Accept connections from `127.0.0.1` / `::1` only.
    #[default]
    Local,
    /// Accept connections from non-loopback addresses.
    Remote,
}

impl From<String> for ModeOfOperation {
    /// Map an executable name to its [`ModeOfOperation`].
    ///
    /// Unknown names fall back to [`ModeOfOperation::Client`].
    fn from(value: String) -> Self {
        match &*value {
            "usersvc-local" => Self::Daemon(Location::default()),
            "usersvc-remote" => Self::Daemon(Location::Remote),
            "userman" | "useradd" => Self::Client,
            "login" => Self::LoginScreen,
            _ => Self::Client,
        }
    }
}
