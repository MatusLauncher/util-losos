//! `perman` — permission enforcement via Linux Landlock LSM.
//!
//! Exports [`apply_session_policy`], which installs a Landlock ruleset
//! restricting `READ_DIR` access (which covers `chdir(2)`) to the supplied
//! list of allowed directories.  The ruleset is inherited across `exec(2)`,
//! so calling this before exec'ing a shell enforces the policy for the entire
//! user session without any dynamic-linker tricks.
//!
//! Passing an empty slice is a no-op (unrestricted), preserving the existing
//! semantics where an empty `allowed_dirs` means no restriction.

use std::path::PathBuf;

use landlock::{AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr, RulesetStatus};
use miette::IntoDiagnostic;
use tracing::warn;

/// Install a Landlock filesystem policy for the current process.
///
/// After this call the kernel denies any `chdir(2)` (or directory `open(2)`)
/// whose target is outside the union of `allowed_dirs`.  The restriction is
/// inherited by all processes spawned via `exec(2)`, making it suitable for
/// session-level enforcement when called from a `login` binary before it
/// exec's the user's shell.
///
/// # Arguments
///
/// * `allowed_dirs` — directories to permit.  If empty, returns `Ok(())`
///   immediately without installing any policy (unrestricted access).
///
/// # Errors
///
/// Returns an error if the kernel rejects ruleset creation, rule addition, or
/// `landlock_restrict_self(2)`.  If the kernel does not enforce Landlock
/// (`CONFIG_SECURITY_LANDLOCK` not set, Linux < 5.13) a warning is logged and
/// `Ok(())` is returned so boot continues unimpeded.
pub fn apply_session_policy(allowed_dirs: &[PathBuf]) -> miette::Result<()> {
    if allowed_dirs.is_empty() {
        return Ok(());
    }

    let mut ruleset = Ruleset::default()
        .handle_access(AccessFs::ReadDir)
        .into_diagnostic()?
        .create()
        .into_diagnostic()?;

    for dir in allowed_dirs {
        let fd = PathFd::new(dir).into_diagnostic()?;
        ruleset = ruleset
            .add_rule(PathBeneath::new(fd, AccessFs::ReadDir))
            .into_diagnostic()?;
    }

    let status = ruleset.restrict_self().into_diagnostic()?;

    if status.ruleset == RulesetStatus::NotEnforced {
        warn!(
            "Landlock is not enforced by this kernel \
             (requires CONFIG_SECURITY_LANDLOCK, Linux ≥ 5.13) — session policy not applied"
        );
    }

    Ok(())
}
