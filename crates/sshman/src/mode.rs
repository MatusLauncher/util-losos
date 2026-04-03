//! Runtime operation-mode detection.
//!
//! The single `sshman` binary reads its own executable name at startup and
//! dispatches to the appropriate code path via [`ModeOfOperation`].

/// The role the process should fulfil, derived from the executable name.
pub enum ModeOfOperation {
    /// SSH daemon — listen on port 22 and serve SSH connections.
    Daemon,
    /// Unknown invocation.
    Unknown,
}

impl From<String> for ModeOfOperation {
    /// Map an executable name to its [`ModeOfOperation`].
    fn from(value: String) -> Self {
        match &*value {
            "sshman" | "sshd" => Self::Daemon,
            _ => Self::Unknown,
        }
    }
}
