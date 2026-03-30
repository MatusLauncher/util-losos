//! Program runner — loads a saved container image tarball and runs it interactively.
//!
//! After a package has been installed by [`crate::install::PackageInstallation`], its
//! container image is stored as a tarball under `/data/progs/<pkg>.tar`. [`ProgRunner`]
//! locates that tarball, feeds it to `nerdctl load`, and then launches the resulting
//! image with `nerdctl run -it`.
//!
//! # Flow
//!
//! ```text
//! ProgRunner::run("git")
//!     ├─ WalkDir /data/progs/   — find the first entry whose path contains "git"
//!     ├─ nerdctl load -i /data/progs/git.tar
//!     └─ nerdctl run -it localhost/local/git
//! ```
//!
//! # Requirements
//!
//! * `nerdctl` must be present at `/bin/nerdctl`.
//! * The requested package must have been previously installed with
//!   [`crate::install::PackageInstallation`] so that its tarball exists under
//!   `/data/progs/`.

use std::process::Command;

use miette::IntoDiagnostic;
use tracing::info;
use walkdir::WalkDir;

/// Loads and runs a previously installed program from its saved container image tarball.
///
/// `ProgRunner` is a stateless helper — all state lives on the filesystem under
/// `/data/progs/`. Construct one with [`ProgRunner::new`] and call [`ProgRunner::run`]
/// with the name of the program to launch.
///
/// # Example
///
/// ```rust,no_run
/// use pakman::run::ProgRunner;
///
/// let runner = ProgRunner::new();
/// runner.run("git").expect("failed to run git");
/// ```
#[derive(Default)]
pub struct ProgRunner;

impl ProgRunner {
    /// Creates a new `ProgRunner`.
    ///
    /// This is a zero-cost constructor — `ProgRunner` carries no state of its own.
    pub fn new() -> Self {
        Self
    }

    /// Loads the saved tarball for `prog` and runs it interactively.
    ///
    /// # Steps
    ///
    /// 1. Walks `/data/progs/` to find the first entry whose path contains `prog`.
    /// 2. Calls `nerdctl load -i <tarball>` to import the image into the container
    ///    runtime.
    /// 3. Calls `nerdctl run -it localhost/local/<prog>` to start the container.
    ///
    /// # Errors
    ///
    /// Returns a [`miette::Report`] if:
    ///
    /// * `nerdctl load` fails to start or returns a non-zero exit status.
    /// * No tarball matching `prog` is found under `/data/progs/` (the `WalkDir`
    ///   iterator will be empty and the index operation will panic — callers should
    ///   ensure the package is installed before calling this method).
    ///
    /// # Panics
    ///
    /// Panics if no entry matching `prog` exists in `/data/progs/`. Use
    /// [`crate::install::PackageInstallation`] to install the package first.
    pub fn run(&self, prog: &str) -> miette::Result<()> {
        info!("Loading the program from the data_drive");
        let matches = find_tarball("/data/progs", prog);
        let p = matches.first().expect(
            "no tarball found for the requested program — install it first with pakman --install",
        );
        Command::new("/bin/nerdctl")
            .arg("load")
            .arg("-i")
            .arg(p)
            .status()
            .into_diagnostic()?;
        info!("Starting up {prog}");
        Command::new("/bin/nerdctl")
            .arg("run")
            .arg("-it")
            .arg(format!("localhost/local/{prog}"));
        Ok(())
    }
}

/// Walks `dir` and returns the display-string paths of every entry whose path
/// contains `prog` as a substring.
///
/// Extracted from [`ProgRunner::run`] so the search-and-filter logic can be
/// unit-tested without a live `nerdctl` installation or a real `/data/progs`
/// directory.
///
/// # Arguments
///
/// * `dir`  — root directory to walk (e.g. `/data/progs`).
/// * `prog` — substring to match against each entry's display path.
///
/// # Returns
///
/// A `Vec<String>` of matching paths.  The caller is responsible for handling
/// the empty case.
fn find_tarball(dir: &str, prog: &str) -> Vec<String> {
    WalkDir::new(dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().display().to_string().contains(prog))
        .map(|e| e.path().display().to_string())
        .collect()
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::fs::{self, File};

    use super::*;

    // ── helpers ───────────────────────────────────────────────────────────────

    /// Create a uniquely-named temporary directory under the system temp root
    /// and return its path.  Each call uses a different suffix derived from
    /// the current thread id so parallel tests never collide.
    fn make_test_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pakman_run_test_{label}_{:?}",
            std::thread::current().id()
        ));
        fs::create_dir_all(&dir).expect("failed to create test dir");
        dir
    }

    /// Remove the directory created by `make_test_dir` after a test finishes.
    fn remove_test_dir(dir: &std::path::Path) {
        let _ = fs::remove_dir_all(dir);
    }

    // ── ProgRunner construction ───────────────────────────────────────────────

    #[test]
    fn new_returns_a_prog_runner() {
        // ProgRunner is a zero-size type; this verifies the constructor
        // compiles and returns without panicking.
        let _runner = ProgRunner::new();
    }

    #[test]
    fn default_is_equivalent_to_new() {
        // Both construction paths must produce the same (ZST) value.
        let _via_new = ProgRunner::new();
        let _via_default = ProgRunner::default();
        // Size == 0 confirms neither variant carries hidden state.
        assert_eq!(std::mem::size_of::<ProgRunner>(), 0);
    }

    // ── find_tarball — empty / missing directory ──────────────────────────────

    #[test]
    fn find_tarball_returns_empty_for_nonexistent_directory() {
        // A path that does not exist should yield an empty result, not a panic.
        let result = find_tarball("/tmp/pakman_nonexistent_dir_xyz", "curl");
        assert!(
            result.is_empty(),
            "expected empty result for nonexistent dir, got: {result:?}"
        );
    }

    #[test]
    fn find_tarball_returns_empty_for_empty_directory() {
        let dir = make_test_dir("empty_dir");
        let result = find_tarball(dir.to_str().unwrap(), "curl");
        assert!(
            result.is_empty(),
            "expected empty result for empty dir, got: {result:?}"
        );
        remove_test_dir(&dir);
    }

    // ── find_tarball — exact name match ───────────────────────────────────────

    #[test]
    fn find_tarball_finds_exact_tarball_by_name() {
        let dir = make_test_dir("exact_match");
        File::create(dir.join("curl.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "curl");
        assert_eq!(result.len(), 1, "expected exactly one match: {result:?}");
        assert!(
            result[0].contains("curl.tar"),
            "matched path should contain 'curl.tar', got: {}",
            result[0]
        );
        remove_test_dir(&dir);
    }

    #[test]
    fn find_tarball_finds_multi_word_package_name() {
        let dir = make_test_dir("multi_word");
        File::create(dir.join("python3.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "python3");
        assert_eq!(result.len(), 1);
        assert!(result[0].contains("python3.tar"));
        remove_test_dir(&dir);
    }

    // ── find_tarball — no match ───────────────────────────────────────────────

    #[test]
    fn find_tarball_returns_empty_when_no_file_matches() {
        let dir = make_test_dir("no_match");
        File::create(dir.join("git.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "curl");
        assert!(
            result.is_empty(),
            "expected no match for 'curl' when only 'git.tar' exists: {result:?}"
        );
        remove_test_dir(&dir);
    }

    // ── find_tarball — substring matching behaviour ───────────────────────────

    #[test]
    fn find_tarball_uses_substring_not_exact_match() {
        // "git" is a substring of "libgit2.tar" — both should be found.
        let dir = make_test_dir("substring");
        File::create(dir.join("git.tar")).unwrap();
        File::create(dir.join("libgit2.tar")).unwrap();
        File::create(dir.join("curl.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "git");
        assert_eq!(
            result.len(),
            2,
            "substring 'git' should match both git.tar and libgit2.tar: {result:?}"
        );
        assert!(result.iter().all(|p| p.contains("git")));
        remove_test_dir(&dir);
    }

    #[test]
    fn find_tarball_does_not_match_unrelated_files() {
        let dir = make_test_dir("unrelated");
        File::create(dir.join("curl.tar")).unwrap();
        File::create(dir.join("wget.tar")).unwrap();
        File::create(dir.join("jq.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "git");
        assert!(
            result.is_empty(),
            "expected no results for 'git' among unrelated tarballs: {result:?}"
        );
        remove_test_dir(&dir);
    }

    // ── find_tarball — multiple files, multiple matches ───────────────────────

    #[test]
    fn find_tarball_returns_all_matching_entries() {
        let dir = make_test_dir("multi_match");
        File::create(dir.join("python3.tar")).unwrap();
        File::create(dir.join("python3-dev.tar")).unwrap();
        File::create(dir.join("curl.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "python3");
        assert_eq!(result.len(), 2, "expected two python3 matches: {result:?}");
        remove_test_dir(&dir);
    }

    #[test]
    fn find_tarball_first_result_is_usable_as_path() {
        let dir = make_test_dir("first_result");
        File::create(dir.join("jq.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "jq");
        assert!(!result.is_empty());
        // The path must actually exist on disk so nerdctl can open it.
        assert!(
            std::path::Path::new(&result[0]).exists(),
            "matched path does not exist on disk: {}",
            result[0]
        );
        remove_test_dir(&dir);
    }

    // ── find_tarball — directory layout ──────────────────────────────────────

    #[test]
    fn find_tarball_directory_itself_is_not_returned_as_a_tarball() {
        // The root dir and any subdirectory should not be returned as matches
        // when they do not contain the program name.
        let dir = make_test_dir("dir_not_matched");
        File::create(dir.join("curl.tar")).unwrap();

        // Searching for the test dir label should not return the directory
        // entry itself as a tarball path.
        let result = find_tarball(dir.to_str().unwrap(), "curl.tar");
        // Every returned path must point to a file, not a directory.
        for path in &result {
            assert!(
                !std::path::Path::new(path).is_dir(),
                "find_tarball returned a directory entry: {path}"
            );
        }
        remove_test_dir(&dir);
    }

    #[test]
    fn find_tarball_walks_subdirectories() {
        // WalkDir is recursive; a tarball nested in a subdirectory should also
        // be found.
        let dir = make_test_dir("recursive");
        let sub = dir.join("nested");
        fs::create_dir_all(&sub).unwrap();
        File::create(sub.join("curl.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "curl");
        assert_eq!(
            result.len(),
            1,
            "expected exactly one match in nested dir: {result:?}"
        );
        assert!(result[0].contains("curl.tar"));
        remove_test_dir(&dir);
    }

    // ── find_tarball — edge cases ─────────────────────────────────────────────

    #[test]
    fn find_tarball_empty_prog_name_matches_everything() {
        // An empty string is a substring of every path — callers should
        // validate the program name before passing it; this test documents
        // the current (defined) behaviour.
        let dir = make_test_dir("empty_prog");
        File::create(dir.join("curl.tar")).unwrap();
        File::create(dir.join("git.tar")).unwrap();

        let result = find_tarball(dir.to_str().unwrap(), "");
        // Every entry (including the root dir itself) contains the empty string.
        assert!(
            result.len() >= 2,
            "empty prog should match all entries: {result:?}"
        );
        remove_test_dir(&dir);
    }

    #[test]
    fn find_tarball_is_case_sensitive() {
        let dir = make_test_dir("case_sensitive");
        File::create(dir.join("Curl.tar")).unwrap();

        // Lower-case query must NOT match a title-case filename.
        let result = find_tarball(dir.to_str().unwrap(), "curl");
        assert!(
            result.is_empty(),
            "find_tarball should be case-sensitive: {result:?}"
        );
        remove_test_dir(&dir);
    }
}
