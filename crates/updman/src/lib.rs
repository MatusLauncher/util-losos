//! # updman — OS Update Manager for util-mdl
//!
//! `updman` is the update-manager crate for the `util-mdl` project. It
//! automates the process of bringing a running system up to date by pulling a
//! new OS image from an OCI registry and atomically swapping the payload onto
//! the BOOT partition.
//!
//! ## What it does
//!
//! 1. **Reads configuration** from the kernel command line (`/proc/cmdline`),
//!    extracting the three keys `base_url`, `tag`, and `hash`.
//! 2. **Pulls an OCI image** whose reference is formed as
//!    `{base_url}/{tag}` using [`nerdctl`](https://github.com/containerd/nerdctl).
//! 3. **Extracts the nested archive** `os.initramfs.tar.gz` that is bundled
//!    inside the pulled image layer.
//! 4. **Swaps the BOOT partition** by writing the extracted initramfs in place,
//!    making the new OS version active on the next boot.
//!
//! ## Public surface
//!
//! The crate re-exports [`UpdMan`] from the [`schema`] sub-module as its
//! primary type.
//!
//! | Item | Description |
//! |------|-------------|
//! | [`UpdMan`] | Central struct that owns the update configuration and drives the update sequence. |
//! | [`UpdMan::default()`] | Constructs an instance by reading `base_url`, `tag`, and `hash` from `/proc/cmdline`. |
//! | [`UpdMan::update()`] | Executes the full update sequence (pull → extract → swap). |
//!
//! ## Configuration — kernel command-line keys
//!
//! `updman` reads the following whitespace-separated `key=value` pairs from
//! `/proc/cmdline` at runtime:
//!
//! | Key | Example value | Description |
//! |-----|---------------|-------------|
//! | `base_url` | `registry.example.com/mtos-v2` | Base URL of the OCI registry and repository prefix. |
//! | `tag` | `util-mdl:v1.2.3` | Image name and tag to pull (`{base_url}/{tag}` forms the full reference). |
//! | `hash` | `sha256:abc123…` | Expected content digest used to verify the pulled image. |
//!
//! ## Example
//!
//! ```no_run
//! use updman::UpdMan;
//!
//! // Build from /proc/cmdline automatically.
//! let mgr = UpdMan::default();
//!
//! // Run the full update sequence.
//! mgr.update().expect("OS update failed");
//! ```

pub mod schema;
pub use schema::UpdMan;

#[cfg(test)]
mod tests {
    use crate::UpdMan;

    fn make_updman(base_url: &str, image_tag: &str, hash: &str) -> UpdMan {
        UpdMan::new(
            base_url.to_string(),
            image_tag.to_string(),
            hash.to_string(),
        )
    }

    // ── image_ref ─────────────────────────────────────────────────────────────

    #[test]
    fn image_ref_combines_base_url_and_image_tag() {
        let u = make_updman(
            "registry.example.com/mtos-v2",
            "util-mdl:latest",
            "sha256:abc123",
        );
        assert_eq!(
            u.image_ref(),
            "registry.example.com/mtos-v2/util-mdl:latest"
        );
    }

    #[test]
    fn image_ref_format_is_base_url_slash_image_tag() {
        let u = make_updman("myregistry.io", "myimage:v0", "sha256:00");
        let expected = format!("{}/{}", "myregistry.io", "myimage:v0");
        assert_eq!(u.image_ref(), expected);
    }

    #[test]
    fn image_ref_with_different_tag() {
        let u = make_updman("reg.io/ns", "app:v2", "sha256:ff");
        assert_eq!(u.image_ref(), "reg.io/ns/app:v2");
    }
}
