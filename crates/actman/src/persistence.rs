//! Persistent overlay layer backed by `overlayfs_fuse`.
//!
//! [`Persistence`] wraps an [`OverlayFS`] session, exposing the three lifecycle
//! operations needed by the util-mdl crates: [`mount`](Persistence::mount),
//! [`commit`](Persistence::commit), and [`discard`](Persistence::discard).
//!
//! The overlay has three layers:
//!
//! - **lower** — the read-only base directory passed to [`Persistence::new`].
//!   This layer holds the last durably committed state.
//! - **upper** — a writable session layer derived automatically by
//!   `overlayfs_fuse` from the lower directory name.  All writes during the
//!   current session land here.
//! - **mountpoint** — the merged view that callers read from and write to.
//!   Accessible via [`handle`](Persistence::handle).
//!
//! On [`commit`](Persistence::commit) the upper layer is merged back into the
//! lower using [`OverlayAction::CommitAtomic`], making all session writes
//! durable across reboots.  [`discard`](Persistence::discard) rolls back by
//! dropping the upper layer without touching the lower.
//!
//! The underlying [`OverlayFS`] implements `Drop`, which unmounts the FUSE
//! session automatically — keeping the [`Persistence`] value alive is enough
//! to keep the overlay mounted.

use std::path::PathBuf;

use miette::IntoDiagnostic;
use overlayfs_fuse::{InodeMode, OverlayAction, OverlayFS};

/// A FUSE-backed overlay filesystem that provides durable write-through
/// persistence for a directory.
///
/// # Usage
///
/// ```no_run
/// use std::path::PathBuf;
/// use actman::persistence::Persistence;
///
/// let mut persist = Persistence::new(PathBuf::from("/data/state"));
/// persist.mount().expect("overlay mount failed");
///
/// // Write files into persist.handle().mountpoint ...
///
/// persist.commit(); // atomically merges changes into /data/state
/// ```
pub struct Persistence {
    overlay: OverlayFS,
}

impl Persistence {
    /// Creates a new overlay with `lower_dir` as the read-only base.
    ///
    /// Upper-layer and mountpoint paths are derived automatically by
    /// `overlayfs_fuse` from the lower directory name using its default
    /// naming conventions.  Call [`mount`](Self::mount) to activate the
    /// FUSE session before performing any I/O through the overlay.
    pub fn new(lower_dir: PathBuf) -> Self {
        let mut overlay = OverlayFS::new(lower_dir);
        overlay.set_inode_mode(InodeMode::Persistent);
        Self { overlay }
    }

    /// Starts the FUSE overlay session.
    ///
    /// After this returns, all writes directed at the overlay mountpoint
    /// (returned by [`handle`](Self::handle)) go into the upper layer and
    /// remain invisible to the lower directory until [`commit`](Self::commit)
    /// is called.
    pub fn mount(&mut self) -> miette::Result<()> {
        self.overlay.mount().into_diagnostic()
    }

    /// Returns the overlay mountpoint path.
    ///
    /// Direct all reads and writes through this path so they are tracked by
    /// the upper layer and visible in the merged view.  Changes written here
    /// are staged until [`commit`](Self::commit) or [`discard`](Self::discard)
    /// is called.
    pub fn mountpoint(&self) -> PathBuf {
        self.overlay.handle().mount_point().to_path_buf()
    }

    /// Atomically merges all upper-layer changes back into the lower directory.
    ///
    /// Uses [`OverlayAction::CommitAtomic`]: a backup-and-swap strategy that
    /// leaves the lower directory intact if the merge itself fails midway,
    /// ensuring the on-disk state is never left in a partially-written
    /// condition.
    pub fn commit(&mut self) {
        self.overlay.overlay_action(OverlayAction::CommitAtomic);
    }

    /// Drops the upper layer without touching the lower directory.
    ///
    /// Use this as the rollback path when an operation fails partway through —
    /// the lower directory is guaranteed to be unmodified.
    pub fn discard(&mut self) {
        self.overlay.overlay_action(OverlayAction::Discard);
    }
}
